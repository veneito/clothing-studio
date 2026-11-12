//! AI upscaling ×4 (Real-ESRGAN general x4v3, Qualcomm AI Hub ONNX export, BSD-3), run locally
//! through ONNX Runtime. For small pictures that would otherwise be stretched blurry on the shirt.
//!
//! The model takes fixed 128x128 RGB tiles (0..1, NCHW) and returns 512x512. Pictures are cut into
//! overlapping tiles and only each tile's centre is kept, so there are no seams. Transparency
//! isn't modelled: alpha is enlarged separately with Lanczos.

use image::{imageops, RgbaImage};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;

pub const MODEL_URL: &str =
    "https://qaihub-public-assets.s3.us-west-2.amazonaws.com/qai-hub-models/models/real_esrgan_general_x4v3/releases/v0.66.0/real_esrgan_general_x4v3-onnx-float.zip";
/// SHA-256 of the zip (its MD5 matches the server's ETag ed2635507dce0a7aefb2f4d89a9275c7).
const ZIP_SHA256: &str = "fc094329b24a00c936ebc1cc4c51bc59633e7dc36f4a553344342171cbd3c7cf";
pub const ZIP_BYTES: u64 = 4_540_895;
const ONNX: (&str, u64) = ("real_esrgan_general_x4v3.onnx", 161_087);
/// Weights live in a separate file the .onnx refers to by name; both must sit side by side.
const DATA: (&str, u64) = ("real_esrgan_general_x4v3.data", 4_836_096);

const TILE: u32 = 128;
const SCALE: u32 = 4;
/// Context kept around each tile and thrown away after upscaling (avoids seams).
const PAD: u32 = 12;
/// Pictures bigger than this don't need upscaling (and would take long).
pub const MAX_SIDE: u32 = 1024;

pub fn model_dir() -> Option<PathBuf> {
    crate::library::app_dir().map(|d| d.join("models").join("real-esrgan-general-x4v3"))
}

pub fn model_present() -> bool {
    model_dir().is_some_and(|d| [ONNX, DATA].iter().all(|(f, n)| d.join(f).metadata().is_ok_and(|m| m.len() == *n)))
}

/// Download the zip (verified by SHA-256) and unpack the model's two files.
pub fn download_model(progress: impl Fn(u64)) -> Result<(), String> {
    let dir = model_dir().ok_or("no APPDATA folder")?;
    let resp = ureq::get(MODEL_URL).call().map_err(|e| format!("download failed: {e}"))?;
    let mut body = resp.into_body();
    let mut reader = body.with_config().limit(ZIP_BYTES + 1).reader();
    let (mut zip_bytes, mut buf) = (Vec::with_capacity(ZIP_BYTES as usize), vec![0u8; 1 << 16]);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        zip_bytes.extend_from_slice(&buf[..n]);
        progress(zip_bytes.len() as u64);
    }
    let hash: String = Sha256::digest(&zip_bytes).iter().map(|b| format!("{b:02x}")).collect();
    if hash != ZIP_SHA256 {
        return Err("the downloaded model failed its checksum, so it was discarded".into());
    }
    unpack(&zip_bytes, &dir)
}

