//! Document model and deterministic compositing. The same `compose()` output feeds the 3D
//! texture, the 2D preview and PNG export.

use crate::avatar;
use crate::template::{self, Face, Garment, Part};
use glam::{Vec2, Vec3};
use image::{imageops, RgbaImage};
use std::rc::Rc;

/// Template pixels per stud. The same on every classic shirt face (e.g. torso front 128 px / 2 studs).
const PX_PER_STUD: f32 = 64.0;

#[derive(Clone)]
pub struct Layer {
    pub name: String,
    /// Untouched source pixels (never modified). Shared so undo snapshots are cheap.
    pub source: Rc<RgbaImage>,
    /// Premultiplied copy capped at 1024 px, used for drawing (see `working_copy`).
    pub work: Rc<RgbaImage>,
    pub visible: bool,
    /// Continue art that crosses a face edge onto the neighbouring faces of the same part.
    pub wrap: bool,
    /// Home face: placement is defined in this face's frame.
    pub part: Part,
    pub face: Face,
    /// Artwork center in face-normalized coords (0..1, origin top-left of the face region).
    pub center: [f32; 2],
    /// Artwork width as a fraction of the face width; height keeps the source aspect.
    pub width: f32,
    /// Degrees, clockwise on the template.
    pub rotation: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub opacity: f32,
    pub grayscale: bool,
    /// -1..1, 0 = unchanged.
    pub brightness: f32,
    /// -1..1, 0 = unchanged.
    pub contrast: f32,
    /// Background-removal mask (alpha channel used), same size as `source`. Kept even when
    /// switched off, so toggling is instant and the original is never modified.
    pub mask: Option<Rc<RgbaImage>>,
    pub remove_bg: bool,
    /// How the art is shrunk onto the (small) template.
    pub quality: Quality,
    /// Invert colours (black <-> white); alpha untouched.
    pub invert: bool,
    /// Text layers keep their settings; `source` is the rendered text.
    pub text: Option<crate::text::TextSpec>,
    /// AI-upscaled (×4) copy of `source`, kept so toggling is instant; the source is untouched.
    pub upscaled: Option<Rc<RgbaImage>>,
    pub upscale: bool,
    /// "Across front": art on a front or back face continues flat onto the same side of the
    /// garment's other parts (torso + sleeves, or torso + legs), which lie in the same plane.
    pub across: Option<Garment>,
    /// Height relative to the source's own proportions (1 = unstretched); set by side/corner drags.
    pub stretch: f32,
}

/// A resize handle on the art's box: (x side, y side), each -1, 0 or 1, not both 0.
pub type Handle = (i8, i8);
pub const HANDLES: [Handle; 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

/// The art's box when a resize drag started (template pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeStart {
    pub w: f32,
    pub h: f32,
    pub cx: f32,
    pub cy: f32,
}

/// Resampling used to shrink art onto the template (chest art is only ~77-128 px wide).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    /// Lanczos plus light sharpening: crisp logos and text. Default for new layers.
    Sharp,
    /// Soft triangle filter: photos and gradients.
    Smooth,
    /// Nearest neighbour: pixel art and hard-edged flat logos.
    Pixel,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Sharp, Quality::Smooth, Quality::Pixel];
}

impl Layer {
    /// New layer centered on the front torso, sized to fit comfortably.
    pub fn new(name: String, source: RgbaImage) -> Self {
        let width = Self::default_width(&source);
        Layer {
            name,
            work: Rc::new(working_copy(&source)),
            source: Rc::new(source),
            visible: true,
            wrap: true,
            part: Part::Torso,
            face: Face::Front,
            center: [0.5, 0.5],
            width,
            rotation: 0.0,
            flip_h: false,
            flip_v: false,
            opacity: 1.0,
            grayscale: false,
            brightness: 0.0,
            contrast: 0.0,
            mask: None,
            remove_bg: false,
            quality: Quality::Sharp,
            invert: false,
            text: None,
            upscaled: None,
            upscale: false,
            across: None,
            stretch: 1.0,
        }
    }

    /// The image actually drawn: the source, with the background mask applied when switched on.
    pub fn effective_source(&self) -> RgbaImage {
        let base = match (&self.upscaled, self.upscale) {
            (Some(u), true) => u.clone(),
            _ => self.source.clone(),
        };
        match (&self.mask, self.remove_bg) {
            // the mask is made at the source's size; stretch it to the upscaled picture if needed
            (Some(m), true) if m.dimensions() != base.dimensions() => {
                crate::bgremove::apply_mask(&base, &imageops::resize(&**m, base.width(), base.height(), imageops::FilterType::Triangle))
            }
            (Some(m), true) => crate::bgremove::apply_mask(&base, m),
            _ => (*base).clone(),
        }
    }

    /// Call after changing `mask` or `remove_bg`.
    pub fn rebuild_work(&mut self) {
        self.work = Rc::new(working_copy(&self.effective_source()));
    }

    fn has_adjustments(&self) -> bool {
        self.invert || self.grayscale || self.brightness != 0.0 || self.contrast != 0.0
    }

    /// True if a template pixel on (part, face) falls inside this layer's (rotated) box, even
    /// where the art is transparent. Used to grab the selected layer anywhere inside its box.
    pub fn box_contains(&self, part: Part, face: Face, px: f32, py: f32) -> bool {
        self.home_point(part, face, px, py).and_then(|(x, y)| self.local(x, y)).is_some()
    }

    /// Affine map from template pixels on `face` to home-face template pixels, or None if this
    /// layer can't reach that face. Unfolding is linear within a face, so three samples define it.
    fn face_map(&self, part: Part, face: Face) -> Option<([f32; 2], [f32; 2], [f32; 2])> {
        let r = template::region(part, face);
        let (x, y) = (r.x as f32, r.y as f32);
        let o = self.home_point(part, face, x, y)?;
        let ex = self.home_point(part, face, x + 1.0, y)?;
        let ey = self.home_point(part, face, x, y + 1.0)?;
        Some(([o.0, o.1], [ex.0 - o.0, ex.1 - o.1], [ey.0 - o.0, ey.1 - o.1]))
    }

