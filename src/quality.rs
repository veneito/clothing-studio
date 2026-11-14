//! Image-quality harness (audit milestone M1). Synthetic fixtures with known ideal results, so
//! rendering changes can be measured instead of eyeballed.
//!
//! - Invariant tests (dimensions, alignment, PNG round-trip, repeated edits) always run.
//! - `quality_baseline` (ignored) prints the metric table; with $CS_QUALITY_DIR set it also writes
//!   4x enlarged crops of each fixture for before/after comparison:
//!   `cargo test quality_baseline -- --ignored --nocapture`

use crate::compose::{compose, Document, Layer, Quality};
use crate::template::{self, Face, Part};
use image::{imageops, Rgba, RgbaImage};
use std::rc::Rc;

const RED: Rgba<u8> = Rgba([230, 30, 30, 255]);

fn to_lin(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(l: f32) -> f32 {
    let l = l.clamp(0.0, 1.0);
    255.0 * if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
}

fn doc(base: [u8; 3], layer: Layer) -> Document {
    Document { base: Rc::new(template::preset(base, true)), layers: vec![layer], overlay: None, underlay: None }
}

fn front() -> template::Rect {
    template::region(Part::Torso, Face::Front)
}

// ------------------------------------------------------------------ fixtures

/// Opaque red square, rotated 30°, on white. Ideal = exact area coverage mixed in linear light.
fn rotated(q: Quality) -> (RgbaImage, RgbaImage) {
    rotated_with(q, true)
}

/// `linear`: false mixes coverage in sRGB, like `blend` does today (isolates geometry from gamma).
fn rotated_with(q: Quality, linear: bool) -> (RgbaImage, RgbaImage) {
    let l = Layer { quality: q, width: 0.6, rotation: 30.0, ..Layer::new("rot".into(), RgbaImage::from_pixel(512, 512, RED)) };
    let (w, h) = l.size_px();
    let (cx, cy) = l.center_px();
    let (s, c) = l.rotation.to_radians().sin_cos();
    let out = compose(&doc([255, 255, 255], l));
    let f = front();
    let ideal = RgbaImage::from_fn(f.w, f.h, |x, y| {
        const N: u32 = 16;
        let mut inside = 0;
        for j in 0..N {
            for i in 0..N {
                let (px, py) = ((f.x + x) as f32 + (i as f32 + 0.5) / N as f32, (f.y + y) as f32 + (j as f32 + 0.5) / N as f32);
                let (dx, dy) = (px - cx, py - cy);
                let (lx, ly) = (dx * c + dy * s, -dx * s + dy * c);
                inside += (lx.abs() < w as f32 / 2.0 && ly.abs() < h as f32 / 2.0) as u32;
            }
        }
        let k = inside as f32 / (N * N) as f32;
        let mix = |a: u8| if linear { to_srgb(k * to_lin(a) + (1.0 - k) * 1.0).round() as u8 } else { (k * a as f32 + (1.0 - k) * 255.0).round() as u8 };
        Rgba([mix(RED[0]), mix(RED[1]), mix(RED[2]), 255])
    });
    (imageops::crop_imm(&out, f.x, f.y, f.w, f.h).to_image(), ideal)
}

/// 1024 px black square with 4 px white lines every 64 px, drawn 128 px wide (8x down, lines 0.5 px).
fn thin_lines(q: Quality) -> RgbaImage {
    let src = RgbaImage::from_fn(1024, 1024, |x, _| if x % 64 < 4 { Rgba([255, 255, 255, 255]) } else { Rgba([0, 0, 0, 255]) });
    let out = compose(&doc([0, 0, 0], Layer { quality: q, width: 1.0, ..Layer::new("lines".into(), src) }));
    let f = front();
    imageops::crop_imm(&out, f.x, f.y, f.w, f.h).to_image()
}

/// 2048 px of 1 px black/white columns drawn ~77 px wide (~27x down). Ideal: flat linear 50% grey.
fn big_downscale(q: Quality) -> RgbaImage {
    let src = RgbaImage::from_fn(2048, 2048, |x, _| if x % 2 == 0 { Rgba([255, 255, 255, 255]) } else { Rgba([0, 0, 0, 255]) });
    let l = Layer { quality: q, width: 0.6, ..Layer::new("fine".into(), src) };
    let (w, h) = l.size_px();
    let (cx, cy) = l.center_px();
    let out = compose(&doc([0, 0, 0], l));
    // inner box, 2 px away from the art's border
    imageops::crop_imm(&out, (cx - w as f32 / 2.0) as u32 + 2, (cy - h as f32 / 2.0) as u32 + 2, w - 4, h - 4).to_image()
}

/// Constant-colour red disc whose alpha fades to 0 over 40 px, on `base`.
fn soft_edge(q: Quality, base: [u8; 3]) -> RgbaImage {
    let src = RgbaImage::from_fn(256, 256, |x, y| {
        let d = ((x as f32 - 127.5).powi(2) + (y as f32 - 127.5).powi(2)).sqrt();
        let a = ((120.0 - d) / 40.0).clamp(0.0, 1.0);
        Rgba([RED[0], RED[1], RED[2], (a * 255.0).round() as u8])
    });
    let out = compose(&doc(base, Layer { quality: q, width: 0.9, ..Layer::new("soft".into(), src) }));
    let f = front();
    imageops::crop_imm(&out, f.x, f.y, f.w, f.h).to_image()
}

// ------------------------------------------------------------------ metrics

/// PSNR (dB) over RGB, and mean absolute error on pixels where the ideal is a partial mix (edges).
fn psnr_and_edge_mae(out: &RgbaImage, ideal: &RgbaImage) -> (f32, f32) {
    let (mut se, mut n, mut edge, mut ne) = (0f64, 0u64, 0f64, 0u64);
    for (o, i) in out.pixels().zip(ideal.pixels()) {
        let partial = i[1] != 255 && i[1] != RED[1]; // neither pure white nor pure red
        for c in 0..3 {
            let d = o[c] as f64 - i[c] as f64;
            se += d * d;
            n += 1;
            if partial {
                edge += d.abs();
                ne += 1;
            }
        }
    }
    let mse = se / n as f64;
    ((10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()) as f32, (edge / ne.max(1) as f64) as f32)
}

/// Mean linear-light value of the red channel (the art is grey, so one channel is enough).
fn mean_lin(img: &RgbaImage) -> f32 {
    img.pixels().map(|p| to_lin(p[0])).sum::<f32>() / img.pixels().len() as f32
}

/// Std-dev of sRGB values: moiré/aliasing on a fixture whose ideal is flat.
fn std_srgb(img: &RgbaImage) -> f32 {
    let n = img.pixels().len() as f32;
    let m = img.pixels().map(|p| p[0] as f32).sum::<f32>() / n;
    (img.pixels().map(|p| (p[0] as f32 - m).powi(2)).sum::<f32>() / n).sqrt()
}

/// Fringe: distance (0..255) of each pixel from the straight line base -> art colour, i.e. how far
/// an edge pixel strays from a pure mix. Returns (max, mean over partially covered pixels).
fn fringe(img: &RgbaImage, base: [u8; 3]) -> (f32, f32) {
    let b = base.map(|v| v as f32);
    let d = [RED[0] as f32 - b[0], RED[1] as f32 - b[1], RED[2] as f32 - b[2]];
    let dd = d.iter().map(|v| v * v).sum::<f32>();
    let (mut max, mut sum, mut n) = (0f32, 0f32, 0u32);
    for p in img.pixels() {
        let v = [p[0] as f32 - b[0], p[1] as f32 - b[1], p[2] as f32 - b[2]];
        let t = (v.iter().zip(&d).map(|(a, b)| a * b).sum::<f32>() / dd).clamp(0.0, 1.0);
        if t <= 0.0 || t >= 1.0 {
            continue;
        }
        let dist = (0..3).map(|c| (v[c] - d[c] * t).powi(2)).sum::<f32>().sqrt();
        max = max.max(dist);
        sum += dist;
        n += 1;
    }
    (max, sum / n.max(1) as f32)
}

// ------------------------------------------------------------------ invariants (always run)

#[test]
fn export_is_template_size_and_full_face_art_aligns_exactly() {
    let art = RgbaImage::from_pixel(64, 64, RED);
    let out = compose(&doc([255, 255, 255], Layer { quality: Quality::Pixel, width: 1.0, wrap: false, ..Layer::new("a".into(), art) }));
    assert_eq!(out.dimensions(), (585, 559));
    let f = front();
    for y in f.y..f.y + f.h {
        for x in f.x..f.x + f.w {
            assert_eq!(*out.get_pixel(x, y), RED, "({x},{y}) inside the face");
        }
    }
    // the 2 px gaps around the face stay transparent: nothing bleeds past the region
    for y in f.y..f.y + f.h {
        assert_eq!(*out.get_pixel(f.x - 1, y), Rgba([0, 0, 0, 0]));
        assert_eq!(*out.get_pixel(f.x + f.w, y), Rgba([0, 0, 0, 0]));
    }
    for x in f.x..f.x + f.w {
        assert_eq!(*out.get_pixel(x, f.y - 1), Rgba([0, 0, 0, 0]));
        assert_eq!(*out.get_pixel(x, f.y + f.h), Rgba([0, 0, 0, 0]));
    }
}

#[test]
fn png_export_round_trips_losslessly() {
    let (img, _) = rotated(Quality::Sharp);
    let mut bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgba8(), img);
}

