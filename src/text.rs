//! Text layers: render a line of text (font, fill, outline) to an RGBA image that is then placed
//! like any other artwork. Uses fonts already installed with Windows.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Everything needed to (re)render a text layer. Saved in projects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextSpec {
    pub text: String,
    /// Display name from `FONTS`.
    pub font: String,
    pub color: [u8; 3],
    pub outline_color: [u8; 3],
    /// Outline thickness as a fraction of the font size (0 = none).
    pub outline: f32,
}

impl Default for TextSpec {
    fn default() -> Self {
        TextSpec {
            text: "Text".into(),
            font: available_fonts().first().map_or_else(|| FONTS[0].0.to_owned(), |f| f.0.to_owned()),
            color: [255, 255, 255],
            outline_color: [0, 0, 0],
            outline: 0.08,
        }
    }
}

/// Fonts offered (display name, file in %WINDIR%\Fonts). Missing ones are simply not listed.
const FONTS: [(&str, &str); 12] = [
    ("Arial Black", "ariblk.ttf"),
    ("Arial Bold", "arialbd.ttf"),
    ("Impact", "impact.ttf"),
    ("Comic Sans Bold", "comicbd.ttf"),
    ("Segoe UI Bold", "segoeuib.ttf"),
    ("Bahnschrift", "bahnschrift.ttf"),
    ("Calibri Bold", "calibrib.ttf"),
    ("Verdana Bold", "verdanab.ttf"),
    ("Trebuchet Bold", "trebucbd.ttf"),
    ("Georgia Bold", "georgiab.ttf"),
    ("Times New Roman", "times.ttf"),
    ("Courier New Bold", "courbd.ttf"),
];

fn fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from).join("Fonts")
}

/// Fonts present on this PC, in menu order.
pub fn available_fonts() -> Vec<(&'static str, PathBuf)> {
    FONTS.iter().map(|(n, f)| (*n, fonts_dir().join(f))).filter(|(_, p)| p.is_file()).collect()
}

thread_local! {
    static LOADED: std::cell::RefCell<Vec<(String, std::rc::Rc<FontVec>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn load_font(name: &str) -> Result<std::rc::Rc<FontVec>, String> {
    if let Some(f) = LOADED.with_borrow(|l| l.iter().find(|(n, _)| n == name).map(|(_, f)| f.clone())) {
        return Ok(f);
    }
    let path = available_fonts().into_iter().find(|(n, _)| *n == name).map(|(_, p)| p).ok_or_else(|| format!("the font \"{name}\" isn't installed on this PC"))?;
    let bytes = std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let font = std::rc::Rc::new(FontVec::try_from_vec(bytes).map_err(|e| format!("could not load the font: {e}"))?);
    LOADED.with_borrow_mut(|l| l.push((name.to_owned(), font.clone())));
    Ok(font)
}

/// Render size in pixels. Art is shrunk onto a 128 px chest anyway; this keeps edges clean.
const SIZE: f32 = 128.0;

pub fn render(spec: &TextSpec) -> Result<RgbaImage, String> {
    let font = load_font(&spec.font)?;
    let text = if spec.text.trim().is_empty() { " " } else { spec.text.as_str() };
    let coverage = rasterize(&*font, text, spec.outline);
    Ok(paint(&coverage, spec))
}

/// Coverage (0..1) of the text, padded for the outline. Width x height row-major.
struct Coverage {
    w: u32,
    h: u32,
    fill: Vec<f32>,
}

fn rasterize(font: &FontVec, text: &str, outline: f32) -> Coverage {
    let scaled = font.as_scaled(PxScale::from(SIZE));
    let pad = (outline.clamp(0.0, 0.5) * SIZE).ceil() as u32 + 2;
    // Lay out one line, with kerning.
    let mut x = 0.0f32;
    let mut prev = None;
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            x += scaled.kern(p, id);
        }
        glyphs.push(id.with_scale_and_position(PxScale::from(SIZE), ab_glyph::point(x, scaled.ascent())));
        x += scaled.h_advance(id);
        prev = Some(id);
    }
    let w = (x.ceil() as u32).max(1) + 2 * pad;
    let h = ((scaled.ascent() - scaled.descent()).ceil() as u32).max(1) + 2 * pad;
    let mut fill = vec![0f32; (w * h) as usize];
    for g in glyphs {
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|gx, gy, c| {
                let (px, py) = (b.min.x as i64 + gx as i64 + pad as i64, b.min.y as i64 + gy as i64 + pad as i64);
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    let i = (py as u32 * w + px as u32) as usize;
                    fill[i] = (fill[i] + c).min(1.0);
                }
            });
        }
    }
    Coverage { w, h, fill }
}

