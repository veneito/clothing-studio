//! Roblox classic shirt and pants template layout (585 x 559) and built-in presets.
//!
//! Both garments use the same layout: the torso in the same place, and pants put the right/left
//! leg where a shirt has the right/left arm.
//!
//! Face naming is in the *character's* frame: `Left` is the character's own left (+X in our
//! scene, which faces +Z). Each region's image orientation is "as seen from outside the part,
//! image-up = world-up" for the four side faces. See docs/UV_MAPPING.md.

use image::{Rgba, RgbaImage};

pub const W: u32 = 585;
pub const H: u32 = 559;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

const fn r(x: u32, y: u32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Part {
    Torso,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
}

/// Which classic clothing item is being edited. Roblox draws the shirt over the pants on the torso.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Garment {
    Shirt,
    Pants,
}

impl Garment {
    pub const ALL: [Garment; 2] = [Garment::Shirt, Garment::Pants];

    /// Body parts this garment's template covers.
    pub fn parts(self) -> [Part; 3] {
        match self {
            Garment::Shirt => [Part::Torso, Part::RightArm, Part::LeftArm],
            Garment::Pants => [Part::Torso, Part::RightLeg, Part::LeftLeg],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Garment::Shirt => "shirt",
            Garment::Pants => "pants",
        }
    }