#[test]
fn repeated_transforms_do_not_degrade() {
    let src = RgbaImage::from_fn(300, 200, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 120, if (x / 20 + y / 20) % 3 == 0 { 0 } else { 255 }]));
    let l = Layer { rotation: 12.0, ..Layer::new("a".into(), src) };
    let mut d = doc([255, 255, 255], l.clone());
    let first = compose(&d);
    let mut seed = 0x2545_f491u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for _ in 0..50 {
        let m = &mut d.layers[0];
        m.center = [rnd(), rnd()];
        m.width = 0.1 + rnd();
        m.rotation = rnd() * 360.0;
        m.stretch = 0.5 + rnd();
        let _ = compose(&d);
    }
    d.layers[0] = l;
    assert_eq!(compose(&d), first, "50 edits then back to the start: identical pixels");
}

#[test]
fn rotated_edges_are_antialiased() {
    // geometry only (sRGB-mixed ideal): nearest-neighbour scored ~29 here, filtered must be far closer
    let (out, ideal) = rotated_with(Quality::Sharp, false);
    let (_, mae) = psnr_and_edge_mae(&out, &ideal);
    assert!(mae < 12.0, "edge error vs ideal coverage: {mae}");
    // Pixel quality keeps hard nearest-neighbour edges on purpose
    let (px, _) = rotated_with(Quality::Pixel, false);
    assert!(px.pixels().all(|p| *p == Rgba([255, 255, 255, 255]) || *p == RED));
}

