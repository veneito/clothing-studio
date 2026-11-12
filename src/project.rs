//! Project files: one JSON file holding the base template, every layer's settings and the
//! original artwork (lossless PNG, base64). Images shared by several layers are stored once.

use crate::compose::{Document, Layer, Overlay, Quality};
use crate::template::{self, Face, Part};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::rc::Rc;

/// Project files are JSON inside, but use their own extension so Windows can open them with the
/// app (associating ".json" itself would take over every JSON file).
pub const EXTENSION: &str = "clothing";
/// Extension used before 2026-11-12; still opened.
pub const LEGACY_EXTENSION: &str = "clothing.json";

/// Whether a file name looks like a project (current or legacy extension).
pub fn is_project_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(&format!(".{EXTENSION}")) || n.ends_with(&format!(".{LEGACY_EXTENSION}"))
}

/// "My shirt", "My shirt.json", "My shirt.clothing.json" -> "My shirt"
pub fn strip_extension(name: &str) -> &str {
    for ext in [LEGACY_EXTENSION, EXTENSION, "json"] {
        let suffix = format!(".{ext}");
        if name.len() > suffix.len() && name.to_ascii_lowercase().ends_with(&suffix) {
            return &name[..name.len() - suffix.len()];
        }
    }
    name
}
const FORMAT: &str = "clothing-studio-project";
/// 2 added `pants`; version-1 files (shirt only) still load.
const VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: String,
    version: u32,
    /// Dropdown entry the base came from, if any (informational; the pixels are in `base`).
    #[serde(default)]
    template_name: Option<String>,
    /// Index into `images`.
    base: usize,
    layers: Vec<LayerFile>,
    /// Base64 PNGs.
    images: Vec<String>,
    /// The pants (the top-level `template_name`/`base`/`layers` are the shirt, as in version 1).
    #[serde(default)]
    pants: Option<GarmentFile>,
    /// The shirt's shading overlay.
    #[serde(default)]
    overlay: Option<OverlayFile>,
    /// The shirt's tattoo underlay.
    #[serde(default)]
    underlay: Option<OverlayFile>,
}

#[derive(Serialize, Deserialize)]
struct OverlayFile {
    image: usize,
    strength: f32,
    #[serde(default)]
    preset: Option<usize>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    ink: bool,
}

#[derive(Serialize, Deserialize)]
struct GarmentFile {
    #[serde(default)]
    template_name: Option<String>,
    base: usize,
    layers: Vec<LayerFile>,
    #[serde(default)]
    overlay: Option<OverlayFile>,
    #[serde(default)]
    underlay: Option<OverlayFile>,
}

/// A garment's document and the dropdown name of its template (if any).
pub type GarmentDoc = (Document, Option<String>);

#[derive(Serialize, Deserialize)]
struct LayerFile {
    name: String,
    image: usize,
    visible: bool,
    wrap: bool,
    part: Part,
    face: Face,
    center: [f32; 2],
    width: f32,
    rotation: f32,
    flip_h: bool,
    flip_v: bool,
    opacity: f32,
    #[serde(default)]
    grayscale: bool,
    #[serde(default)]
    brightness: f32,
    #[serde(default)]
    contrast: f32,
    /// Index into `images` of the background-removal mask, if one was computed.
    #[serde(default)]
    mask: Option<usize>,
    #[serde(default)]
    remove_bg: bool,
    /// Missing in projects saved before quality modes existed: they were drawn Smooth.
    #[serde(default = "smooth")]
    quality: Quality,
    #[serde(default)]
    invert: bool,
    /// Text layers: their settings (the rendered text is the layer's image).
    #[serde(default)]
    text: Option<crate::text::TextSpec>,
    /// Index into `images` of the AI-upscaled copy, if made.
    #[serde(default)]
    upscaled: Option<usize>,
    #[serde(default)]
    upscale: bool,
    /// "Across front" garment, if on.
    #[serde(default)]
    across: Option<template::Garment>,
    #[serde(default = "one")]
    stretch: f32,
}

fn one() -> f32 {
    1.0
}

fn smooth() -> Quality {
    Quality::Smooth
}

/// Encoded PNGs by image, so repeated saves (and auto-saves) don't re-encode big artwork.
#[derive(Default)]
pub struct PngCache(Vec<(Rc<RgbaImage>, String)>);

impl PngCache {
    fn encode(&mut self, img: &Rc<RgbaImage>) -> Result<String, String> {
        if let Some((_, s)) = self.0.iter().find(|(i, _)| Rc::ptr_eq(i, img)) {
            return Ok(s.clone());
        }
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        let s = B64.encode(bytes);
        // Drop entries nothing else holds any more (deleted layers no longer in the undo history).
        self.0.retain(|(i, _)| Rc::strong_count(i) > 1);
        self.0.push((img.clone(), s.clone()));
        Ok(s)
    }
}