fn unpack(zip_bytes: &[u8], dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| format!("bad model archive: {e}"))?;
    for (name, size) in [ONNX, DATA] {
        // the zip has them in a sub-folder; look them up by file name only
        let idx = (0..zip.len())
            .find(|&i| zip.by_index(i).is_ok_and(|f| f.name().is_ok_and(|n| n.rsplit('/').next() == Some(name))))
            .ok_or_else(|| format!("{name} is missing from the model archive"))?;
        let mut f = zip.by_index(idx).map_err(|e| e.to_string())?;
        let mut data = Vec::with_capacity(size as usize);
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        if data.len() as u64 != size {
            return Err(format!("{name} in the archive has an unexpected size"));
        }
        let part = dir.join(format!("{name}.part"));
        std::fs::write(&part, &data).map_err(|e| e.to_string())?;
        std::fs::rename(&part, dir.join(name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

static SESSION: Mutex<Option<ort::session::Session>> = Mutex::new(None);

/// Upscale `src` ×4 with the model (call from a worker thread). `progress` gets "tile i/n".
pub fn upscale(src: &RgbaImage, progress: impl Fn(String)) -> Result<RgbaImage, String> {
    if !model_present() {
        return Err("the upscaling model isn't downloaded".into());
    }
    let dir = model_dir().unwrap();
    let mut guard = SESSION.lock().map_err(|_| "model state was poisoned")?;
    if guard.is_none() {
        *guard = Some(crate::bgremove::load_session(&dir.join(ONNX.0))?);
    }
    let session = guard.as_mut().unwrap();
    upscale_tiled(src, &progress, |tile| {
        let input = ort::value::Tensor::from_array((vec![1i64, 3, TILE as i64, TILE as i64], tile.to_vec())).map_err(|e| e.to_string())?;
        let outputs = session.run(ort::inputs![input]).map_err(|e| format!("upscaling failed: {e}"))?;
        let out = outputs[0].try_extract_array::<f32>().map_err(|e| e.to_string())?;
        Ok(out.iter().copied().collect())
    })
}

/// Tile `src`, run `model` (128x128 RGB NCHW 0..1 -> 512x512) on each tile, stitch the centres.
fn upscale_tiled(src: &RgbaImage, progress: &dyn Fn(String), mut model: impl FnMut(&[f32]) -> Result<Vec<f32>, String>) -> Result<RgbaImage, String> {
    let (w, h) = src.dimensions();
    if w == 0 || h == 0 {
        return Err("the picture is empty".into());
    }
    if w.max(h) > MAX_SIDE {
        return Err(format!("the picture is already {w}×{h}; upscaling is for small pictures (up to {MAX_SIDE} px)"));
    }
    let step = TILE - 2 * PAD;
    let (nx, ny) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = RgbaImage::new(w * SCALE, h * SCALE);
    let plane = (TILE * TILE) as usize;
    let mut done = 0;
    for ty in 0..ny {
        for tx in 0..nx {
            // tile covers [x0, x0+TILE) with PAD of context; edges repeat the border pixel
            let (x0, y0) = ((tx * step) as i64 - PAD as i64, (ty * step) as i64 - PAD as i64);
            let mut input = vec![0f32; 3 * plane];
            for y in 0..TILE {
                for x in 0..TILE {
                    let sx = (x0 + x as i64).clamp(0, w as i64 - 1) as u32;
                    let sy = (y0 + y as i64).clamp(0, h as i64 - 1) as u32;
                    let p = src.get_pixel(sx, sy);
                    for c in 0..3 {
                        input[c * plane + (y * TILE + x) as usize] = p[c] as f32 / 255.0;
                    }
                }
            }
            let result = model(&input)?;
            let oplane = (TILE * SCALE * TILE * SCALE) as usize;
            if result.len() < 3 * oplane {
                return Err(format!("unexpected model output size {}", result.len()));
            }
            // keep the centre `step` source pixels of this tile
            for y in 0..step * SCALE {
                for x in 0..step * SCALE {
                    let (ox, oy) = (tx * step * SCALE + x, ty * step * SCALE + y);
                    if ox >= w * SCALE || oy >= h * SCALE {
                        continue;
                    }
                    let ti = ((y + PAD * SCALE) * TILE * SCALE + (x + PAD * SCALE)) as usize;
                    let px = out.get_pixel_mut(ox, oy);
                    for c in 0..3 {
                        px[c] = (result[c * oplane + ti] * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
            done += 1;
            progress(format!("Upscaling… {done}/{}", nx * ny));
        }
    }
    // alpha: smooth ×4 enlargement of the original transparency
    let alpha = imageops::resize(&image::GrayImage::from_fn(w, h, |x, y| image::Luma([src.get_pixel(x, y)[3]])), w * SCALE, h * SCALE, imageops::FilterType::Lanczos3);
    for (p, a) in out.pixels_mut().zip(alpha.pixels()) {
        p[3] = a[0];
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in model: nearest-neighbour ×4 of the tile.
    fn nearest_x4(tile: &[f32]) -> Result<Vec<f32>, String> {
        let (t, o) = (TILE as usize, (TILE * SCALE) as usize);
        let mut out = vec![0f32; 3 * o * o];
        for c in 0..3 {
            for y in 0..o {
                for x in 0..o {
                    out[c * o * o + y * o + x] = tile[c * t * t + (y / 4) * t + x / 4];
                }
            }
        }
        Ok(out)
    }

    #[test]
    fn tiles_stitch_without_seams_or_shifts() {
        // non-multiple size so edge tiles are partial; distinct value per pixel
        let src = RgbaImage::from_fn(237, 150, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x * 7 + y * 3) % 256) as u8, 255]));
        let out = upscale_tiled(&src, &|_| {}, nearest_x4).unwrap();
        assert_eq!(out.dimensions(), (237 * 4, 150 * 4));
        for (x, y) in [(0, 0), (1, 1), (103, 50), (104, 50), (105, 103), (236, 149), (120, 104)] {
            let s = src.get_pixel(x, y);
            for (dx, dy) in [(0, 0), (3, 3)] {
                let o = out.get_pixel(x * 4 + dx, y * 4 + dy);
                assert_eq!((o[0], o[1], o[2]), (s[0], s[1], s[2]), "pixel ({x},{y}) +({dx},{dy})");
            }
        }
    }

    #[test]
    fn keeps_transparency_and_refuses_big_pictures() {
        let src = RgbaImage::from_fn(40, 40, |x, _| image::Rgba([200, 10, 10, if x < 20 { 255 } else { 0 }]));
        let out = upscale_tiled(&src, &|_| {}, nearest_x4).unwrap();
        assert_eq!(out.get_pixel(10, 80)[3], 255);
        assert_eq!(out.get_pixel(150, 80)[3], 0);
        let big = RgbaImage::new(MAX_SIDE + 1, 10);
        assert!(upscale_tiled(&big, &|_| {}, nearest_x4).is_err());
    }

    #[test]
    fn unpack_rejects_archives_without_the_model() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            z.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut z, b"hi").unwrap();
            z.finish().unwrap();
        }
        let dir = std::env::temp_dir().join(format!("cs-upscale-test-{}", std::process::id()));
        assert!(unpack(buf.get_ref(), &dir).unwrap_err().contains("missing"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs the real model from $CS_UPSCALE_TEST_DIR (the unpacked zip folder).
    #[test]
    #[ignore]
    fn real_model_sharpens_a_small_picture() {
        let Some(dir) = std::env::var_os("CS_UPSCALE_TEST_DIR") else { return };
        let session = crate::bgremove::load_session(&std::path::Path::new(&dir).join(ONNX.0)).unwrap();
        let mut session = session;
        let custom = std::env::var_os("CS_UPSCALE_TEST_SRC").map(|p| image::open(p).unwrap().to_rgba8());
        let src = custom.clone().unwrap_or_else(|| RgbaImage::from_fn(60, 40, |x, y| if (x / 10 + y / 10) % 2 == 0 { image::Rgba([230, 40, 40, 255]) } else { image::Rgba([250, 250, 250, 255]) }));
        let t = std::time::Instant::now();
        let out = upscale_tiled(&src, &|_| {}, |tile| {
            let input = ort::value::Tensor::from_array((vec![1i64, 3, TILE as i64, TILE as i64], tile.to_vec())).map_err(|e| e.to_string())?;
            let outputs = session.run(ort::inputs![input]).map_err(|e| e.to_string())?;
            let o = outputs[0].try_extract_array::<f32>().map_err(|e| e.to_string())?;
            Ok(o.iter().copied().collect())
        })
        .unwrap();
        eprintln!("UPSCALE 60x40 -> {:?} in {:?}", out.dimensions(), t.elapsed());
        if let Some(p) = std::env::var_os("CS_UPSCALE_TEST_OUT") {
            out.save(p).unwrap();
        }
        if custom.is_some() {
            return;
        }
        // colours preserved: centre of a red square stays red, of a white square stays white
        let red = out.get_pixel(20, 20);
        let white = out.get_pixel(60, 20);
        assert!(red[0] > 180 && red[1] < 90, "red kept: {red:?}");
        assert!(white[0] > 220 && white[2] > 220, "white kept: {white:?}");
    }
}