    /// Default width as a fraction of the front torso width.
    pub fn default_width(source: &RgbaImage) -> f32 {
        let (sw, sh) = source.dimensions();
        (0.6 * sw.max(1) as f32 / sh.max(1) as f32).min(0.6)
    }

    /// Size on the template in pixels.
    pub fn size_px(&self) -> (u32, u32) {
        let rc = template::region(self.part, self.face);
        let (sw, sh) = self.source.dimensions();
        let w = (self.width * rc.w as f32).round().max(1.0) as u32;
        let h = ((w as f32) * sh.max(1) as f32 / sw.max(1) as f32 * self.stretch).round().max(1.0) as u32;
        (w, h)
    }

    /// Template-pixel position of a resize handle on the (rotated) box.
    pub fn handle_px(&self, (sx, sy): Handle) -> (f32, f32) {
        let (w, h) = self.size_px();
        let (cx, cy) = self.center_px();
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (lx, ly) = (sx as f32 * w as f32 / 2.0, sy as f32 * h as f32 / 2.0);
        (cx + lx * c - ly * s, cy + lx * s + ly * c)
    }

    /// The handle within `tol` template pixels of (px, py), if any (corners win over sides).
    pub fn handle_at(&self, px: f32, py: f32, tol: f32) -> Option<Handle> {
        HANDLES
            .iter()
            .copied()
            .filter(|&h| {
                let (hx, hy) = self.handle_px(h);
                (hx - px).abs() <= tol && (hy - py).abs() <= tol
            })
            .min_by_key(|&(sx, sy)| (sx == 0 || sy == 0) as u8)
    }

    pub fn resize_start(&self) -> ResizeStart {
        let (w, h) = self.size_px();
        let (cx, cy) = self.center_px();
        ResizeStart { w: w as f32, h: h as f32, cx, cy }
    }

    /// Resize by dragging handle `(sx, sy)` to (px, py): the opposite corner/side stays put.
    /// Corners resize freely unless `keep_ratio`; sides change one dimension only.
    pub fn resize_from(&mut self, start: &ResizeStart, (sx, sy): Handle, px: f32, py: f32, keep_ratio: bool) {
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (dx, dy) = (px - start.cx, py - start.cy);
        let (lx, ly) = (dx * c + dy * s, -dx * s + dy * c); // pointer in the art's own frame
        let (ax, ay) = (-(sx as f32) * start.w / 2.0, -(sy as f32) * start.h / 2.0); // fixed point
        let mut nw = if sx != 0 { (lx - ax).abs() } else { start.w };
        let mut nh = if sy != 0 { (ly - ay).abs() } else { start.h };
        if keep_ratio && sx != 0 && sy != 0 {
            let k = (nw / start.w).max(nh / start.h);
            (nw, nh) = (start.w * k, start.h * k);
        }
        let (nw, nh) = (nw.max(4.0), nh.max(4.0));
        let ncx = if sx != 0 { ax + sx as f32 * nw / 2.0 } else { 0.0 };
        let ncy = if sy != 0 { ay + sy as f32 * nh / 2.0 } else { 0.0 };
        let (tx, ty) = (start.cx + ncx * c - ncy * s, start.cy + ncx * s + ncy * c);
        let rc = template::region(self.part, self.face);
        let (sw, sh) = self.source.dimensions();
        self.width = nw / rc.w as f32;
        self.stretch = (nh / nw) / (sh.max(1) as f32 / sw.max(1) as f32);
        self.center = [(tx - rc.x as f32) / rc.w as f32, (ty - rc.y as f32) / rc.h as f32];
    }

    /// Center on the template in pixels.
    pub fn center_px(&self) -> (f32, f32) {
        let rc = template::region(self.part, self.face);
        (rc.x as f32 + self.center[0] * rc.w as f32, rc.y as f32 + self.center[1] * rc.h as f32)
    }

    /// Point in home-face template pixels -> layer-local pixel (0..w, 0..h) if inside the art box.
    fn local(&self, px: f32, py: f32) -> Option<(f32, f32)> {
        let (w, h) = self.size_px();
        let (cx, cy) = self.center_px();
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (dx, dy) = (px - cx, py - cy);
        let (lx, ly) = (dx * c + dy * s + w as f32 / 2.0, -dx * s + dy * c + h as f32 / 2.0);
        (lx >= 0.0 && ly >= 0.0 && lx < w as f32 && ly < h as f32).then_some((lx, ly))
    }

    /// A texel of (part, face) expressed in the home face's template-pixel space, unfolding
    /// adjacent faces flat around the home face. None if this layer can't reach that face.
    fn home_point(&self, part: Part, face: Face, px: f32, py: f32) -> Option<(f32, f32)> {
        if part != self.part {
            return self.across_point(part, face, px, py); // other parts only share art "across"
        }
        if face == self.face {
            return Some((px, py));
        }
        if !self.wrap {
            return None;
        }
        let home = template::region(self.part, self.face);
        let (s, t) = {
            let r = template::region(part, face);
            ((px - r.x as f32) / r.w as f32, (py - r.y as f32) / r.h as f32)
        };
        let u = unfold(part, self.face, face, s, t)?;
        Some((home.x as f32 + u.x * PX_PER_STUD, home.y as f32 + u.y * PX_PER_STUD))
    }