pub fn to_json(shirt: (&Document, Option<&str>), pants: (&Document, Option<&str>), cache: &mut PngCache) -> Result<String, String> {
    let mut uniq: Vec<Rc<RgbaImage>> = Vec::new();
    let mut id = |img: &Rc<RgbaImage>| match uniq.iter().position(|u| Rc::ptr_eq(u, img)) {
        Some(i) => i,
        None => {
            uniq.push(img.clone());
            uniq.len() - 1
        }
    };
    let mut garment = |doc: &Document| -> (usize, Vec<LayerFile>, Option<OverlayFile>, Option<OverlayFile>) {
        let base = id(&doc.base);
        let layers = doc
            .layers
            .iter()
            .map(|l| LayerFile {
                name: l.name.clone(),
                image: id(&l.source),
                visible: l.visible,
                wrap: l.wrap,
                part: l.part,
                face: l.face,
                center: l.center,
                width: l.width,
                rotation: l.rotation,
                flip_h: l.flip_h,
                flip_v: l.flip_v,
                opacity: l.opacity,
                grayscale: l.grayscale,
                brightness: l.brightness,
                contrast: l.contrast,
                mask: l.mask.as_ref().map(&mut id),
                remove_bg: l.remove_bg,
                quality: l.quality,
                invert: l.invert,
                text: l.text.clone(),
                upscaled: l.upscaled.as_ref().map(&mut id),
                upscale: l.upscale,
                across: l.across,
                stretch: l.stretch,
            })
            .collect();
        let mut file = |o: &Overlay| OverlayFile { image: id(&o.image), strength: o.strength, preset: o.preset, name: o.name.clone(), ink: o.ink };
        let overlay = doc.overlay.as_ref().map(&mut file);
        let underlay = doc.underlay.as_ref().map(&mut file);
        (base, layers, overlay, underlay)
    };
    let (base, layers, overlay, underlay) = garment(shirt.0);
    let (pants_base, pants_layers, pants_overlay, pants_underlay) = garment(pants.0);
    let images = uniq.iter().map(|i| cache.encode(i)).collect::<Result<_, _>>()?;
    let file = ProjectFile {
        format: FORMAT.into(),
        version: VERSION,
        template_name: shirt.1.map(Into::into),
        base,
        layers,
        images,
        pants: Some(GarmentFile { template_name: pants.1.map(Into::into), base: pants_base, layers: pants_layers, overlay: pants_overlay, underlay: pants_underlay }),
        overlay,
        underlay,
    };
    serde_json::to_string(&file).map_err(|e| e.to_string())
}

/// Parse a project. Everything from the file is validated: it may come from anywhere.
/// Returns the shirt and, for version-2 files, the pants.
pub fn from_json(text: &str) -> Result<(GarmentDoc, Option<GarmentDoc>), String> {
    let f: ProjectFile = serde_json::from_str(text).map_err(|e| format!("not a valid project file ({e})"))?;
    if f.format != FORMAT {
        return Err("not a Clothing Studio project".into());
    }
    if f.version > VERSION {
        return Err(format!("made by a newer version of Clothing Studio (format {})", f.version));
    }
    let images: Vec<Rc<RgbaImage>> = f
        .images
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let bytes = B64.decode(s).map_err(|e| format!("image {i} is damaged ({e})"))?;
            let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map_err(|e| format!("image {i} is damaged ({e})"))?;
            Ok(Rc::new(img.to_rgba8()))
        })
        .collect::<Result<_, String>>()?;
    let get = |i: usize| images.get(i).cloned().ok_or_else(|| format!("missing image {i}"));
    let finite = |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
    let overlay_from = |o: Option<OverlayFile>, what: &str| -> Result<Option<Overlay>, String> {
        let Some(o) = o else { return Ok(None) };
        let image = get(o.image)?;
        if image.dimensions() != (template::W, template::H) {
            return Err(format!("its {what} isn't 585×559"));
        }
        let preset = o.preset.filter(|&p| p < crate::shading::PRESETS.len());
        Ok(Some(Overlay { image, strength: finite(o.strength, 0.0, 1.0, 0.5), preset, name: o.name, ink: o.ink }))
    };
    let garment = |base: usize, layers: Vec<LayerFile>, overlay: Option<OverlayFile>, underlay: Option<OverlayFile>| -> Result<Document, String> {
    let base = get(base)?;
    if base.dimensions() != (template::W, template::H) {
        return Err(format!("its template is {}×{}, not 585×559", base.width(), base.height()));
    }
    let layers = layers
        .into_iter()
        .map(|lf| {
            let source = get(lf.image)?;
            if source.width() == 0 || source.height() == 0 {
                return Err(format!("layer \"{}\" has an empty image", lf.name));
            }
            let fresh = Layer::new(String::new(), RgbaImage::new(1, 1));
            let mask = lf.mask.map(get).transpose()?.filter(|m| m.dimensions() == source.dimensions());
            let upscaled = lf.upscaled.map(get).transpose()?.filter(|u| u.dimensions() == (source.width() * 4, source.height() * 4));
            let mut layer = Layer {
                name: lf.name,
                work: source.clone(), // rebuilt below
                source,
                mask,
                remove_bg: lf.remove_bg,
                quality: lf.quality,
                invert: lf.invert,
                text: lf.text,
                upscale: lf.upscale && upscaled.is_some(),
                across: lf.across,
                stretch: finite(lf.stretch, 0.05, 20.0, 1.0),
                upscaled,
                visible: lf.visible,
                wrap: lf.wrap,
                part: lf.part,
                face: lf.face,
                center: [finite(lf.center[0], -2.0, 3.0, 0.5), finite(lf.center[1], -2.0, 3.0, 0.5)],
                width: finite(lf.width, 0.01, 10.0, fresh.width),
                rotation: finite(lf.rotation, -360.0, 360.0, 0.0),
                flip_h: lf.flip_h,
                flip_v: lf.flip_v,
                opacity: finite(lf.opacity, 0.0, 1.0, 1.0),
                grayscale: lf.grayscale,
                brightness: finite(lf.brightness, -1.0, 1.0, 0.0),
                contrast: finite(lf.contrast, -1.0, 1.0, 0.0),
            };
            layer.rebuild_work();
            Ok(layer)
        })
        .collect::<Result<_, String>>()?;
    Ok(Document { base, layers, overlay: overlay_from(overlay, "shading overlay")?, underlay: overlay_from(underlay, "tattoo")? })
    };
    let shirt = (garment(f.base, f.layers, f.overlay, f.underlay)?, f.template_name);
    let pants = f.pants.map(|g| Ok::<_, String>((garment(g.base, g.layers, g.overlay, g.underlay)?, g.template_name))).transpose()?;
    Ok((shirt, pants))
}

