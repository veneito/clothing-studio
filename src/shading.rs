//! Shading overlays: soft shadows/highlights drawn over the finished clothing so it doesn't look
//! flat. Presets are generated here (no third-party art); users can also import their own
//! 585x559 overlay PNG. Black pixels darken, white pixels lighten, alpha = amount.

use crate::template::{self, Face, Garment, Part};
use image::{Rgba, RgbaImage};

pub const PRESETS: [&str; 3] = ["Soft", "Wrinkles", "Strong"];

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Soft bump: 1 at `c`, fading to 0 at distance `w`.
fn band(x: f32, c: f32, w: f32) -> f32 {
    let d = ((x - c) / w).abs();
    if d >= 1.0 { 0.0 } else { (1.0 - d * d).powi(2) }
}

/// Shade at face position (u, v) in 0..1: negative darkens, positive lightens (-1..1).
fn shade(preset: usize, part: Part, face: Face, u: f32, v: f32) -> f32 {
    let (folds, strength) = match preset {
        0 => (0.0, 1.0),
        1 => (1.0, 1.0),
        _ => (1.3, 1.6),
    };
    let side = matches!(face, Face::Front | Face::Back | Face::Left | Face::Right);
    if !side {
        // tops/bottoms: just darker toward their edges
        let e = 1.0 - smoothstep(0.0, 0.35, u.min(1.0 - u).min(v).min(1.0 - v));
        return -0.25 * e * strength;
    }
    // Rounded-body look: darker toward the vertical edges of each side face.
    let edge = 1.0 - smoothstep(0.0, 0.3, u.min(1.0 - u));
    let mut s = -0.45 * edge * edge;
    match part {
        Part::Torso => {
            s -= 0.18 * smoothstep(0.82, 1.0, v); // hem
            if matches!(face, Face::Front | Face::Back) {
                // diagonal folds from the armpits toward the waist
                let d1 = band(u - 0.15 - 0.35 * v, 0.0, 0.06);
                let d2 = band(u - 0.85 + 0.35 * v, 0.0, 0.06);
                s -= folds * 0.3 * (d1 + d2) * smoothstep(0.15, 0.4, v) * (1.0 - smoothstep(0.75, 0.95, v));
                // light across the chest
                s += 0.12 * (strength - 1.0) / 0.6 * band(u, 0.5, 0.35) * band(v, 0.3, 0.3);
            }
        }
        Part::RightArm | Part::LeftArm | Part::RightLeg | Part::LeftLeg => {
            // creases at the elbow / knee, curved a little around the limb
            let joint = if matches!(part, Part::RightLeg | Part::LeftLeg) { 0.5 } else { 0.55 };
            let curve = 0.06 * (u - 0.5).powi(2) * 4.0;
            for (k, o) in [(1.0, 0.0), (0.7, -0.08), (0.7, 0.08)] {
                s -= folds * 0.35 * k * band(v - curve, joint + o, 0.035);
            }
            s -= 0.15 * smoothstep(0.85, 1.0, v); // cuff
        }
    }
    (s * strength).clamp(-1.0, 1.0)
}

/// Generate preset `preset` for a garment: shading only inside that garment's regions.
pub fn preset(garment: Garment, preset: usize) -> RgbaImage {
    let mut img = RgbaImage::new(template::W, template::H);
    for part in garment.parts() {
        for face in Face::ALL {
            let r = template::region(part, face);
            for y in r.y..r.y + r.h {
                for x in r.x..r.x + r.w {
                    let (u, v) = ((x - r.x) as f32 / (r.w - 1) as f32, (y - r.y) as f32 / (r.h - 1) as f32);
                    let s = shade(preset, part, face, u, v);
                    let a = (s.abs() * 255.0).round() as u8;
                    img.put_pixel(x, y, if s < 0.0 { Rgba([0, 0, 0, a]) } else { Rgba([255, 255, 255, a]) });
                }
            }
        }
    }
    img
}

/// Mix `overlay` into `out` at `strength` (0..1), only where there is clothing (alpha kept), so
/// transparent areas (bare arms) don't get shadows.
pub fn apply(out: &mut RgbaImage, overlay: &RgbaImage, strength: f32) {
    if overlay.dimensions() != out.dimensions() {
        return;
    }
    let k = strength.clamp(0.0, 1.0);
    for (d, s) in out.pixels_mut().zip(overlay.pixels()) {
        if d[3] == 0 || s[3] == 0 {
            continue;
        }
        let t = s[3] as f32 / 255.0 * k;
        for c in 0..3 {
            d[c] = (d[c] as f32 * (1.0 - t) + s[c] as f32 * t).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_shade_only_inside_the_garment() {
        for g in Garment::ALL {
            for i in 0..PRESETS.len() {
                let img = preset(g, i);
                assert_eq!(img.get_pixel(0, 0)[3], 0, "outside regions untouched");
                let f = template::region(Part::Torso, Face::Front);
                let edge = img.get_pixel(f.x + 1, f.y + 60);
                let mid = img.get_pixel(f.x + 64, f.y + 10);
                assert!(edge[0] == 0 && edge[3] > mid[3].saturating_add(20), "{g:?} {i}: edges darker than centre");
            }
        }
    }

    #[test]
    fn wrinkles_add_creases_soft_does_not() {
        let r = template::region(Part::RightArm, Face::Front);
        let (x, y) = (r.x + r.w / 2, r.y + (0.55 * (r.h - 1) as f32) as u32);
        let soft = preset(Garment::Shirt, 0).get_pixel(x, y)[3];
        let wrinkles = preset(Garment::Shirt, 1).get_pixel(x, y)[3];
        assert!(wrinkles > soft + 40, "crease at the elbow: soft {soft}, wrinkles {wrinkles}");
    }

    #[test]
    fn apply_skips_transparent_clothing_and_scales_with_strength() {
        let mut out = RgbaImage::from_fn(2, 1, |x, _| if x == 0 { Rgba([200, 200, 200, 255]) } else { Rgba([200, 200, 200, 0]) });
        let ov = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 255]));
        let mut half = out.clone();
        apply(&mut half, &ov, 0.5);
        assert_eq!(*half.get_pixel(0, 0), Rgba([100, 100, 100, 255]));
        assert_eq!(*half.get_pixel(1, 0), Rgba([200, 200, 200, 0]), "bare skin stays clean");
        apply(&mut out, &ov, 0.0);
        assert_eq!(*out.get_pixel(0, 0), Rgba([200, 200, 200, 255]), "strength 0 = no change");
    }
}