    pub fn preset_names(self) -> &'static [&'static str] {
        match self {
            Garment::Shirt => &PRESET_NAMES,
            Garment::Pants => &PANTS_PRESET_NAMES,
        }
    }

    /// Built-in preset `i` (out-of-range indices give the first preset).
    pub fn preset(self, i: usize) -> RgbaImage {
        match self {
            Garment::Shirt => preset_by_index(i),
            Garment::Pants => {
                let rgb = [[24, 24, 24], [245, 245, 245], [110, 112, 118]];
                pants_preset(rgb[i.min(rgb.len() - 1)])
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Face {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Torso, Part::RightArm, Part::LeftArm];
}
impl Face {
    pub const ALL: [Face; 6] = [Face::Front, Face::Back, Face::Left, Face::Right, Face::Top, Face::Bottom];
}

/// Pixel region of one cube face in the shirt template.
pub fn region(part: Part, face: Face) -> Rect {
    use Face::*;
    match part {
        // Strip left->right: Right | Front | Left | Back (continuous wrap around the torso).
        Part::Torso => match face {
            Top => r(231, 8, 128, 64),
            Right => r(165, 74, 64, 128),
            Front => r(231, 74, 128, 128),
            Left => r(361, 74, 64, 128),
            Back => r(427, 74, 128, 128),
            Bottom => r(231, 204, 128, 64),
        },
        // Strip: Left(inner) | Back | Right(outer) | Front
        Part::RightArm => match face {
            Top => r(217, 289, 64, 64),
            Left => r(19, 355, 64, 128),
            Back => r(85, 355, 64, 128),
            Right => r(151, 355, 64, 128),
            Front => r(217, 355, 64, 128),
            Bottom => r(217, 485, 64, 64),
        },
        // Legs use the arm areas of the (pants) template, same orientation rules.
        Part::RightLeg => region(Part::RightArm, face),
        Part::LeftLeg => region(Part::LeftArm, face),
        // Strip: Front | Left(outer) | Back | Right(inner)
        Part::LeftArm => match face {
            Top => r(308, 289, 64, 64),
            Front => r(308, 355, 64, 128),
            Left => r(374, 355, 64, 128),
            Back => r(440, 355, 64, 128),
            Right => r(506, 355, 64, 128),
            Bottom => r(308, 485, 64, 64),
        },
    }
}

/// Template pixel -> (part, face, face-normalized uv) for `garment`, if it lies inside a region.
pub fn locate(garment: Garment, px: f32, py: f32) -> Option<(Part, Face, [f32; 2])> {
    garment.parts().into_iter().flat_map(|p| Face::ALL.iter().map(move |&f| (p, f))).find_map(|(p, f)| {
        let r = region(p, f);
        let (u, v) = ((px - r.x as f32) / r.w as f32, (py - r.y as f32) / r.h as f32);
        ((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)).then_some((p, f, [u, v]))
    })
}

/// Rows of each arm side face covered by a short sleeve (out of 128).
pub const SHORT_SLEEVE_ROWS: u32 = 40;

/// Solid-color shirt. Short sleeves leave the lower arm transparent so skin shows through.
pub fn preset(rgb: [u8; 3], long_sleeve: bool) -> RgbaImage {
    let mut img = RgbaImage::new(W, H); // fully transparent
    let px = Rgba([rgb[0], rgb[1], rgb[2], 255]);
    for part in Part::ALL {
        for face in Face::ALL {
            let rc = region(part, face);
            let rows = match (part, face, long_sleeve) {
                (Part::Torso, _, _) | (_, _, true) | (_, Face::Top, _) => rc.h,
                (_, Face::Bottom, false) => 0,
                _ => SHORT_SLEEVE_ROWS,
            };
            for y in rc.y..rc.y + rows {
                for x in rc.x..rc.x + rc.w {
                    img.put_pixel(x, y, px);
                }
            }
        }
    }
    img
}

pub const PRESET_NAMES: [&str; 4] = ["White short sleeve", "Black short sleeve", "White long sleeve", "Black long sleeve"];
pub const PANTS_PRESET_NAMES: [&str; 3] = ["Black pants", "White pants", "Grey pants"];

/// Solid-colour pants: torso and both legs fully covered.
pub fn pants_preset(rgb: [u8; 3]) -> RgbaImage {
    let mut img = RgbaImage::new(W, H);
    let px = Rgba([rgb[0], rgb[1], rgb[2], 255]);
    for part in Garment::Pants.parts() {
        for face in Face::ALL {
            let rc = region(part, face);
            for y in rc.y..rc.y + rc.h {
                for x in rc.x..rc.x + rc.w {
                    img.put_pixel(x, y, px);
                }
            }
        }
    }
    img
}

pub fn preset_by_index(i: usize) -> RgbaImage {
    let white = [245, 245, 245];
    let black = [24, 24, 24];
    match i {
        1 => preset(black, false),
        2 => preset(white, true),
        3 => preset(black, true),
        _ => preset(white, false),
    }
}

/// Average colour of an image's outer edge (2 px ring, opaque pixels only): the background of a
/// typical photo or screenshot. None if the edge is (mostly) transparent.
pub fn border_colour(img: &RgbaImage) -> Option<[u8; 3]> {
    let (w, h) = img.dimensions();
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for y in 0..h {
        for x in 0..w {
            if x >= 2 && y >= 2 && x + 2 < w && y + 2 < h {
                continue;
            }
            let p = img.get_pixel(x, y);
            if p[3] >= 128 {
                (0..3).for_each(|c| sum[c] += p[c] as u64);
                n += 1;
            }
        }
    }
    let ring = (2 * w + 2 * h).max(1) as u64;
    (n * 2 >= ring).then(|| sum.map(|v| (v / n.max(1)) as u8)) // at least half the edge opaque
}

/// The template filled with one colour, keeping its shape and transparency (e.g. bare arms).
pub fn recolour(base: &RgbaImage, rgb: [u8; 3]) -> RgbaImage {
    let mut out = base.clone();
    for p in out.pixels_mut().filter(|p| p[3] > 0) {
        (p[0], p[1], p[2]) = (rgb[0], rgb[1], rgb[2]);
    }
    out
}

/// Studio test texture: each face a distinct color with a black square in its top-left corner,
/// so mirroring/rotation of any region is visible on a real Roblox rig.
pub fn diagnostic() -> RgbaImage {
    let colors: [[u8; 3]; 6] = [[230, 60, 60], [60, 160, 230], [80, 200, 90], [240, 200, 40], [200, 90, 220], [250, 140, 40]];
    let mut img = RgbaImage::new(W, H);
    for part in Part::ALL {
        for (i, face) in Face::ALL.into_iter().enumerate() {
            let rc = region(part, face);
            let c = colors[i];
            let m = rc.w.min(rc.h) / 4;
            for y in rc.y..rc.y + rc.h {
                for x in rc.x..rc.x + rc.w {
                    let marker = x < rc.x + m && y < rc.y + m;
                    img.put_pixel(x, y, if marker { Rgba([0, 0, 0, 255]) } else { Rgba([c[0], c[1], c[2], 255]) });
                }
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<(Part, Face, Rect)> {
        Part::ALL.iter().flat_map(|&p| Face::ALL.iter().map(move |&f| (p, f, region(p, f)))).collect()
    }

    #[test]
    fn regions_in_bounds_and_sized() {
        for (p, f, rc) in all() {
            assert!(rc.x + rc.w <= W && rc.y + rc.h <= H, "{p:?} {f:?} out of bounds");
            let expect = match (p, f) {
                (Part::Torso, Face::Front | Face::Back) => (128, 128),
                (Part::Torso, Face::Top | Face::Bottom) => (128, 64),
                (_, Face::Top | Face::Bottom) => (64, 64),
                _ => (64, 128),
            };
            assert_eq!((rc.w, rc.h), expect, "{p:?} {f:?}");
        }
    }

    #[test]
    fn regions_do_not_overlap() {
        let a = all();
        for i in 0..a.len() {
            for j in i + 1..a.len() {
                let (r1, r2) = (a[i].2, a[j].2);
                let overlap = r1.x < r2.x + r2.w && r2.x < r1.x + r1.w && r1.y < r2.y + r2.h && r2.y < r1.y + r1.h;
                assert!(!overlap, "{:?} overlaps {:?}", a[i], a[j]);
            }
        }
    }

    #[test]
    fn locate_inverts_region() {
        for (p, f, rc) in all() {
            let (part, face, uv) = locate(Garment::Shirt, rc.x as f32 + 0.25 * rc.w as f32, rc.y as f32 + 0.75 * rc.h as f32).unwrap();
            assert_eq!((part, face), (p, f));
            assert!((uv[0] - 0.25).abs() < 1e-5 && (uv[1] - 0.75).abs() < 1e-5);
        }
        assert_eq!(locate(Garment::Shirt, 2.0, 2.0), None);
        // pants: the same pixels are legs
        let ra = region(Part::RightArm, Face::Front);
        assert_eq!(locate(Garment::Pants, ra.x as f32 + 1.0, ra.y as f32 + 1.0).map(|l| l.0), Some(Part::RightLeg));
    }

    #[test]
    fn pants_use_arm_areas_for_legs_and_fill_torso() {
        for f in Face::ALL {
            assert_eq!(region(Part::RightLeg, f), region(Part::RightArm, f));
            assert_eq!(region(Part::LeftLeg, f), region(Part::LeftArm, f));
        }
        let img = Garment::Pants.preset(0);
        for part in Garment::Pants.parts() {
            let r = region(part, Face::Front);
            assert_eq!(img.get_pixel(r.x + 3, r.y + r.h - 1)[3], 255, "{part:?} fully covered");
        }
        assert_eq!(img.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn border_colour_and_recolour() {
        // grey background with a dark subject in the middle
        let img = RgbaImage::from_fn(40, 30, |x, y| if (10..30).contains(&x) && (8..22).contains(&y) { Rgba([20, 20, 20, 255]) } else { Rgba([200, 201, 202, 255]) });
        assert_eq!(border_colour(&img), Some([200, 201, 202]));
        assert_eq!(border_colour(&RgbaImage::new(40, 30)), None, "transparent edge: nothing to match");
        let base = preset([255, 255, 255], false);
        let out = recolour(&base, [200, 201, 202]);
        let f = region(Part::Torso, Face::Front);
        assert_eq!(*out.get_pixel(f.x + 5, f.y + 5), Rgba([200, 201, 202, 255]));
        let arm = region(Part::RightArm, Face::Front);
        assert_eq!(out.get_pixel(arm.x + 5, arm.y + arm.h - 1)[3], 0, "bare arm stays transparent");
    }

    #[test]
    fn short_sleeve_leaves_lower_arm_transparent() {
        let img = preset([255, 255, 255], false);
        let f = region(Part::RightArm, Face::Front);
        assert_eq!(img.get_pixel(f.x + 5, f.y + 5)[3], 255);
        assert_eq!(img.get_pixel(f.x + 5, f.y + f.h - 1)[3], 0);
        assert_eq!(img.get_pixel(0, 0)[3], 0, "outside all regions stays transparent");
        let long = preset([255, 255, 255], true);
        assert_eq!(long.get_pixel(f.x + 5, f.y + f.h - 1)[3], 255);
        assert_eq!(img.dimensions(), (W, H));
    }
}
