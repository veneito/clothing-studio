//! R6-style block avatar: geometry, template UVs, raycasting and orbit camera.
//! Scene units are studs; +Y up, the avatar faces +Z, character's left is +X.

use crate::template::{self, Face, Part};
use glam::{camera::rh, Mat4, Vec3, Vec4Swizzles};

pub struct BoxPart {
    pub part: Option<Part>,
    pub center: Vec3,
    pub half: Vec3,
    pub color: [f32; 3],
    /// Hidden parts are left out of the display mesh and can't be clicked.
    pub hidden: bool,
}

/// Index ranges in `rig()` for the parts the user can hide (the torso always shows).
pub const HEAD: [usize; 1] = [0];
pub const ARMS: [usize; 2] = [2, 3];
pub const LEGS: [usize; 2] = [4, 5];

const SKIN: [f32; 3] = [0.96, 0.80, 0.41];
// Legs are always black (near-black so the lighting still shows their shape).
const PANTS: [f32; 3] = [0.07, 0.07, 0.08];

/// Box (center, half extents) of a shirt-covered part, in studs.
pub fn part_box(part: Part) -> (Vec3, Vec3) {
    match part {
        Part::Torso => (Vec3::ZERO, Vec3::new(1.0, 1.0, 0.5)),
        Part::RightArm => (Vec3::new(-1.5, 0.0, 0.0), Vec3::new(0.5, 1.0, 0.5)),
        Part::LeftArm => (Vec3::new(1.5, 0.0, 0.0), Vec3::new(0.5, 1.0, 0.5)),
        Part::RightLeg => (Vec3::new(-0.5, -2.0, 0.0), Vec3::new(0.5, 1.0, 0.5)),
        Part::LeftLeg => (Vec3::new(0.5, -2.0, 0.0), Vec3::new(0.5, 1.0, 0.5)),
    }
}

/// Classic R6 proportions: torso 2x2x1, arms/legs 1x2x1 studs.
pub fn rig() -> Vec<BoxPart> {
    let b = |part, c: [f32; 3], h: [f32; 3], color| BoxPart { part, center: c.into(), half: h.into(), color, hidden: false };
    let p = |part| {
        let (c, h) = part_box(part);
        BoxPart { part: Some(part), center: c, half: h, color: SKIN, hidden: false }
    };
    vec![
        b(None, [0.0, 1.6, 0.0], [0.6, 0.6, 0.6], SKIN),
        p(Part::Torso),
        p(Part::RightArm),
        p(Part::LeftArm),
        BoxPart { color: PANTS, ..p(Part::RightLeg) },
        BoxPart { color: PANTS, ..p(Part::LeftLeg) },
    ]
}

/// Vertex flag telling the shader which clothing textures cover a part (Roblox draws the shirt
/// over the pants): 0 = shirt (arms), 0.25 = pants then shirt (torso), 0.5 = pants (legs),
/// 1 = no clothing (head).
pub fn texture_flag(part: Option<Part>) -> f32 {
    match part {
        Some(Part::RightArm | Part::LeftArm) => 0.0,
        Some(Part::Torso) => 0.25,
        Some(Part::RightLeg | Part::LeftLeg) => 0.5,
        None => 1.0,
    }
}