    /// "Across": a texel of another part's face that lies in the same plane as the home face
    /// (e.g. a sleeve front next to the torso front), projected flat into home-face pixels.
    fn across_point(&self, part: Part, face: Face, px: f32, py: f32) -> Option<(f32, f32)> {
        let garment = self.across?;
        if face != self.face || !matches!(face, Face::Front | Face::Back) || !garment.parts().contains(&part) {
            return None;
        }
        let (hc, hh) = avatar::part_box(self.part);
        let (htl, hr, hd) = avatar::face_frame(self.face, hc, hh);
        let (c, h) = avatar::part_box(part);
        let (tl, r, d) = avatar::face_frame(face, c, h);
        let n = hd.cross(hr).normalize();
        if (tl - htl).dot(n).abs() > 1e-4 {
            return None; // not the same plane
        }
        let rr = template::region(part, face);
        let p = tl + r * ((px - rr.x as f32) / rr.w as f32) + d * ((py - rr.y as f32) / rr.h as f32);
        let q = p - htl;
        let home = template::region(self.part, self.face);
        Some((home.x as f32 + q.dot(hr) / hr.length_squared() * home.w as f32, home.y as f32 + q.dot(hd) / hd.length_squared() * home.h as f32))
    }

    /// Move to another face keeping the on-template pixel size.
    pub fn move_to(&mut self, part: Part, face: Face, center: [f32; 2]) {
        if (part, face) != (self.part, self.face) {
            let old = template::region(self.part, self.face).w as f32;
            self.width *= old / template::region(part, face).w as f32;
            (self.part, self.face) = (part, face);
        }
        self.center = center;
    }
}

/// Point at face-normalized (s, t) on `face`, laid flat into `home`'s 2D frame (studs, origin at
/// home's top-left, +x right, +y down). Only faces sharing an edge with `home` unfold; the
/// opposite face returns None.
fn unfold(part: Part, home: Face, face: Face, s: f32, t: f32) -> Option<Vec2> {
    let (c, h) = avatar::part_box(part);
    let (tl, r, d) = avatar::face_frame(home, c, h);
    let (rn, dn) = (r.normalize(), d.normalize());
    let n_home = d.cross(r).normalize(); // outward
    let (ftl, fr, fd) = avatar::face_frame(face, c, h);
    let n_face = fd.cross(fr).normalize();
    // The home side shared with `face` points the same way as `face`'s outward normal.
    let out = [(-rn, Vec2::NEG_X), (rn, Vec2::X), (-dn, Vec2::NEG_Y), (dn, Vec2::Y)]
        .into_iter()
        .find(|(v, _)| v.dot(n_face) > 0.9)?
        .1;
    let p: Vec3 = ftl + fr * s + fd * t;
    let depth = (tl - p).dot(n_home).max(0.0); // distance behind the home plane = distance from the edge
    let q = p + n_home * depth - tl; // projected onto the home plane
    Some(Vec2::new(q.dot(rn), q.dot(dn)) + out * depth)
}

pub struct Document {
    pub base: Rc<RgbaImage>,
    /// Bottom to top.
    pub layers: Vec<Layer>,
    /// Shading drawn over everything (see `shading`).
    pub overlay: Option<Overlay>,
    /// Tattoos drawn *under* the template: they only show where it's transparent (bare arms).
    pub underlay: Option<Overlay>,
}

#[derive(Clone)]
pub struct Overlay {
    /// 585x559; black darkens, white lightens, alpha = amount.
    pub image: Rc<RgbaImage>,
    pub strength: f32,
    /// Index into `shading::PRESETS`, or None for an imported overlay.
    pub preset: Option<usize>,
    pub name: String,
    /// "White = skin": turn light pixels see-through and dark ones into ink (see `ink`).
    pub ink: bool,
}

pub fn compose(doc: &Document) -> RgbaImage {
    let mut out = match &doc.underlay {
        None => (*doc.base).clone(),
        Some(u) => {
            // tattoo first, then the template over it
            let mut out = RgbaImage::new(doc.base.width(), doc.base.height());
            if u.image.dimensions() == out.dimensions() {
                for (d, s) in out.pixels_mut().zip(u.image.pixels()) {
                    let s = if u.ink { ink(*s) } else { *s };
                    blend(d, s, u.strength.clamp(0.0, 1.0));
                }
            }
            for (d, s) in out.pixels_mut().zip(doc.base.pixels()) {
                blend(d, *s, 1.0);
            }
            out
        }
    };
    for l in doc.layers.iter().filter(|l| l.visible) {
        draw_layer(&mut out, l);
    }
    if let Some(o) = &doc.overlay {
        crate::shading::apply(&mut out, &o.image, o.strength);
    }
    out
}

/// Topmost visible layer with an opaque-ish pixel at this template pixel (Photopea auto-select).
pub fn layer_at(doc: &Document, part: Part, face: Face, px: f32, py: f32) -> Option<usize> {
    doc.layers.iter().enumerate().rev().find_map(|(i, l)| {
        if !l.visible {
            return None;
        }
        let (hx, hy) = l.home_point(part, face, px, py)?;
        let (lx, ly) = l.local(hx, hy)?;
        let (w, h) = l.size_px();
        let (sw, sh) = l.source.dimensions();
        let fx = if l.flip_h { w as f32 - lx } else { lx };
        let fy = if l.flip_v { h as f32 - ly } else { ly };
        let sx = ((fx / w as f32 * sw as f32) as u32).min(sw - 1);
        let sy = ((fy / h as f32 * sh as f32) as u32).min(sh - 1);
        let mut a = l.source.get_pixel(sx, sy)[3] as u32;
        if let (Some(m), true) = (&l.mask, l.remove_bg) {
            a = a * m.get_pixel(sx.min(m.width() - 1), sy.min(m.height() - 1))[3] as u32 / 255;
        }
        (a > 24).then_some(i)
    })
}

