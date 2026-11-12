//! Optional "Upload to Roblox": sends an exported PNG to the user's Roblox account through Open
//! Cloud (Assets API) so it can be used in Studio. Opt-in: nothing here runs until the user adds
//! an API key. The key lives in Windows Credential Manager, never in a plain file or a log.
//!
//! Docs: create.roblox.com/docs/cloud/guides/usage-assets (Create Asset, Operations) and
//! create.roblox.com/docs/cloud/auth/api-keys (introspect).

use serde_json::Value;
use std::time::{Duration, Instant};

const ASSETS_URL: &str = "https://apis.roblox.com/assets/v1/assets";
const OPERATIONS_URL: &str = "https://apis.roblox.com/assets/v1/operations";
const INTROSPECT_URL: &str = "https://apis.roblox.com/api-keys/v1/introspect";
const KEYRING_SERVICE: &str = "Clothing Studio";
const KEYRING_USER: &str = "roblox-open-cloud-api-key";

// ---------------------------------------------------------------- key storage

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| format!("Credential Manager unavailable: {e}"))
}

pub fn load_key() -> Option<String> {
    entry().ok()?.get_password().ok().filter(|k| !k.trim().is_empty())
}

pub fn save_key(key: &str) -> Result<(), String> {
    entry()?.set_password(key.trim()).map_err(|e| format!("could not store the key: {e}"))
}

pub fn delete_key() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("could not remove the key: {e}")),
    }
}

// ---------------------------------------------------------------- HTTP

fn agent() -> ureq::Agent {
    // Read error bodies instead of failing on 4xx/5xx, so Roblox's message can be shown.
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(60))).build().into()
}

/// (status, body)
fn finish(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<(u16, String), String> {
    let mut resp = resp.map_err(|e| format!("couldn't reach Roblox ({e})"))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().read_to_string().unwrap_or_default();
    Ok((status, body))
}

/// Best human-readable error from a Roblox error body.
fn roblox_error(status: u16, body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        ["message", "error"].iter().find_map(|k| v.get(k).and_then(|m| m.as_str().map(str::to_owned)))
    });
    match (status, msg) {
        (401 | 403, m) => format!("Roblox refused the API key ({status}{}). Check it has the Assets API with Read and Write, and that it isn't expired or IP-restricted.", m.map(|m| format!(": {m}")).unwrap_or_default()),
        (_, Some(m)) => format!("Roblox said: {m} ({status})"),
        (_, None) => format!("Roblox returned HTTP {status}"),
    }
}

#[derive(Debug)]
pub struct KeyInfo {
    pub name: String,
    pub user_id: String,
}

/// Who the key belongs to (its owner is the uploader), via the introspect endpoint.
pub fn introspect(key: &str) -> Result<KeyInfo, String> {
    let body = serde_json::json!({ "apiKey": key.trim() }).to_string();
    let (status, text) = finish(agent().post(INTROSPECT_URL).header("Content-Type", "application/json").send(body.as_bytes()))?;
    if status != 200 {
        return Err(roblox_error(status, &text));
    }
    parse_introspect(&text)
}

fn parse_introspect(text: &str) -> Result<KeyInfo, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "unexpected reply from Roblox".to_owned())?;
    if v.get("expired").and_then(Value::as_bool) == Some(true) {
        return Err("this API key has expired; create a new one on the Creator Hub".into());
    }
    if v.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Err("this API key is disabled".into());
    }
    let user_id = match v.get("authorizedUserId") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => return Err("this key isn't owned by a user account (group keys aren't supported yet)".into()),
    };
    let name = v.get("name").and_then(Value::as_str).unwrap_or("API key").to_owned();
    Ok(KeyInfo { name, user_id })
}

/// The multipart body for Create Asset: a JSON "request" part and the PNG "fileContent" part.
fn multipart(boundary: &str, request_json: &str, file_name: &str, png: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"request\"\r\nContent-Type: application/json\r\n\r\n{request_json}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"fileContent\"; filename=\"{file_name}\"\r\nContent-Type: image/png\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn request_json(user_id: &str, display_name: &str) -> String {
    serde_json::json!({
        "assetType": "Decal",
        "displayName": display_name.chars().take(50).collect::<String>(),
        "description": "Made with Clothing Studio",
        "creationContext": { "creator": { "userId": user_id } }
    })
    .to_string()
}

pub struct Uploaded {
    pub asset_id: String,
    /// e.g. "MODERATION_STATE_APPROVED" / "..._REVIEWING", if Roblox reported it.
    pub moderation: Option<String>,
}

/// What an operation reply says: finished (with result or error) or still running.
#[derive(Debug, PartialEq)]
enum OpState {
    Running(Option<String>), // operation id/path to poll, if given
    Done { asset_id: String, moderation: Option<String> },
    Failed(String),
}