/// Face frame seen from outside: top-left corner, full right edge, full down edge.
pub fn face_frame(face: Face, c: Vec3, h: Vec3) -> (Vec3, Vec3, Vec3) {
    let (x, y, z) = (h.x, h.y, h.z);
    let down_side = Vec3::new(0.0, -2.0 * y, 0.0);
    match face {
        Face::Front => (c + Vec3::new(-x, y, z), Vec3::new(2.0 * x, 0.0, 0.0), down_side),
        Face::Back => (c + Vec3::new(x, y, -z), Vec3::new(-2.0 * x, 0.0, 0.0), down_side),
        Face::Left => (c + Vec3::new(x, y, z), Vec3::new(0.0, 0.0, -2.0 * z), down_side),
        Face::Right => (c + Vec3::new(-x, y, -z), Vec3::new(0.0, 0.0, 2.0 * z), down_side),
        Face::Top => (c + Vec3::new(-x, y, -z), Vec3::new(2.0 * x, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0 * z)),
        Face::Bottom => (c + Vec3::new(-x, -y, z), Vec3::new(2.0 * x, 0.0, 0.0), Vec3::new(0.0, 0.0, -2.0 * z)),
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// rgb = base colour under the clothes (skin, or black legs); a = which textures apply,
    /// see `texture_flag`.
    pub color: [f32; 4],
}

pub fn mesh(rig: &[BoxPart]) -> (Vec<Vertex>, Vec<u32>) {
    let (mut v, mut idx) = (Vec::new(), Vec::new());
    for b in rig.iter().filter(|b| !b.hidden) {
        for face in Face::ALL {
            let (tl, right, down) = face_frame(face, b.center, b.half);
            let n = down.cross(right).normalize();
            let (uv0, uv1) = match b.part {
                Some(p) => {
                    let r = template::region(p, face);
                    let (w, h) = (template::W as f32, template::H as f32);
                    ([r.x as f32 / w, r.y as f32 / h], [(r.x + r.w) as f32 / w, (r.y + r.h) as f32 / h])
                }
                None => ([0.0; 2], [0.0; 2]),
            };
            let color = [b.color[0], b.color[1], b.color[2], texture_flag(b.part)];
            let base = v.len() as u32;
            for (p, uv) in [
                (tl, [uv0[0], uv0[1]]),
                (tl + right, [uv1[0], uv0[1]]),
                (tl + right + down, [uv1[0], uv1[1]]),
                (tl + down, [uv0[0], uv1[1]]),
            ] {
                v.push(Vertex { pos: p.into(), normal: n.into(), uv, color });
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    (v, idx)
}

/// Face-normalized (s, t) -> template UV (0..1 over the whole 585x559 image).
fn template_uv(part: Part, face: Face, s: f32, t: f32) -> [f32; 2] {
    let r = template::region(part, face);
    let (s, t) = (s.clamp(0.0, 1.0), t.clamp(0.0, 1.0));
    [(r.x as f32 + s * r.w as f32) / template::W as f32, (r.y as f32 + t * r.h as f32) / template::H as f32]
}

/// Which rig box an OBJ object belongs to, by Roblox body-part name.
fn box_for_object(name: &str) -> Option<usize> {
    let n = name.to_ascii_lowercase();
    Some(match () {
        _ if n.contains("head") => 0,
        _ if n.contains("torso") => 1,
        _ if n.contains("rightupperarm") || n.contains("rightlowerarm") || n.contains("righthand") => 2,
        _ if n.contains("leftupperarm") || n.contains("leftlowerarm") || n.contains("lefthand") => 3,
        _ if n.contains("right") && (n.contains("leg") || n.contains("foot")) => 4,
        _ if n.contains("left") && (n.contains("leg") || n.contains("foot")) => 5,
        _ => return None, // attachments etc.
    })
}

/// Load an R15/R6 blocky OBJ (e.g. exported from Blender) as the display mesh.
///
/// Its own UVs target Roblox's body atlas, not the classic template, so they are ignored: each
/// triangle is assigned to the matching rig box by object name and projected onto that box's
/// face (picked by the triangle normal) — the same per-face mapping the block rig uses, so
/// raycasting against the boxes stays valid. `offset` moves the OBJ into rig space.
pub fn load_obj(text: &str, offset: Vec3, rig: &[BoxPart]) -> Result<(Vec<Vertex>, Vec<u32>), String> {
    let mut pos: Vec<Vec3> = Vec::new();
    let mut cur: Option<usize> = None;
    let mut out = Vec::new();
    for (ln, line) in text.lines().enumerate() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("o") | Some("g") => cur = it.next().and_then(box_for_object),
            Some("v") => {
                let c: Vec<f32> = it.take(3).map(|s| s.parse().unwrap_or(f32::NAN)).collect();
                if c.len() != 3 || c.iter().any(|v| v.is_nan()) {
                    return Err(format!("line {}: bad vertex", ln + 1));
                }
                pos.push(Vec3::new(c[0], c[1], c[2]) + offset);
            }
            Some("f") => {
                let Some(bi) = cur.filter(|&i| !rig[i].hidden) else { continue };
                let idx: Result<Vec<usize>, String> = it
                    .map(|s| {
                        let i: i64 = s.split('/').next().unwrap_or("").parse().map_err(|_| format!("line {}: bad face", ln + 1))?;
                        let i = if i < 0 { pos.len() as i64 + i } else { i - 1 };
                        usize::try_from(i).ok().filter(|&i| i < pos.len()).ok_or(format!("line {}: index out of range", ln + 1))
                    })
                    .collect();
                let idx = idx?;
                for k in 1..idx.len().saturating_sub(1) {
                    push_tri(&mut out, &rig[bi], [pos[idx[0]], pos[idx[k]], pos[idx[k + 1]]]);
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        return Err("no body-part meshes found (expected objects like UpperTorso, LeftUpperArm, Head)".into());
    }
    let n = out.len() as u32;
    Ok((out, (0..n).collect()))
}

fn push_tri(out: &mut Vec<Vertex>, b: &BoxPart, p: [Vec3; 3]) {
    let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
    let a = n.abs();
    let side = if a.x >= a.z {
        if n.x > 0.0 { Face::Left } else { Face::Right }
    } else if n.z > 0.0 {
        Face::Front
    } else {
        Face::Back
    };
    // Up/down faces only use the Top/Bottom regions on the box's outer cap. Interior ones (R15
    // joints like the waist step between LowerTorso and UpperTorso) take the side texture instead.
    let cy = (p[0].y + p[1].y + p[2].y) / 3.0;
    let on_cap = |plane: f32| (cy - plane).abs() < 0.15;
    let face = if a.y > a.x && a.y > a.z && n.y > 0.0 && on_cap(b.center.y + b.half.y) {
        Face::Top
    } else if a.y > a.x && a.y > a.z && n.y < 0.0 && on_cap(b.center.y - b.half.y) {
        Face::Bottom
    } else {
        side
    };
    let (tl, right, down) = face_frame(face, b.center, b.half);
    for q in p {
        let (uv, color) = match b.part {
            Some(part) => {
                let d = q - tl;
                let uv = template_uv(part, face, d.dot(right) / right.length_squared(), d.dot(down) / down.length_squared());
                (uv, [b.color[0], b.color[1], b.color[2], texture_flag(Some(part))])
            }
            None => ([0.0; 2], [b.color[0], b.color[1], b.color[2], 1.0]),
        };
        out.push(Vertex { pos: q.into(), normal: n.into(), uv, color });
    }
}

/// The R15 blocky rig exported from Blender stands on y=0; our torso is centered at the origin.
pub const R15_OFFSET: Vec3 = Vec3::new(0.0, -3.0, 0.0);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub part: Option<Part>,
    pub face: Face,
    /// Face-normalized coordinates (0..1, top-left origin), same convention as `Layer::center`.
    pub uv: [f32; 2],
    pub t: f32,
}

pub fn raycast(rig: &[BoxPart], origin: Vec3, dir: Vec3) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for b in rig.iter().filter(|b| !b.hidden) {
        let (lo, hi) = (b.center - b.half, b.center + b.half);
        let inv = dir.recip();
        let (t1, t2) = ((lo - origin) * inv, (hi - origin) * inv);
        let (tmin, tmax) = (t1.min(t2).max_element(), t1.max(t2).min_element());
        if tmax < tmin.max(0.0) || best.is_some_and(|h| h.t <= tmin) {
            continue;
        }
        let p = origin + dir * tmin;
        let q = (p - b.center) / b.half;
        let a = q.abs();
        let face = if a.x >= a.y && a.x >= a.z {
            if q.x > 0.0 { Face::Left } else { Face::Right }
        } else if a.y >= a.z {
            if q.y > 0.0 { Face::Top } else { Face::Bottom }
        } else if q.z > 0.0 {
            Face::Front
        } else {
            Face::Back
        };
        let (tl, right, down) = face_frame(face, b.center, b.half);
        let d = p - tl;
        let uv = [d.dot(right) / right.length_squared(), d.dot(down) / down.length_squared()];
        best = Some(Hit { part: b.part, face, uv, t: tmin });
    }
    best
}

pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub target: Vec3,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { yaw: 0.0, pitch: 0.15, dist: 9.0, target: Vec3::new(0.0, 0.0, 0.0) }
    }
}

impl Camera {
    pub fn eye(&self) -> Vec3 {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        self.target + self.dist * Vec3::new(cp * sy, sp, cp * cy)
    }
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        rh::proj::directx::perspective(40f32.to_radians(), aspect, 0.1, 100.0) * rh::view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }
    /// Ray through a point given in normalized viewport coords (0..1, top-left origin).
    pub fn ray(&self, aspect: f32, x: f32, y: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let (nx, ny) = (x * 2.0 - 1.0, 1.0 - y * 2.0);
        let a = inv * glam::Vec4::new(nx, ny, 0.0, 1.0);
        let b = inv * glam::Vec4::new(nx, ny, 1.0, 1.0);
        let (a, b) = (a.xyz() / a.w, b.xyz() / b.w);
        (a, (b - a).normalize())
    }
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.01;
        self.pitch = (self.pitch + dy * 0.01).clamp(-1.5, 1.5);
    }
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let fwd = (self.target - self.eye()).normalize();
        let right = fwd.cross(Vec3::Y).normalize();
        let up = right.cross(fwd);
        let k = self.dist * 0.0015;
        self.target += (-right * dx + up * dy) * k;
    }
    pub fn zoom(&mut self, scroll: f32) {
        self.dist = (self.dist * (-scroll * 0.002).exp()).clamp(3.0, 30.0);
    }
    /// Blender numpad views: 1 front, 3 side, 7 top (Ctrl = opposite), 2/4/6/8 rotate 15°,
    /// Home or "." reset. Returns whether the camera changed.
    pub fn view_key(&mut self, key: &str, ctrl: bool) -> bool {
        use std::f32::consts::FRAC_PI_2;
        let step = 15f32.to_radians();
        let flip = if ctrl { -1.0 } else { 1.0 };
        match key {
            "1" => (self.yaw, self.pitch) = (if ctrl { std::f32::consts::PI } else { 0.0 }, 0.0),
            "3" => (self.yaw, self.pitch) = (FRAC_PI_2 * flip, 0.0),
            "7" => self.pitch = 1.5 * flip,
            "4" => self.yaw -= step,
            "6" => self.yaw += step,
            "8" => self.pitch = (self.pitch - step).clamp(-1.5, 1.5),
            "2" => self.pitch = (self.pitch + step).clamp(-1.5, 1.5),
            "." | "\u{F729}" => *self = Camera::default(), // U+F729 = Slint's Home key
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_torso_center_ray_hits_front_center() {
        let cam = Camera::default();
        let rig = rig();
        let (o, d) = (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -1.0));
        let h = raycast(&rig, o, d).unwrap();
        assert_eq!((h.part, h.face), (Some(Part::Torso), Face::Front));
        assert!((h.uv[0] - 0.5).abs() < 1e-5 && (h.uv[1] - 0.5).abs() < 1e-5);
        // camera center ray also hits the torso front
        let (o, d) = cam.ray(1.0, 0.5, 0.5);
        assert_eq!(raycast(&rig, o, d).unwrap().face, Face::Front);
    }

    #[test]
    fn viewer_left_of_front_is_characters_right_side_of_template() {
        // A point on the viewer's left half of the front torso (character's right, -X) must map
        // to the left half of the template's front-torso region.
        let rig = rig();
        let h = raycast(&rig, Vec3::new(-0.5, 0.5, 10.0), Vec3::NEG_Z).unwrap();
        assert!((h.uv[0] - 0.25).abs() < 1e-5 && (h.uv[1] - 0.25).abs() < 1e-5);
    }

    #[test]
    fn mesh_front_torso_uvs_match_template_region() {
        let (v, _) = mesh(&rig());
        let r = template::region(Part::Torso, Face::Front);
        let want = [r.x as f32 / template::W as f32, r.y as f32 / template::H as f32];
        // torso is rig[1], Front is the first face -> vertices 24..28; first is top-left
        let tl = v[24];
        assert_eq!(tl.pos, [-1.0, 1.0, 0.5]);
        assert_eq!(tl.uv, want);
    }

    #[test]
    fn adjacent_faces_share_edges_in_strip_order() {
        // Front's right edge (character's left) must equal Left face's left edge, and so on, so
        // the side strip in the template wraps continuously around the part.
        for b in rig().iter().filter(|b| b.part.is_some()) {
            let ring = [Face::Front, Face::Left, Face::Back, Face::Right];
            for i in 0..4 {
                let (tl_a, ra, _) = face_frame(ring[i], b.center, b.half);
                let (tl_b, _, _) = face_frame(ring[(i + 1) % 4], b.center, b.half);
                assert!((tl_a + ra - tl_b).length() < 1e-6, "{:?} -> {:?}", ring[i], ring[(i + 1) % 4]);
            }
        }
    }

    #[test]
    fn obj_torso_front_maps_to_template_front_region() {
        // One quad on the front of "UpperTorso" (R15 rig space, standing on y=0).
        let obj = "o UpperTorso_Geo\nv -1 4 0.5\nv 1 4 0.5\nv 1 2.4 0.5\nv -1 2.4 0.5\nf 1 4 3 2\no Neck_Att\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 5 6 7\n";
        let (v, i) = load_obj(obj, R15_OFFSET, &rig()).unwrap();
        assert_eq!(i.len(), 6, "attachment ignored, quad triangulated");
        let r = template::region(Part::Torso, Face::Front);
        let (w, h) = (template::W as f32, template::H as f32);
        let tl = v.iter().find(|x| x.pos == [-1.0, 1.0, 0.5]).unwrap();
        assert_eq!(tl.uv, [r.x as f32 / w, r.y as f32 / h]);
        // y=2.4 is 0.8 of the way down the 2-stud torso
        let bl = v.iter().find(|x| x.pos[0] == -1.0 && (x.pos[1] + 0.6).abs() < 1e-5).unwrap();
        assert!((bl.uv[1] - (r.y as f32 + 0.8 * r.h as f32) / h).abs() < 1e-5);
        assert!(load_obj("o Neck_Att\nv 0 0 0\n", R15_OFFSET, &rig()).is_err());
        // Upward-facing step at the waist (y=2.4, interior) must not sample the Top region.
        let step = "o LowerTorso_Geo\nv -0.2 2.4 0.5\nv 0.2 2.4 0.5\nv 0 2.4 0.3\nf 1 2 3\n";
        let (v, _) = load_obj(step, R15_OFFSET, &rig()).unwrap();
        let top = template::region(Part::Torso, Face::Top);
        assert!(v.iter().all(|x| x.uv[1] * h > (top.y + top.h) as f32), "interior step mapped to Top");
    }

    #[test]
    fn hidden_parts_leave_the_mesh_and_cannot_be_clicked() {
        let mut r = rig();
        let full = mesh(&r).0.len();
        ARMS.iter().for_each(|&i| r[i].hidden = true);
        assert_eq!(mesh(&r).0.len(), full - 2 * 24, "two arm boxes (24 vertices each) removed");
        // a ray at the right arm now passes it by
        let h = raycast(&r, Vec3::new(-1.5, 0.0, 10.0), Vec3::NEG_Z);
        assert!(h.is_none_or(|h| h.part != Some(Part::RightArm)));
        // OBJ: hidden arm objects are skipped, torso kept
        let obj = "o RightUpperArm_Geo
v -2 3 0.5
v -1 3 0.5
v -1 2 0.5
f 1 2 3
o UpperTorso_Geo
v -1 4 0.5
v 1 4 0.5
v 1 3 0.5
f 4 5 6
";
        assert_eq!(load_obj(obj, R15_OFFSET, &r).unwrap().1.len(), 3);
    }

    #[test]
    fn hit_uv_round_trips_through_every_textured_face() {
        let rig = rig();
        for b in rig.iter().filter(|b| b.part.is_some()) {
            for face in Face::ALL {
                let (tl, right, down) = face_frame(face, b.center, b.half);
                let p = tl + right * 0.3 + down * 0.6;
                let n = down.cross(right).normalize();
                let h = raycast(&rig, p + n * 0.01, -n).unwrap();
                if h.part == b.part && h.face == face {
                    assert!((h.uv[0] - 0.3).abs() < 1e-3 && (h.uv[1] - 0.6).abs() < 1e-3, "{:?} {face:?}", b.part);
                }
            }
        }
    }
}
