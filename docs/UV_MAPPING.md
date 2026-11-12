# Roblox classic shirt UV mapping

Source of truth in code: [`src/template.rs`](../src/template.rs) → `region()`.

Canvas: **585 × 559 px**. Every region is a cube face of the R6 torso (2×2×1 studs) or an arm
(1×2×1 studs), at 64 px per stud.

## Regions (x, y, w, h)

"Left" and "Right" mean the **character's own** left and right. In the scene, the avatar faces
+Z and its left is +X.

| Part | Top | Front | Back | Left side | Right side | Bottom |
|---|---|---|---|---|---|---|
| Torso | 231,8,128,64 | 231,74,128,128 | 427,74,128,128 | 361,74,64,128 | 165,74,64,128 | 231,204,128,64 |
| Right arm | 217,289,64,64 | 217,355,64,128 | 85,355,64,128 | 19,355,64,128 (inner) | 151,355,64,128 (outer) | 217,485,64,64 |
| Left arm | 308,289,64,64 | 308,355,64,128 | 440,355,64,128 | 374,355,64,128 (outer) | 506,355,64,128 (inner) | 308,485,64,64 |

## Orientation

- **Side faces** (front, back, left, right) are drawn as seen from outside the part, with image-up
  as world-up. Each row of side faces wraps continuously around the part:
  torso `Right | Front | Left | Back`, right arm `inner | Back | outer | Front`,
  left arm `Front | outer | Back | inner`.
  So on the front torso, the image's left half is the character's right side, which is the
  viewer's left. That's what you'd expect.
- **Top**: the image's bottom edge borders the front face.
- **Bottom**: the image's top edge borders the front face.

The automated tests (`cargo test`) check the following:
- Every region is in bounds, has the right size, and doesn't overlap another.
- The mesh UVs match these regions.
- Neighbouring faces share 3D edges in strip order.
- A raycast hit point maps back to the correct face position, on every face.

## Pants

Classic pants use the same 585 × 559 layout. The torso regions are identical. The **right leg**
uses the right-arm regions and the **left leg** uses the left-arm regions, with the same
orientation rules (inner/outer side, top, bottom). In code: `region(Part::RightLeg, f) ==
region(Part::RightArm, f)`. Roblox draws the shirt over the pants on the torso, and the app's 3D
view does the same.

**Status:** the pants layout is **not yet checked in Roblox Studio**; the shirt layout is (see
below). To check it, export a pants PNG with art on each leg's front and outer side, put it on an
R6 block rig as `Pants.PantsTemplate`, and compare with the app.

## How it was verified

The region positions match the widely used community template, and the orientation follows from
the wrap continuity described above. There's no official Roblox document listing these
coordinates. **Checked in Roblox Studio on 2026-11-12:** an exported shirt on a Roblox avatar
matched the app. That was a visual check, not a pixel-by-pixel comparison.

## Manual check in Roblox Studio (status: passed on 2026-11-12)

1. Run `cargo run -- --diagnostic diag.png`. Each face gets its own color with a black square in
   its top-left corner.
2. In Studio, open a baseplate. Go to Avatar → Rig Builder → **Block Avatar, R6**.
3. Import `diag.png` with View → Asset Manager → Bulk Import (it uploads as an image), then copy
   its asset ID.
4. Insert a `Shirt` into the rig and set `ShirtTemplate` to `rbxassetid://<id>`. If a decal ID
   doesn't work, use the image ID instead.
5. Compare against Clothing Studio with **Load template…** → `diag.png`, looking at the same faces
   from the same angles. For every face, check two things:
   - **Color**: the face is the same color in both.
   - **Corner square**: the black square is in the same corner (seen from outside the part).
6. Repeat with an exported shirt that has artwork on the front torso. Check that the artwork is
   upright, not mirrored, and in the same spot.

Record the result here. If a face is mirrored or rotated, fix it in `face_frame()` (orientation)
or `region()` (position). The tests will then lock the fix in.
