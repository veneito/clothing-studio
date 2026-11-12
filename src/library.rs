//! The user's saved templates (PNG files in the app's data folder) and small persisted settings.

use image::RgbaImage;
use std::path::{Path, PathBuf};

/// `%APPDATA%\clothing-studio`.
pub fn app_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| Path::new(&d).join("clothing-studio"))
}

pub fn templates_dir() -> Option<PathBuf> {
    app_dir().map(|d| d.join("templates"))
}

/// The user's tattoo templates (drawn under the shirt template).
pub fn tattoos_dir() -> Option<PathBuf> {
    app_dir().map(|d| d.join("tattoos"))
}

/// The user's shading overlays (listed first in the Shading menu).
pub fn shading_dir() -> Option<PathBuf> {
    app_dir().map(|d| d.join("shading"))
}

/// A one-line setting such as `last_rig`; missing or unreadable reads as None.
pub fn read_setting(name: &str) -> Option<String> {
    let s = std::fs::read_to_string(app_dir()?.join(format!("{name}.txt"))).ok()?;
    Some(s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Best effort: losing a setting only costs a convenience.
pub fn write_setting(name: &str, value: &str) {
    if let Some(dir) = app_dir() {
        let _ = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(format!("{name}.txt")), value));
    }
}

/// Saved templates as (display name, path), sorted by name.
pub fn list(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    out.sort_by_key(|(n, _)| n.to_lowercase());
    out
}

/// Store `img` in the library under `name`. An identical template already there is reused
/// instead of duplicated; a different one with the same name gets "name (2)", "name (3)"…
pub fn save(dir: &Path, name: &str, img: &RgbaImage) -> image::ImageResult<PathBuf> {
    std::fs::create_dir_all(dir)?;
    for (_, p) in list(dir) {
        if image::open(&p).is_ok_and(|e| e.to_rgba8() == *img) {
            return Ok(p);
        }
    }
    let clean: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let clean = if clean.trim().is_empty() { "Template".to_owned() } else { clean.trim().to_owned() };
    let path = (1..)
        .map(|n| if n == 1 { dir.join(format!("{clean}.png")) } else { dir.join(format!("{clean} ({n}).png")) })
        .find(|p| !p.exists())
        .expect("unbounded range");
    img.save_with_format(&path, image::ImageFormat::Png)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("clothing-studio-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn save_dedupes_identical_and_numbers_name_clashes() {
        let dir = temp_dir("lib");
        let a = RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let b = RgbaImage::from_pixel(4, 4, image::Rgba([9, 9, 9, 255]));
        let p1 = save(&dir, "Shirt", &a).unwrap();
        assert_eq!(save(&dir, "Other name", &a).unwrap(), p1, "same pixels reuse the file");
        let p2 = save(&dir, "Shirt", &b).unwrap();
        assert_eq!(p2.file_name().unwrap(), "Shirt (2).png");
        let names: Vec<String> = list(&dir).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["Shirt", "Shirt (2)"]);
        assert_eq!(save(&dir, "a/b:c", &RgbaImage::new(1, 1)).unwrap().file_name().unwrap(), "a_b_c.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_lists_nothing() {
        assert!(list(&temp_dir("missing")).is_empty());
    }
}