fn parse_operation(text: &str) -> OpState {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return OpState::Failed("unexpected reply from Roblox".into()) };
    let id = v
        .get("operationId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| v.get("path").and_then(Value::as_str).and_then(|p| p.strip_prefix("operations/")).map(str::to_owned));
    if v.get("done").and_then(Value::as_bool) != Some(true) {
        return OpState::Running(id);
    }
    // the docs mention both `error` and `status` for failures
    if let Some(e) = v.get("error").or_else(|| v.get("status")) {
        let msg = e.get("message").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| e.to_string());
        return OpState::Failed(msg);
    }
    let r = v.get("response").unwrap_or(&Value::Null);
    match r.get("assetId").map(|a| a.as_str().map(str::to_owned).unwrap_or_else(|| a.to_string())) {
        Some(asset_id) => OpState::Done {
            asset_id,
            moderation: r.get("moderationResult").and_then(|m| m.get("moderationState")).and_then(Value::as_str).map(str::to_owned),
        },
        None => OpState::Failed("Roblox finished but sent no asset ID".into()),
    }
}

/// Upload a PNG as a Decal and wait (up to ~60 s) for its asset ID. `progress` gets status text.
pub fn upload_png(key: &str, user_id: &str, display_name: &str, png: &[u8], progress: impl Fn(String)) -> Result<Uploaded, String> {
    let boundary = format!("----ClothingStudio{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    let body = multipart(&boundary, &request_json(user_id, display_name), "clothing.png", png);
    progress("Uploading to Roblox…".into());
    let (status, text) = finish(
        agent()
            .post(ASSETS_URL)
            .header("x-api-key", key.trim())
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .send(&body[..]),
    )?;
    if status != 200 {
        return Err(roblox_error(status, &text));
    }
    let start = Instant::now();
    let mut state = parse_operation(&text);
    loop {
        match state {
            OpState::Done { asset_id, moderation } => return Ok(Uploaded { asset_id, moderation }),
            OpState::Failed(e) => return Err(format!("upload failed: {e}")),
            OpState::Running(None) => return Err("Roblox accepted the upload but gave no operation to follow".into()),
            OpState::Running(Some(id)) => {
                if start.elapsed() > Duration::from_secs(60) {
                    return Err("Roblox is still processing the upload; check Creator Hub → Creations → Decals in a minute".into());
                }
                progress("Roblox is processing the upload…".into());
                std::thread::sleep(Duration::from_millis(1500)); // well under the 300/min poll limit
                let (status, text) = finish(agent().get(format!("{OPERATIONS_URL}/{id}")).header("x-api-key", key.trim()).call())?;
                if status != 200 {
                    return Err(roblox_error(status, &text));
                }
                state = match parse_operation(&text) {
                    OpState::Running(None) => OpState::Running(Some(id)),
                    s => s,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_has_both_parts_and_raw_png() {
        let png = [0x89, b'P', b'N', b'G', 0, 1, 2];
        let body = multipart("B", &request_json("123", "My shirt"), "clothing.png", &png);
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("--B\r\nContent-Disposition: form-data; name=\"request\""));
        assert!(text.contains(r#""assetType":"Decal""#) && text.contains(r#""userId":"123""#));
        assert!(text.contains("name=\"fileContent\"; filename=\"clothing.png\"\r\nContent-Type: image/png\r\n\r\n"));
        assert!(body.windows(png.len()).any(|w| w == png), "PNG bytes included verbatim");
        assert!(text.ends_with("\r\n--B--\r\n"));
    }

    #[test]
    fn parses_operations_from_the_docs() {
        assert_eq!(parse_operation(r#"{"path":"operations/abc-123","done":false}"#), OpState::Running(Some("abc-123".into())));
        assert_eq!(parse_operation(r#"{"path":"operations/x"}"#), OpState::Running(Some("x".into())));
        let done = r#"{"path":"operations/x","done":true,"response":{"path":"assets/2205400862","assetId":"2205400862","assetType":"ASSET_TYPE_DECAL","moderationResult":{"moderationState":"MODERATION_STATE_APPROVED"}}}"#;
        assert_eq!(parse_operation(done), OpState::Done { asset_id: "2205400862".into(), moderation: Some("MODERATION_STATE_APPROVED".into()) });
        assert!(matches!(parse_operation(r#"{"done":true,"error":{"message":"Asset name is inappropriate"}}"#), OpState::Failed(m) if m.contains("inappropriate")));
        assert!(matches!(parse_operation("not json"), OpState::Failed(_)));
    }

    #[test]
    fn parses_introspect_and_rejects_bad_keys() {
        let ok = parse_introspect(r#"{"name":"Clothing","authorizedUserId":234,"scopes":[],"enabled":true,"expired":false}"#).unwrap();
        assert_eq!((ok.name.as_str(), ok.user_id.as_str()), ("Clothing", "234"));
        assert!(parse_introspect(r#"{"authorizedUserId":1,"expired":true}"#).unwrap_err().contains("expired"));
        assert!(parse_introspect(r#"{"name":"g","enabled":true}"#).is_err(), "no user id");
    }

    #[test]
    fn error_messages_are_readable() {
        assert!(roblox_error(401, "{}").contains("refused the API key"));
        assert_eq!(roblox_error(400, r#"{"message":"Bad file"}"#), "Roblox said: Bad file (400)");
    }
}