fn draw_layer(out: &mut RgbaImage, l: &Layer) {
    if l.source.width() == 0 || l.source.height() == 0 {
        return;
    }
    let (w, h) = l.size_px();
    let scaled = scaled_art(l, w, h);
    // The art's axis-aligned bounds in home-face pixels, to skip faces it can't touch.
    let (cx, cy) = l.center_px();
    let (s, c) = l.rotation.to_radians().sin_cos();
    let hw = (w as f32 * c.abs() + h as f32 * s.abs()) / 2.0 + 1.0;
    let hh = (w as f32 * s.abs() + h as f32 * c.abs()) / 2.0 + 1.0;
    // Each face of the layer's part: the home face directly, neighbours unfolded (if wrapping).
    // shortcut: wraps one face deep (no corner/opposite-face wrap); fine for chest/sleeve-sized art.
    let parts = match l.across {
        Some(g) => g.parts().to_vec(),
        None => vec![l.part],
    };
    for (part, face) in parts.into_iter().flat_map(|p| Face::ALL.into_iter().map(move |f| (p, f))) {
        let Some((o, dx, dy)) = l.face_map(part, face) else { continue };
        let rc = template::region(part, face);
        let map = |x: f32, y: f32| [o[0] + dx[0] * (x - rc.x as f32) + dy[0] * (y - rc.y as f32), o[1] + dx[1] * (x - rc.x as f32) + dy[1] * (y - rc.y as f32)];
        let corners = [map(rc.x as f32, rc.y as f32), map((rc.x + rc.w) as f32, rc.y as f32), map(rc.x as f32, (rc.y + rc.h) as f32), map((rc.x + rc.w) as f32, (rc.y + rc.h) as f32)];
        let (min_x, max_x) = corners.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        let (min_y, max_y) = corners.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
        if max_x < cx - hw || min_x > cx + hw || max_y < cy - hh || min_y > cy + hh {
            continue; // the art doesn't reach this face
        }
        for y in rc.y..rc.y + rc.h {
            for x in rc.x..rc.x + rc.w {
                let [hx, hy] = map(x as f32 + 0.5, y as f32 + 0.5);
                let Some((lx, ly)) = l.local(hx, hy) else { continue };
                let sp = *scaled.get_pixel(lx as u32, ly as u32);
                blend(out.get_pixel_mut(x, y), sp, l.opacity);
            }
        }
    }
}

/// What the shrunk art depends on (its position does not): the cache key.
#[derive(PartialEq)]
struct ScaleKey {
    work: Rc<RgbaImage>,
    size: (u32, u32),
    quality: Quality,
    flips: (bool, bool),
    adjust: (bool, bool, u32, u32),
}

thread_local! {
    /// Recently shrunk art. Dragging only moves art, so this turns the expensive resize (and
    /// sharpening/adjustments) into a lookup on every frame of a drag.
    static SCALE_CACHE: std::cell::RefCell<Vec<(ScaleKey, Rc<RgbaImage>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn scaled_art(l: &Layer, w: u32, h: u32) -> Rc<RgbaImage> {
    let key = ScaleKey {
        work: l.work.clone(), // holding the Rc keeps its address from being reused while cached
        size: (w, h),
        quality: l.quality,
        flips: (l.flip_h, l.flip_v),
        adjust: (l.invert, l.grayscale, l.brightness.to_bits(), l.contrast.to_bits()),
    };
    let same = |k: &ScaleKey| Rc::ptr_eq(&k.work, &key.work) && k.size == key.size && k.quality == key.quality && k.flips == key.flips && k.adjust == key.adjust;
    if let Some(hit) = SCALE_CACHE.with_borrow(|c| c.iter().find(|(k, _)| same(k)).map(|(_, v)| v.clone())) {
        return hit;
    }
    let mut scaled = resize_premultiplied(&l.work, w, h, l.quality);
    if l.flip_h {
        imageops::flip_horizontal_in_place(&mut scaled);
    }
    if l.flip_v {
        imageops::flip_vertical_in_place(&mut scaled);
    }
    if l.has_adjustments() {
        for p in scaled.pixels_mut() {
            if l.invert {
                (p[0], p[1], p[2]) = (255 - p[0], 255 - p[1], 255 - p[2]);
            }
            *p = adjust(*p, l.grayscale, l.brightness, l.contrast);
        }
    }
    let scaled = Rc::new(scaled);
    SCALE_CACHE.with_borrow_mut(|c| {
        if c.len() >= 32 {
            c.remove(0); // oldest out
        }
        c.push((key, scaled.clone()));
    });
    scaled
}

/// Grayscale (Rec. 601 luma), then brightness (adds up to ±255), then contrast around mid-grey
/// (factor (1+c)/(1-c): 0 keeps, -1 flattens to grey, toward +1 approaches a hard threshold).
/// Alpha is untouched.
pub fn adjust(p: image::Rgba<u8>, grayscale: bool, brightness: f32, contrast: f32) -> image::Rgba<u8> {
    let mut c = [p[0] as f32, p[1] as f32, p[2] as f32];
    if grayscale {
        let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        c = [y; 3];
    }
    let k = contrast.clamp(-1.0, 0.95);
    let factor = (1.0 + k) / (1.0 - k);
    let out = c.map(|v| ((v + brightness * 255.0 - 127.5) * factor + 127.5).round().clamp(0.0, 255.0) as u8);
    image::Rgba([out[0], out[1], out[2], p[3]])
}

/// Ink: opacity follows darkness (white -> clear, black -> solid), colour kept, so red ink stays
/// red and the avatar's skin shows between the lines.
pub fn ink(p: image::Rgba<u8>) -> image::Rgba<u8> {
    let lum = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
    let a = (p[3] as f32 * (1.0 - lum / 255.0)).round() as u8;
    image::Rgba([p[0], p[1], p[2], a])
}

/// Straight-alpha "over".
fn blend(d: &mut image::Rgba<u8>, s: image::Rgba<u8>, opacity: f32) {
    let sa = s[3] as f32 / 255.0 * opacity;
    let da = d[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    for c in 0..3 {
        let v = (s[c] as f32 * sa + d[c] as f32 * da * (1.0 - sa)) / oa;
        d[c] = v.round().clamp(0.0, 255.0) as u8;
    }
    d[3] = (oa * 255.0).round() as u8;
}

/// Largest side of a layer's working copy. Art is never drawn larger than ~256 px on the
/// template, so this keeps full quality while big pasted images stay fast to redraw.
const WORK_MAX: u32 = 1024;

/// Premultiplied, size-capped copy of a source, made once per layer (see `Layer::work`).
pub fn working_copy(src: &RgbaImage) -> RgbaImage {
    let mut pm = src.clone();
    for p in pm.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + 127) / 255) as u8;
        }
    }
    let (w, h) = pm.dimensions();
    if w.max(h) > WORK_MAX {
        let k = WORK_MAX as f32 / w.max(h) as f32;
        let (nw, nh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        pm = imageops::resize(&pm, nw, nh, imageops::FilterType::Triangle);
    }
    pm
}