/// Grow coverage by `r` px: repeated 3x3 max, alternating square/cross steps (round-ish corners).
fn dilate(src: &[f32], w: u32, h: u32, r: u32) -> Vec<f32> {
    let mut cur = src.to_vec();
    for step in 0..r {
        let square = step % 2 == 0;
        let mut next = cur.clone();
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let mut m = cur[(y * w as i64 + x) as usize];
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                    if !square && dx != 0 && dy != 0 {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64 {
                        m = m.max(cur[(ny * w as i64 + nx) as usize]);
                    }
                }
                next[(y * w as i64 + x) as usize] = m;
            }
        }
        cur = next;
    }
    cur
}

fn paint(c: &Coverage, spec: &TextSpec) -> RgbaImage {
    let r = (spec.outline.clamp(0.0, 0.5) * SIZE).round() as u32;
    let outline = if r > 0 { dilate(&c.fill, c.w, c.h, r) } else { vec![0.0; c.fill.len()] };
    RgbaImage::from_fn(c.w, c.h, |x, y| {
        let i = (y * c.w + x) as usize;
        let (f, o) = (c.fill[i], outline[i]);
        // fill over outline (straight alpha "over")
        let a = f + o * (1.0 - f);
        if a <= 0.0 {
            return Rgba([0, 0, 0, 0]);
        }
        let mix = |fc: u8, oc: u8| ((fc as f32 * f + oc as f32 * o * (1.0 - f)) / a).round() as u8;
        Rgba([
            mix(spec.color[0], spec.outline_color[0]),
            mix(spec.color[1], spec.outline_color[1]),
            mix(spec.color[2], spec.outline_color[2]),
            (a * 255.0).round() as u8,
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str, outline: f32) -> Option<TextSpec> {
        // Tests need at least one installed font (always true on Windows).
        available_fonts().first().map(|f| TextSpec { text: text.into(), font: f.0.into(), outline, ..TextSpec::default() })
    }

    #[test]
    fn renders_fill_and_outline_colours() {
        let Some(s) = spec("H", 0.1) else { return };
        let img = render(&TextSpec { color: [255, 0, 0], outline_color: [0, 0, 255], ..s }).unwrap();
        let mut red = 0;
        let mut blue = 0;
        for p in img.pixels().filter(|p| p[3] == 255) {
            if p[0] > 200 && p[2] < 50 { red += 1 }
            if p[2] > 200 && p[0] < 50 { blue += 1 }
        }
        assert!(red > 100 && blue > 100, "fill {red} px, outline {blue} px");
        assert_eq!(img.get_pixel(0, 0)[3], 0, "padding is transparent");
    }

    #[test]
    fn longer_text_is_wider_and_outline_adds_size() {
        let Some(s) = spec("Hi", 0.0) else { return };
        let short = render(&s).unwrap();
        let long = render(&TextSpec { text: "Hello world".into(), ..s.clone() }).unwrap();
        assert!(long.width() > short.width() * 2);
        let outlined = render(&TextSpec { outline: 0.2, ..s }).unwrap();
        assert!(outlined.width() > short.width() && outlined.height() > short.height());
    }

    #[test]
    fn unknown_font_is_an_error_not_a_panic() {
        let s = TextSpec { font: "No Such Font".into(), ..TextSpec::default() };
        assert!(render(&s).is_err());
    }
}