// ------------------------------------------------------------------ baseline report

#[test]
#[ignore]
fn quality_baseline() {
    let dir = std::env::var_os("CS_QUALITY_DIR").map(std::path::PathBuf::from);
    let save = |name: &str, img: &RgbaImage| {
        if let Some(d) = &dir {
            std::fs::create_dir_all(d).unwrap();
            imageops::resize(img, img.width() * 4, img.height() * 4, imageops::FilterType::Nearest).save(d.join(format!("{name}.png"))).unwrap();
        }
    };
    let big = RgbaImage::from_pixel(4000, 3000, RED);
    let t = std::time::Instant::now();
    let wc = crate::compose::working_copy(&big);
    println!("working_copy 4000x3000 -> {:?} in {:.0} ms", wc.dimensions(), t.elapsed().as_secs_f32() * 1000.0);
    println!("| Quality | Rotated PSNR dB | Rotated edge MAE | Lines lin. mean (ideal 0.0625) | 27x mean sRGB (ideal 188) | 27x std | Soft/white fringe max, mean | Soft/black fringe max, mean | compose ms |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for q in Quality::ALL {
        let t = std::time::Instant::now();
        let (rot, ideal) = rotated(q);
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        let (psnr, mae) = psnr_and_edge_mae(&rot, &ideal);
        let (_, mae_srgb) = psnr_and_edge_mae(&rot, &rotated_with(q, false).1);
        // warm-cache redraw of the rotated layer, as during a drag
        let d = doc([255, 255, 255], Layer { quality: q, width: 0.6, rotation: 30.0, ..Layer::new("rot".into(), RgbaImage::from_pixel(512, 512, RED)) });
        let _ = compose(&d);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            let _ = compose(&d);
        }
        let warm = t.elapsed().as_secs_f32() * 1000.0 / 20.0;
        println!("{q:?}: rotated edge MAE vs sRGB-mixed ideal {mae_srgb:.2}; warm rotated compose {warm:.2} ms");
        let lines = thin_lines(q);
        let big = big_downscale(q);
        let (sw, sb) = (soft_edge(q, [255, 255, 255]), soft_edge(q, [0, 0, 0]));
        let (fw, fb) = (fringe(&sw, [255, 255, 255]), fringe(&sb, [0, 0, 0]));
        let mean_big = big.pixels().map(|p| p[0] as f32).sum::<f32>() / big.pixels().len() as f32;
        println!(
            "| {q:?} | {psnr:.2} | {mae:.2} | {:.4} | {mean_big:.1} | {:.2} | {:.1}, {:.2} | {:.1}, {:.2} | {ms:.1} |",
            mean_lin(&lines),
            std_srgb(&big),
            fw.0,
            fw.1,
            fb.0,
            fb.1
        );
        save(&format!("{q:?}_rotated"), &rot);
        save(&format!("{q:?}_thin_lines"), &lines);
        save(&format!("{q:?}_downscale27x"), &big);
        save(&format!("{q:?}_soft_white"), &sw);
        save(&format!("{q:?}_soft_black"), &sb);
        if q == Quality::Sharp {
            save("ideal_rotated", &ideal);
        }
    }
}