/// Resize a premultiplied working copy, returning straight alpha (no dark fringes at edges).
fn resize_premultiplied(pm: &RgbaImage, w: u32, h: u32, quality: Quality) -> RgbaImage {
    let mut out = match quality {
        Quality::Smooth => imageops::resize(pm, w, h, imageops::FilterType::Triangle),
        Quality::Pixel => imageops::resize(pm, w, h, imageops::FilterType::Nearest),
        Quality::Sharp => {
            let r = imageops::resize(pm, w, h, imageops::FilterType::Lanczos3);
            // Sharpen while still premultiplied (all four channels alike), then keep colour <=
            // alpha, so edges against transparency get crisper without dark or light halos.
            let mut r = imageops::unsharpen(&r, 0.6, 1);
            for p in r.pixels_mut() {
                for c in 0..3 {
                    p[c] = p[c].min(p[3]);
                }
            }
            r
        }
    };
    for p in out.pixels_mut() {
        let a = p[3] as u32;
        if a > 0 {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

    fn art() -> RgbaImage {
        // 64x64: left half opaque red, right half fully transparent.
        RgbaImage::from_fn(64, 64, |x, _| if x < 32 { RED } else { Rgba([0, 255, 0, 0]) })
    }

    fn solid() -> RgbaImage {
        RgbaImage::from_pixel(64, 64, RED)
    }

    fn layer(width: f32, center: [f32; 2]) -> Layer {
        Layer { width, center, ..Layer::new("art".into(), art()) }
    }

    fn doc_with(layers: Vec<Layer>) -> Document {
        Document { base: Rc::new(template::preset([255, 255, 255], false)), layers, overlay: None, underlay: None }
    }

    fn doc(width: f32, center: [f32; 2]) -> Document {
        doc_with(vec![layer(width, center)])
    }

    fn region(f: Face) -> template::Rect {
        template::region(Part::Torso, f)
    }

    #[test]
    fn output_is_template_size() {
        assert_eq!(compose(&doc(0.5, [0.5, 0.5])).dimensions(), (template::W, template::H));
    }

    #[test]
    fn art_lands_in_front_torso_and_keeps_transparency() {
        let out = compose(&doc(0.5, [0.5, 0.5]));
        let f = region(Face::Front);
        // art is 64px wide centered: x 263..327. Left half red, right half transparent -> base white.
        let cy = f.y + f.h / 2;
        assert_eq!(*out.get_pixel(f.x + 40, cy), RED);
        assert_eq!(*out.get_pixel(f.x + 85, cy), WHITE);
        assert_eq!(*out.get_pixel(f.x + 5, f.y + 5), WHITE);
    }

    #[test]
    fn without_wrap_art_is_clipped_to_face_region() {
        let mut d = doc(1.5, [0.2, 0.5]);
        d.layers[0].wrap = false;
        let out = compose(&d);
        let (f, side) = (region(Face::Front), region(Face::Right)); // Right is the left neighbour in the image
        let cy = f.y + f.h / 2;
        assert_eq!(*out.get_pixel(f.x, cy), RED);
        assert_eq!(*out.get_pixel(side.x + side.w - 1, cy), WHITE);
    }

    #[test]
    fn wrap_continues_across_side_and_top_seams() {
        let f = region(Face::Front);
        // Solid art hanging 10px off the front's right edge (character's left) and top edge.
        let w = 40.0;
        let l = Layer {
            width: w / 128.0,
            center: [(128.0 - 10.0) / 128.0, 10.0 / 128.0],
            ..Layer::new("solid".into(), solid())
        };
        let out = compose(&doc_with(vec![l.clone()]));
        let (left, top) = (region(Face::Left), region(Face::Top));
        // Front's right edge meets Left's left edge: the first 10 columns of Left are painted.
        assert_eq!(*out.get_pixel(left.x + 2, f.y + 15), RED);
        assert_eq!(*out.get_pixel(left.x + 12, f.y + 15), WHITE);
        // Front's top edge meets Top's bottom edge: the last 10 rows of Top are painted.
        assert_eq!(*out.get_pixel(f.x + 110, top.y + top.h - 2), RED);
        assert_eq!(*out.get_pixel(f.x + 110, top.y + top.h - 12), WHITE);
        // never onto the opposite face or another part
        let back = region(Face::Back);
        assert_eq!(*out.get_pixel(back.x + 5, back.y + 15), WHITE);
        // the wrapped part is clickable too
        let d = doc_with(vec![l]);
        assert_eq!(layer_at(&d, Part::Torso, Face::Left, (left.x + 2) as f32, (f.y + 15) as f32), Some(0));
    }

    #[test]
    fn wrap_is_continuous_across_the_seam() {
        // A pixel just inside the front edge and just past it on the side must map to adjacent
        // points in the home frame (no jump, no mirror).
        let l = layer(0.5, [0.5, 0.5]);
        let (f, left) = (region(Face::Front), region(Face::Left));
        let y = (f.y + 40) as f32;
        let a = l.home_point(Part::Torso, Face::Front, (f.x + f.w) as f32 - 0.5, y).unwrap();
        let b = l.home_point(Part::Torso, Face::Left, left.x as f32 + 0.5, y).unwrap();
        assert!((b.0 - a.0 - 1.0).abs() < 1e-3 && (b.1 - a.1).abs() < 1e-3, "{a:?} {b:?}");
        // arm seam: right sleeve front -> its outer (Right) side, which sits left of Front in the strip
        let mut arm = l.clone();
        arm.move_to(Part::RightArm, Face::Front, [0.5, 0.5]);
        let (af, ar) = (template::region(Part::RightArm, Face::Front), template::region(Part::RightArm, Face::Right));
        let a = arm.home_point(Part::RightArm, Face::Front, af.x as f32 + 0.5, y).unwrap();
        let b = arm.home_point(Part::RightArm, Face::Right, (ar.x + ar.w) as f32 - 0.5, y).unwrap();
        assert!((a.0 - b.0 - 1.0).abs() < 1e-3 && (b.1 - a.1).abs() < 1e-3, "{a:?} {b:?}");
    }

    #[test]
    fn source_is_preserved() {
        let d = doc(0.5, [0.5, 0.5]);
        let _ = compose(&d);
        assert_eq!(*d.layers[0].source, art());
    }

    #[test]
    fn flip_and_rotate_move_the_red_half() {
        let f = region(Face::Front);
        let (cx, cy) = (f.x + 64, f.y + 64);
        let mut d = doc(0.5, [0.5, 0.5]);
        d.layers[0].flip_h = true; // red now on the right
        let out = compose(&d);
        assert_eq!((*out.get_pixel(cx - 20, cy), *out.get_pixel(cx + 20, cy)), (WHITE, RED));
        d.layers[0].flip_h = false;
        d.layers[0].rotation = 90.0; // clockwise: left half -> top half
        let out = compose(&d);
        assert_eq!((*out.get_pixel(cx, cy - 20), *out.get_pixel(cx, cy + 20)), (RED, WHITE));
    }

    #[test]
    fn adjustments() {
        let p = Rgba([200, 100, 50, 77]);
        assert_eq!(adjust(p, false, 0.0, 0.0), p, "neutral settings change nothing");
        let g = adjust(p, true, 0.0, 0.0);
        assert!(g[0] == g[1] && g[1] == g[2] && g[3] == 77, "grey keeps alpha");
        assert_eq!(adjust(p, false, 1.0, 0.0), Rgba([255, 255, 255, 77]));
        assert_eq!(adjust(p, false, -1.0, 0.0), Rgba([0, 0, 0, 77]));
        let flat = adjust(p, false, 0.0, -1.0);
        assert!(flat[0] == 128 && flat[2] == 128, "min contrast is mid grey: {flat:?}");
        let hi = adjust(Rgba([140, 115, 128, 255]), false, 0.0, 0.5);
        assert!(hi[0] > 140 && hi[1] < 115, "more contrast pushes away from grey: {hi:?}");
    }

    #[test]
    fn adjustments_apply_to_output_not_source() {
        let mut d = doc(0.5, [0.5, 0.5]);
        d.layers[0].grayscale = true;
        let out = compose(&d);
        let f = region(Face::Front);
        let px = *out.get_pixel(f.x + 40, f.y + 64);
        assert!(px[0] == px[1] && px[1] == px[2] && px[0] < 255, "red became grey: {px:?}");
        assert_eq!(*d.layers[0].source, art(), "source untouched");
    }

    /// Blue/white vertical stripes 4 px wide, 256 px square.
    fn stripes() -> RgbaImage {
        RgbaImage::from_fn(256, 256, |x, _| if (x / 32) % 2 == 0 { Rgba([20, 40, 200, 255]) } else { WHITE })
    }

    fn render(q: Quality, src: RgbaImage) -> RgbaImage {
        let l = Layer { quality: q, width: 0.5, ..Layer::new("s".into(), src) };
        compose(&doc_with(vec![l]))
    }

    #[test]
    fn pixel_quality_only_uses_source_colours() {
        let out = render(Quality::Pixel, stripes());
        let f = region(Face::Front);
        for x in f.x + 33..f.x + 95 {
            let p = *out.get_pixel(x, f.y + 64);
            assert!(p == Rgba([20, 40, 200, 255]) || p == WHITE, "blended pixel {p:?} at {x}");
        }
    }

    #[test]
    fn sharp_has_more_edge_contrast_than_smooth() {
        // sum of |difference| between neighbouring pixels across the art = edge crispness
        let crisp = |img: &RgbaImage| {
            let f = region(Face::Front);
            (f.x + 33..f.x + 95).map(|x| (img.get_pixel(x, f.y + 64)[0] as i32 - img.get_pixel(x + 1, f.y + 64)[0] as i32).abs()).sum::<i32>()
        };
        let (sharp, smooth) = (crisp(&render(Quality::Sharp, stripes())), crisp(&render(Quality::Smooth, stripes())));
        assert!(sharp > smooth, "sharp {sharp} vs smooth {smooth}");
    }

    #[test]
    fn sharp_adds_no_dark_halo_on_transparent_edges() {
        // solid red disc on transparency over a white shirt: no pixel may get darker than the
        // red/white mix (a dark halo would push green/blue below the red's own value of 0..)
        let disc = RgbaImage::from_fn(300, 300, |x, y| {
            let d = ((x as f32 - 150.0).powi(2) + (y as f32 - 150.0).powi(2)).sqrt();
            if d < 120.0 { Rgba([230, 30, 30, 255]) } else { Rgba([0, 0, 0, 0]) }
        });
        let out = render(Quality::Sharp, disc);
        let f = region(Face::Front);
        for y in f.y..f.y + f.h {
            for x in f.x..f.x + f.w {
                let p = out.get_pixel(x, y);
                assert!(p[0] >= 200, "dark halo at ({x},{y}): {p:?}");
            }
        }
    }

    #[test]
    fn invert_swaps_black_and_white() {
        let art = RgbaImage::from_fn(64, 64, |x, _| if x < 32 { Rgba([0, 0, 0, 255]) } else { WHITE });
        let l = Layer { invert: true, quality: Quality::Pixel, width: 0.5, ..Layer::new("bw".into(), art) };
        let out = compose(&Document { base: Rc::new(template::preset([128, 128, 128], false)), layers: vec![l], overlay: None, underlay: None });
        let f = region(Face::Front);
        assert_eq!(*out.get_pixel(f.x + 40, f.y + 64), WHITE, "black became white");
        assert_eq!(*out.get_pixel(f.x + 85, f.y + 64), Rgba([0, 0, 0, 255]), "white became black");
    }

    #[test]
    fn cached_drag_matches_fresh_render() {
        // Moving a layer reuses the cached shrunk art; the result must equal an uncached render.
        let mut d = doc(0.5, [0.5, 0.5]);
        let _ = compose(&d); // warm the cache
        d.layers[0].center = [0.3, 0.6];
        let cached = compose(&d);
        SCALE_CACHE.with_borrow_mut(|c| c.clear());
        assert_eq!(cached, compose(&d));
    }

    #[test]
    fn box_contains_transparent_parts() {
        let l = layer(0.5, [0.5, 0.5]); // right half of the art is transparent
        let f = region(Face::Front);
        let (cx, cy) = ((f.x + 64) as f32, (f.y + 64) as f32);
        assert!(l.box_contains(Part::Torso, Face::Front, cx + 20.0, cy));
        assert!(!l.box_contains(Part::Torso, Face::Front, cx + 40.0, cy));
    }

    #[test]
    fn tattoo_underlay_shows_only_where_the_template_is_transparent() {
        let base = template::preset([255, 255, 255], false); // short sleeves: lower arm bare
        let tattoo = RgbaImage::from_pixel(template::W, template::H, Rgba([20, 20, 20, 255]));
        let mut d = Document { base: Rc::new(base), layers: vec![], overlay: None, underlay: Some(Overlay { image: Rc::new(tattoo), strength: 1.0, preset: None, name: "t".into(), ink: false }) };
        let out = compose(&d);
        let arm = template::region(Part::RightArm, Face::Front);
        assert_eq!(*out.get_pixel(arm.x + 5, arm.y + arm.h - 2), Rgba([20, 20, 20, 255]), "tattoo on the bare forearm");
        assert_eq!(*out.get_pixel(arm.x + 5, arm.y + 5), Rgba([255, 255, 255, 255]), "the sleeve covers it");
        // white = skin: a white tattoo background disappears
        d.underlay = Some(Overlay { image: Rc::new(RgbaImage::from_pixel(template::W, template::H, WHITE)), strength: 1.0, preset: None, name: "w".into(), ink: true });
        assert_eq!(compose(&d).get_pixel(arm.x + 5, arm.y + arm.h - 2)[3], 0, "white became skin");
        assert_eq!(ink(Rgba([0, 0, 0, 255]))[3], 255);
        assert!(ink(Rgba([200, 20, 20, 255]))[0] == 200, "coloured ink keeps its colour");
    }

    #[test]
    fn across_front_continues_onto_the_sleeves_in_line() {
        // solid art wider than the torso front, on the torso front
        let mut l = Layer { width: 2.0, center: [0.5, 0.5], wrap: false, ..Layer::new("wide".into(), RgbaImage::from_pixel(64, 16, RED)) };
        let base = || Rc::new(template::preset([255, 255, 255], true));
        let doc = |l: &Layer| Document { base: base(), layers: vec![l.clone()], overlay: None, underlay: None };
        let (ra, la) = (template::region(Part::RightArm, Face::Front), template::region(Part::LeftArm, Face::Front));
        let f = region(Face::Front);
        // art is 256x64 px centred on the torso front: the middle 64 of its 128 rows. Torso and
        // sleeves are both 2 studs tall, so on a sleeve that's the same rows of *its* region.
        let (mid, arm_mid) = (f.y + 64, ra.y + 64);
        assert_eq!(*compose(&doc(&l)).get_pixel(ra.x + ra.w - 3, arm_mid), WHITE, "off: sleeves untouched");
        l.across = Some(Garment::Shirt);
        let out = compose(&doc(&l));
        // the right sleeve sits left of the torso front, so its inner (right-hand) columns get art
        assert_eq!(*out.get_pixel(ra.x + ra.w - 3, arm_mid), RED, "right sleeve front");
        assert_eq!(*out.get_pixel(la.x + 2, la.y + 64), RED, "left sleeve front");
        assert_eq!(*out.get_pixel(ra.x + ra.w - 3, ra.y + 20), WHITE, "same height as on the torso only");
        let side = region(Face::Left);
        assert_eq!(*out.get_pixel(side.x + 5, mid), WHITE, "no wrap around the torso side");
        let back = template::region(Part::RightArm, Face::Back);
        assert_eq!(*out.get_pixel(back.x + 5, back.y + 64), WHITE, "sleeve backs untouched");
        // the arm portion is clickable as part of the layer
        let d = doc(&l);
        assert_eq!(layer_at(&d, Part::RightArm, Face::Front, (ra.x + ra.w - 3) as f32, arm_mid as f32), Some(0));
    }

    #[test]
    fn corner_drag_keeps_the_opposite_corner_and_shift_keeps_proportions() {
        let mut l = layer(0.5, [0.5, 0.5]); // 64x64 at the torso front centre
        let start = l.resize_start();
        let tl = l.handle_px((-1, -1));
        let br = l.handle_px((1, 1));
        // drag bottom-right corner 32 px right and 8 px down, freely
        l.resize_from(&start, (1, 1), br.0 + 32.0, br.1 + 8.0, false);
        assert_eq!(l.size_px(), (96, 72));
        let tl2 = l.handle_px((-1, -1));
        assert!((tl2.0 - tl.0).abs() < 0.6 && (tl2.1 - tl.1).abs() < 0.6, "top-left stays: {tl:?} -> {tl2:?}");
        // same drag with Shift keeps the 1:1 proportions (bigger of the two)
        let mut k = layer(0.5, [0.5, 0.5]);
        k.resize_from(&start, (1, 1), br.0 + 32.0, br.1 + 8.0, true);
        assert_eq!(k.size_px(), (96, 96));
    }

    #[test]
    fn side_drag_changes_one_dimension_even_when_rotated() {
        let mut l = layer(0.5, [0.5, 0.5]);
        l.rotation = 90.0;
        let start = l.resize_start();
        let right = l.handle_px((1, 0)); // rotated 90° clockwise: the art's right side points down
        let left = l.handle_px((-1, 0));
        l.resize_from(&start, (1, 0), right.0, right.1 + 20.0, false);
        assert_eq!(l.size_px(), (84, 64), "only the art's width grew");
        let left2 = l.handle_px((-1, 0));
        assert!((left2.0 - left.0).abs() < 0.6 && (left2.1 - left.1).abs() < 0.6, "opposite side stays");
        assert_eq!(l.handle_at(left2.0 + 2.0, left2.1, 5.0), Some((-1, 0)));
        assert_eq!(l.handle_at(left2.0 + 40.0, left2.1, 5.0), None);
    }

    #[test]
    fn hidden_layers_are_not_drawn() {
        let mut d = doc(0.5, [0.5, 0.5]);
        d.layers[0].visible = false;
        assert_eq!(compose(&d), *d.base);
    }

    #[test]
    fn layer_at_picks_topmost_opaque_pixel() {
        let mut d = doc(0.5, [0.5, 0.5]);
        d.layers.push(layer(0.5, [0.5, 0.5]));
        let f = region(Face::Front);
        let (cx, cy) = ((f.x + 64) as f32, (f.y + 64) as f32);
        assert_eq!(layer_at(&d, Part::Torso, Face::Front, cx - 20.0, cy), Some(1));
        assert_eq!(layer_at(&d, Part::Torso, Face::Front, cx + 20.0, cy), None);
        assert_eq!(layer_at(&d, Part::Torso, Face::Back, cx - 20.0, cy), None);
        d.layers[1].visible = false;
        assert_eq!(layer_at(&d, Part::Torso, Face::Front, cx - 20.0, cy), Some(0));
    }

    #[test]
    fn move_to_other_face_keeps_pixel_size() {
        let mut l = layer(0.5, [0.5, 0.5]);
        let before = l.size_px();
        l.move_to(Part::RightArm, Face::Front, [0.5, 0.3]);
        assert_eq!(l.size_px(), before);
        assert_eq!((l.part, l.face), (Part::RightArm, Face::Front));
    }
}

#[cfg(test)]
mod perf {
    use super::*;
    #[test]
    #[ignore]
    fn big_source_compose_time() {
        let big = RgbaImage::from_fn(3000, 3000, |x, y| image::Rgba([(x % 255) as u8, (y % 255) as u8, 90, 255]));
        let d = Document { base: Rc::new(template::preset([255, 255, 255], false)), layers: vec![Layer::new("big".into(), big)], overlay: None, underlay: None };
        let t = std::time::Instant::now();
        for _ in 0..5 {
            let _ = compose(&d);
        }
        eprintln!("PERF compose with 3000x3000 source: {:?} per call", t.elapsed() / 5);
    }
}

#[cfg(test)]
mod quality_preview {
    use super::*;
    /// Writes a Sharp | Smooth | Pixel comparison of $CS_QUALITY_SRC to $CS_QUALITY_OUT.
    #[test]
    #[ignore]
    fn write_quality_comparison() {
        let (Some(src), Some(out)) = (std::env::var_os("CS_QUALITY_SRC"), std::env::var_os("CS_QUALITY_OUT")) else { return };
        let art = image::open(src).unwrap().to_rgba8();
        let f = template::region(Part::Torso, Face::Front);
        let mut sheet = RgbaImage::new(f.w * 4 * 3 + 20, f.h * 4);
        for (i, q) in Quality::ALL.into_iter().enumerate() {
            let l = Layer { quality: q, ..Layer::new("a".into(), art.clone()) };
            let d = Document { base: Rc::new(template::preset([20, 20, 20], true)), layers: vec![l], overlay: None, underlay: None };
            let img = compose(&d);
            let crop = imageops::crop_imm(&img, f.x, f.y, f.w, f.h).to_image();
            let big = imageops::resize(&crop, f.w * 4, f.h * 4, imageops::FilterType::Nearest);
            imageops::replace(&mut sheet, &big, (i as u32 * (f.w * 4 + 10)) as i64, 0);
        }
        sheet.save(out).unwrap();
    }
}

#[cfg(test)]
mod perf2 {
    use super::*;
    #[test]
    #[ignore]
    fn drag_frame_cost() {
        let art = RgbaImage::from_fn(2000, 1500, |x, y| image::Rgba([(x % 255) as u8, (y % 255) as u8, 90, if (x / 50 + y / 50) % 2 == 0 { 255 } else { 0 }]));
        for q in Quality::ALL {
            let layers: Vec<Layer> = (0..3).map(|_| Layer { quality: q, ..Layer::new("a".into(), art.clone()) }).collect();
            let d = Document { base: Rc::new(template::preset([255, 255, 255], false)), layers, overlay: None, underlay: None };
            let t = std::time::Instant::now();
            for _ in 0..5 { let _ = compose(&d); }
            eprintln!("PERF {q:?}: 3 layers compose {:?}", t.elapsed() / 5);
        }
        let big = Rc::new(art.clone());
        let mut c = crate::project::PngCache::default();
        let d = Document { base: Rc::new(template::preset([255, 255, 255], false)), layers: vec![Layer { source: big, ..Layer::new("a".into(), art) }], overlay: None, underlay: None };
        let t = std::time::Instant::now();
        let j = crate::project::to_json((&d, None), (&d, None), &mut c).unwrap();
        eprintln!("PERF first save/autosave (encode 2000x1500): {:?}, {} KB", t.elapsed(), j.len() / 1024);
        let t = std::time::Instant::now();
        let _ = crate::project::to_json((&d, None), (&d, None), &mut c).unwrap();
        eprintln!("PERF later autosave (cached): {:?}", t.elapsed());
    }
}
