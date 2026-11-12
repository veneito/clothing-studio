//! Update check: asks GitHub for the latest release of this app and, if it's newer, downloads
//! its Windows installer (verified against the SHA-256 GitHub publishes for release assets).
//! Sends nothing about the user; just an anonymous request for public release info.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

const LATEST_URL: &str = "https://api.github.com/repos/veneito/clothing-studio/releases/latest";
const MAX_INSTALLER_BYTES: u64 = 200_000_000;

#[derive(Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub installer_url: String,
    /// "sha256:<hex>" as published by GitHub, when available.
    pub digest: Option<String>,
}

/// "v1.2.3" / "1.2.3" -> (1, 2, 3)
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

/// The latest release, if it's newer than `current`. Ok(None) = up to date.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let mut resp = agent
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "ClothingStudio-update-check")
        .call()
        .map_err(|e| format!("couldn't reach GitHub ({e})"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    parse_latest(&text, current)
}

fn parse_latest(text: &str, current: &str) -> Result<Option<Release>, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "unexpected reply from GitHub".to_owned())?;
    let tag = v.get("tag_name").and_then(Value::as_str).ok_or("no release found")?;
    let (Some(latest), Some(mine)) = (parse_version(tag), parse_version(current)) else {
        return Err(format!("can't compare versions {tag} and {current}"));
    };
    if latest <= mine {
        return Ok(None);
    }
    let asset = v
        .get("assets")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().find(|x| x.get("name").and_then(Value::as_str).is_some_and(|n| n.ends_with("-setup.exe"))))
        .ok_or("the new release has no installer yet")?;
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_owned(),
        page: v.get("html_url").and_then(Value::as_str).unwrap_or_default().to_owned(),
        installer_url: asset.get("browser_download_url").and_then(Value::as_str).ok_or("installer link missing")?.to_owned(),
        digest: asset.get("digest").and_then(Value::as_str).map(str::to_owned),
    }))
}

/// Download the installer to the temp folder, verifying GitHub's SHA-256 when it's given.
pub fn download(release: &Release, progress: impl Fn(u64)) -> Result<PathBuf, String> {
    let resp = ureq::get(&release.installer_url).header("User-Agent", "ClothingStudio-update-check").call().map_err(|e| format!("download failed: {e}"))?;
    let mut body = resp.into_body();
    let mut reader = body.with_config().limit(MAX_INSTALLER_BYTES).reader();
    let (mut bytes, mut buf) = (Vec::new(), vec![0u8; 1 << 16]);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        progress(bytes.len() as u64);
    }
    if let Some(expected) = release.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if !got.eq_ignore_ascii_case(expected) {
            return Err("the downloaded installer didn't match GitHub's checksum, so it wasn't run".into());
        }
    }
    let path = std::env::temp_dir().join(format!("Clothing.Studio_{}_x64-setup.exe", release.version));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"tag_name":"v1.2.0","html_url":"https://github.com/veneito/clothing-studio/releases/tag/v1.2.0",
        "assets":[{"name":"notes.txt","browser_download_url":"x"},
                  {"name":"Clothing.Studio_1.2.0_x64-setup.exe","browser_download_url":"https://example/setup.exe","digest":"sha256:abc"}]}"#;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v1.10.0"), Some((1, 10, 0)));
        assert!(parse_version("1.10.0") > parse_version("1.9.9"));
        assert_eq!(parse_version("2.1"), Some((2, 1, 0)));
        assert_eq!(parse_version("abc"), None);
    }

    #[test]
    fn finds_a_newer_installer_and_ignores_older_or_equal() {
        let r = parse_latest(REPLY, "1.0.3").unwrap().unwrap();
        assert_eq!(r.version, "1.2.0");
        assert_eq!(r.installer_url, "https://example/setup.exe");
        assert_eq!(r.digest.as_deref(), Some("sha256:abc"));
        assert_eq!(parse_latest(REPLY, "1.2.0").unwrap(), None, "same version");
        assert_eq!(parse_latest(REPLY, "1.3.0").unwrap(), None, "running a newer build");
        assert!(parse_latest(r#"{"tag_name":"v9.0.0","assets":[]}"#, "1.0.0").is_err(), "no installer yet");
        assert!(parse_latest(r#"{"message":"Not Found"}"#, "1.0.0").is_err());
    }
}