/// Write via a temporary file and rename, so a crash mid-save never leaves a half-written project.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pants() -> Document {
        let art = RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let mut l = Layer::new("knee patch".into(), art);
        l.part = Part::RightLeg;
        Document { base: Rc::new(template::Garment::Pants.preset(2)), layers: vec![l], overlay: None, underlay: None }
    }

    fn sample() -> Document {
        let art = RgbaImage::from_fn(10, 6, |x, y| image::Rgba([x as u8 * 20, y as u8 * 30, 7, (x * 25) as u8]));
        let mut a = Layer::new("logo".into(), art);
        a.part = Part::LeftArm;
        a.face = Face::Back;
        a.center = [0.3, 0.7];
        a.rotation = 45.0;
        a.flip_v = true;
        a.opacity = 0.5;
        a.grayscale = true;
        a.brightness = 0.25;
        a.contrast = -0.5;
        a.wrap = false;
        a.mask = Some(Rc::new(RgbaImage::from_fn(10, 6, |x, _| image::Rgba([255, 255, 255, if x < 5 { 255 } else { 0 }]))));
        a.remove_bg = true;
        a.quality = Quality::Pixel;
        a.rebuild_work();
        a.text = Some(crate::text::TextSpec { text: "Hi".into(), ..crate::text::TextSpec::default() });
        a.upscaled = Some(Rc::new(RgbaImage::from_pixel(40, 24, image::Rgba([9, 8, 7, 255]))));
        a.upscale = true;
        a.rebuild_work();
        let mut b = a.clone(); // duplicate shares the image
        b.name = "logo copy".into();
        b.visible = false;
        let shade = Rc::new(crate::shading::preset(template::Garment::Shirt, 1));
        Document { base: Rc::new(template::preset([10, 20, 30], true)), layers: vec![a, b], overlay: Some(Overlay { image: shade, strength: 0.4, preset: Some(1), name: "Wrinkles".into(), ink: false }), underlay: Some(Overlay { image: Rc::new(RgbaImage::from_pixel(template::W, template::H, image::Rgba([5, 5, 5, 200]))), strength: 0.7, preset: None, name: "Sleeve".into(), ink: true }) }
    }

    #[test]
    fn round_trip_keeps_everything_and_shares_images() {
        let doc = sample();
        let json = to_json((&doc, Some("My shirt")), (&pants(), Some("Grey pants")), &mut PngCache::default()).unwrap();
        let ((back, name), back_pants) = from_json(&json).unwrap();
        let (back_pants, pants_name) = back_pants.expect("pants saved");
        assert_eq!(pants_name.as_deref(), Some("Grey pants"));
        assert_eq!(back_pants.layers.len(), 1);
        assert_eq!(back_pants.layers[0].part, Part::RightLeg);
        assert_eq!(*back_pants.base, *pants().base);
        assert_eq!(name.as_deref(), Some("My shirt"));
        assert_eq!(*back.base, *doc.base);
        assert_eq!(back.layers.len(), 2);
        for (x, y) in back.layers.iter().zip(&doc.layers) {
            assert_eq!(*x.source, *y.source, "pixels incl. alpha preserved");
            assert_eq!((x.part, x.face, x.center, x.width, x.rotation), (y.part, y.face, y.center, y.width, y.rotation));
            assert_eq!((x.flip_h, x.flip_v, x.visible, x.wrap, x.grayscale), (y.flip_h, y.flip_v, y.visible, y.wrap, y.grayscale));
            assert_eq!((x.opacity, x.brightness, x.contrast, &x.name), (y.opacity, y.brightness, y.contrast, &y.name));
        }
        assert!(Rc::ptr_eq(&back.layers[0].source, &back.layers[1].source), "shared image loaded once");
        assert_eq!(crate::compose::compose(&back), crate::compose::compose(&doc), "renders identically");
        let f: ProjectFile = serde_json::from_str(&json).unwrap();
        assert_eq!(f.images.len(), 8, "shirt base + shared artwork + its mask + upscaled copy + shading + tattoo + pants base + pants art");
        assert!(back.layers[0].upscale && back.layers[0].upscaled.as_ref().is_some_and(|u| u.dimensions() == (40, 24)), "upscale restored");
        let u = back.underlay.as_ref().expect("tattoo saved");
        assert_eq!((u.strength, u.ink, u.name.as_str()), (0.7, true, "Sleeve"));
        let o = back.overlay.as_ref().expect("shading saved");
        assert_eq!((o.strength, o.preset, o.name.as_str()), (0.4, Some(1), "Wrinkles"));
        assert_eq!(back.layers[0].text.as_ref().map(|t| t.text.as_str()), Some("Hi"));
        assert!(back.layers[0].remove_bg && back.layers[0].mask.is_some(), "mask restored");
        assert_eq!(back.layers[0].quality, Quality::Pixel);
        // a project from before quality modes loads as Smooth (how it looked when saved)
        let old = json.replace(r#","quality":"Pixel""#, "");
        assert_eq!(from_json(&old).unwrap().0.0.layers[0].quality, Quality::Smooth);
        // a version-1 file (shirt only, no pants) still loads
        let mut v1: serde_json::Value = serde_json::from_str(&json).unwrap();
        v1.as_object_mut().unwrap().remove("pants");
        v1["version"] = 1.into();
        assert!(from_json(&v1.to_string()).unwrap().1.is_none());
    }

    #[test]
    fn rejects_bad_files() {
        assert!(from_json("hello").is_err());
        assert!(from_json(r#"{"format":"other","version":1,"base":0,"layers":[],"images":[]}"#).is_err());
        let newer = format!(r#"{{"format":"{FORMAT}","version":99,"base":0,"layers":[],"images":[]}}"#);
        assert!(from_json(&newer).err().unwrap().contains("newer"));
        let missing = format!(r#"{{"format":"{FORMAT}","version":1,"base":3,"layers":[],"images":[]}}"#);
        assert!(from_json(&missing).is_err());
        let bad_b64 = format!(r#"{{"format":"{FORMAT}","version":1,"base":0,"layers":[],"images":["!!"]}}"#);
        assert!(from_json(&bad_b64).err().unwrap().contains("damaged"));
    }

    /// Writes a sample project to $CS_SAMPLE_PROJECT (manual end-to-end check).
    #[test]
    #[ignore]
    fn write_sample_project() {
        let Some(path) = std::env::var_os("CS_SAMPLE_PROJECT") else { return };
        let json = to_json((&sample(), Some("Black long sleeve")), (&pants(), None), &mut PngCache::default()).unwrap();
        write_atomic(Path::new(&path), &json).unwrap();
    }

    #[test]
    fn extensions() {
        assert!(is_project_file("a.clothing") && is_project_file("A.Clothing.JSON") && !is_project_file("a.json"));
        assert_eq!(strip_extension("My shirt.clothing.json"), "My shirt");
        assert_eq!(strip_extension("My shirt.clothing"), "My shirt");
        assert_eq!(strip_extension("My shirt.json"), "My shirt");
        assert_eq!(strip_extension("My shirt"), "My shirt");
    }

    #[test]
    fn cache_reuses_encodings() {
        let doc = sample();
        let mut cache = PngCache::default();
        let a = to_json((&doc, None), (&doc, None), &mut cache).unwrap();
        assert_eq!(cache.0.len(), 6);
        let b = to_json((&doc, None), (&doc, None), &mut cache).unwrap();
        assert_eq!(a, b);
        assert_eq!(cache.0.len(), 6, "no re-encoding");
    }
}
