//! AI background removal with ISNet (general use), run locally through ONNX Runtime.
//!
//! The model (~179 MB) is downloaded once, with the user's consent, into the app's data folder
//! and checked against a fixed SHA-256 before use. Inference runs in-process on a worker thread;
//! nothing leaves the machine, and nothing keeps running after the app closes.
//!
//! Pre/post-processing follows rembg's ISNet session: RGB resized to 1024x1024 (Lanczos), divided
//! by the image's max value, minus mean (0.485, 0.456, 0.406), std 1, NCHW float32; output 0,
//! channel 0, min-max normalized, resized back to the source size.

use image::{imageops, GrayImage, RgbaImage};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

pub const MODEL_FILE: &str = "isnet-general-use.onnx";
pub const MODEL_URL: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx";
/// SHA-256 of the file published by rembg (its MD5 fc16ebd8b0c10d971d3513d564d01e29 matches the
/// value rembg's own code checks).
pub const MODEL_SHA256: &str = "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a";
pub const MODEL_BYTES: u64 = 178_648_008;
const SIZE: u32 = 1024;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];

pub fn model_path() -> Option<PathBuf> {
    crate::library::app_dir().map(|d| d.join("models").join(MODEL_FILE))
}

/// Why the model isn't usable (shown in the download prompt), or None if it is.
pub fn model_problem() -> Option<String> {
    let Some(p) = model_path() else { return Some("no APPDATA folder".into()) };
    match p.metadata() {
        Err(e) => Some(format!("not found at {} ({e})", p.display())),
        Ok(m) if m.len() != MODEL_BYTES => Some(format!("{} is {} bytes, expected {MODEL_BYTES}", p.display(), m.len())),
        Ok(_) => None,
    }
}

pub fn model_present() -> bool {
    model_path().is_some_and(|p| p.metadata().is_ok_and(|m| m.len() == MODEL_BYTES))
}

/// Download the model, reporting progress (bytes so far). Verified by SHA-256 before it is moved
/// into place, so a partial or tampered file is never used.
pub fn download_model(progress: impl Fn(u64)) -> Result<(), String> {
    let path = model_path().ok_or("no APPDATA folder")?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let part = path.with_extension("onnx.part");
    let resp = ureq::get(MODEL_URL).call().map_err(|e| format!("download failed: {e}"))?;
    let mut body = resp.into_body();
    let mut reader = body.with_config().limit(MODEL_BYTES + 1).reader();
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let (mut hasher, mut done, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done);
    }
    drop(file);
    let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if hash != MODEL_SHA256 {
        let _ = std::fs::remove_file(&part);
        return Err("the downloaded model failed its checksum, so it was discarded".into());
    }
    std::fs::rename(&part, &path).map_err(|e| e.to_string())
}

/// RGB (alpha ignored) -> 1x3x1024x1024 float32, rembg's ISNet normalization.
pub fn preprocess(src: &RgbaImage) -> Vec<f32> {
    let rgb = image::DynamicImage::ImageRgba8(src.clone()).to_rgb8();
    let rgb = imageops::resize(&rgb, SIZE, SIZE, imageops::FilterType::Lanczos3);
    let max = rgb.as_raw().iter().copied().max().unwrap_or(0).max(1) as f32;
    let plane = (SIZE * SIZE) as usize;
    let mut out = vec![0f32; 3 * plane];
    for (i, p) in rgb.pixels().enumerate() {
        for c in 0..3 {
            out[c * plane + i] = p[c] as f32 / max - MEAN[c];
        }
    }
    out
}

/// Model output (1024x1024 values) -> 0..255 mask at the source size.
pub fn postprocess(raw: &[f32], w: u32, h: u32) -> GrayImage {
    let (mi, ma) = raw.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let span = if ma > mi { ma - mi } else { 1.0 };
    let small = GrayImage::from_fn(SIZE, SIZE, |x, y| {
        let v = (raw[(y * SIZE + x) as usize] - mi) / span;
        image::Luma([(v * 255.0).round().clamp(0.0, 255.0) as u8])
    });
    imageops::resize(&small, w, h, imageops::FilterType::Lanczos3)
}

/// Mask stored as an RGBA image (white, alpha = mask) so projects and caches treat it like art.
pub fn mask_to_rgba(mask: &GrayImage) -> RgbaImage {
    RgbaImage::from_fn(mask.width(), mask.height(), |x, y| image::Rgba([255, 255, 255, mask.get_pixel(x, y)[0]]))
}

/// Source with its alpha multiplied by the mask's alpha. Colors are untouched.
pub fn apply_mask(src: &RgbaImage, mask: &RgbaImage) -> RgbaImage {
    let mut out = src.clone();
    if mask.dimensions() != src.dimensions() {
        return out; // shortcut: mismatched masks are ignored rather than stretched (never produced by us)
    }
    for (p, m) in out.pixels_mut().zip(mask.pixels()) {
        p[3] = ((p[3] as u32 * m[3] as u32 + 127) / 255) as u8;
    }
    out
}

/// Loaded once on first use (takes a second or two) and kept for the rest of the session.
static SESSION: Mutex<Option<ort::session::Session>> = Mutex::new(None);
static ENV: Mutex<Option<ort::environment::Environment>> = Mutex::new(None);

/// The ONNX Runtime environment, created once and shared by background removal and upscaling.
pub fn ort_environment() -> Result<ort::environment::Environment, String> {
    let mut env = ENV.lock().map_err(|_| "AI runtime state was poisoned")?;
    if env.is_none() {
        *env = Some(ort::init().with_name("clothing-studio").build().map_err(|e| e.to_string())?);
    }
    Ok(env.clone().unwrap())
}

/// Load an ONNX model for CPU inference (all cores).
pub fn load_session(path: &std::path::Path) -> Result<ort::session::Session, String> {
    let env = ort_environment()?;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    ort::session::Session::builder(&env)
        .map_err(|e| e.to_string())?
        .with_intra_threads(threads)
        .map_err(|e| e.to_string())?
        .commit_from_file(path)
        .map_err(|e| format!("could not load the model: {e}"))
}

/// Run ISNet on `src` (call from a worker thread). Returns the mask at the source size.
pub fn compute_mask(src: &RgbaImage) -> Result<GrayImage, String> {
    let path = model_path().filter(|_| model_present()).ok_or("the background-removal model isn't downloaded")?;
    let mut guard = SESSION.lock().map_err(|_| "model state was poisoned")?;
    if guard.is_none() {
        *guard = Some(load_session(&path)?);
    }
    let session = guard.as_mut().unwrap();
    let input = ort::value::Tensor::from_array((vec![1i64, 3, SIZE as i64, SIZE as i64], preprocess(src))).map_err(|e| e.to_string())?;
    let outputs = session.run(ort::inputs![input]).map_err(|e| format!("background removal failed: {e}"))?;
    let out = outputs[0].try_extract_array::<f32>().map_err(|e| e.to_string())?;
    let flat: Vec<f32> = out.iter().copied().collect();
    if flat.len() < (SIZE * SIZE) as usize {
        return Err(format!("unexpected model output size {}", flat.len()));
    }
    // first channel of the first output
    Ok(postprocess(&flat[..(SIZE * SIZE) as usize], src.width(), src.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_layout_and_normalization() {
        let img = RgbaImage::from_pixel(8, 4, image::Rgba([255, 0, 255, 0])); // alpha ignored
        let t = preprocess(&img);
        let plane = (SIZE * SIZE) as usize;
        assert_eq!(t.len(), 3 * plane);
        assert!((t[0] - (1.0 - MEAN[0])).abs() < 1e-3, "R plane first");
        assert!((t[plane] - (0.0 - MEAN[1])).abs() < 1e-3, "G plane second");
        assert!((t[2 * plane + plane - 1] - (1.0 - MEAN[2])).abs() < 1e-3, "B plane last");
    }

    #[test]
    fn postprocess_normalizes_and_resizes() {
        let mut raw = vec![-2.0f32; (SIZE * SIZE) as usize];
        // right half = foreground
        for y in 0..SIZE {
            for x in SIZE / 2..SIZE {
                raw[(y * SIZE + x) as usize] = 5.0;
            }
        }
        let m = postprocess(&raw, 40, 20);
        assert_eq!(m.dimensions(), (40, 20));
        assert_eq!(m.get_pixel(2, 10)[0], 0);
        assert_eq!(m.get_pixel(37, 10)[0], 255);
    }

    #[test]
    fn mask_multiplies_alpha_only() {
        let src = RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 200]));
        let mask = mask_to_rgba(&GrayImage::from_raw(2, 1, vec![255, 0]).unwrap());
        let out = apply_mask(&src, &mask);
        assert_eq!(*out.get_pixel(0, 0), image::Rgba([10, 20, 30, 200]));
        assert_eq!(*out.get_pixel(1, 0), image::Rgba([10, 20, 30, 0]));
    }

    /// Runs the real model (needs it downloaded): a dark disc on a white background.
    #[test]
    #[ignore]
    fn real_model_separates_disc_from_background() {
        let img = RgbaImage::from_fn(300, 300, |x, y| {
            let d = ((x as f32 - 150.0).powi(2) + (y as f32 - 150.0).powi(2)).sqrt();
            if d < 90.0 { image::Rgba([200, 30, 40, 255]) } else { image::Rgba([255, 255, 255, 255]) }
        });
        let t = std::time::Instant::now();
        let m = compute_mask(&img).unwrap();
        eprintln!("ISNET first run (incl. load) {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let _ = compute_mask(&img).unwrap();
        eprintln!("ISNET second run {:?}", t.elapsed());
        eprintln!("ISNET center {} corner {}", m.get_pixel(150, 150)[0], m.get_pixel(10, 10)[0]);
        assert!(m.get_pixel(150, 150)[0] > 200, "subject kept");
        assert!(m.get_pixel(10, 10)[0] < 40, "background removed");
    }
}

