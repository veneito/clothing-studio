# Architecture (Milestone 0)

## Research findings

- **Versions used:** Rust 1.98.1, Slint 1.18.1, wgpu 30 (pulled in through Slint), image 0.25,
  glam 0.34, rfd 0.18.
- **Slint + wgpu works natively.** Slint 1.18 has an `unstable-wgpu-30` feature.
  `BackendSelector::require_wgpu_30()` makes Slint render with wgpu. A rendering notifier then
  hands us Slint's own `wgpu::Device` and `Queue`. We render the avatar into a `wgpu::Texture` and
  show it with `slint::Image::try_from(texture)`, which avoids any copy back to the CPU. No
  alternative UI framework was needed.
  - Risk: the feature is marked *unstable*. It's tied to one wgpu major version, so a Slint
    upgrade can mean bumping wgpu and fixing small API changes. wgpu 30, for example, made several
    pipeline fields `Option`s.
- **Roblox UV layout:** see [UV_MAPPING.md](UV_MAPPING.md).
- **PNG export:** the `image` crate writes 8-bit RGBA PNGs. We check the 585 × 559 size before
  every save.

## Data flow

```
Document { base template, layers[source image, part, face, center, width, opacity] }
        │  compose()  (deterministic, CPU)
        ▼
  composed 585×559 RGBA ──► GPU texture ──► 3D avatar (wgpu → Slint Image)
                        ├─► 2D template view (Slint Image)
                        └─► PNG export
```

All three outputs come from the same `compose()` result, so the 3D view, the 2D view and the
export can't drift apart.

## Modules

| Module | Responsibility | Depends on |
|---|---|---|
| `template` | Region map, presets, diagnostic | image |
| `compose` | Document model, compositing | template |
| `avatar` | Rig geometry, UV mesh, raycast, camera | template, glam |
| `render` | GPU pipeline, offscreen rendering | avatar, slint's wgpu |
| `main` | UI wiring, file dialogs | all |

`template`, `compose` and `avatar` don't depend on the UI or the GPU, so they're unit tested
headless.

## Placement

1. A ray from the camera through the cursor is tested against the rig's boxes. The nearest hit
   gives a part, a face, and a position on that face (0..1 across and down, from the top-left).
2. The layer stores that face position, so placement is resolution-independent and defined per
   surface.
3. `compose()` resizes the source with premultiplied alpha (so edges don't get dark fringes). It
   then draws it into the region of the layer's *home face*.
4. **Seam wrapping** (per layer, on by default) is done face by face. For every texel of the
   faces next to the home face on the same body part, we find its 3D point on the part's box. We
   then unfold that point flat into the home face's plane, like a box net: the point keeps its
   position along the shared edge, and its distance from the edge carries on past the home face's
   border. The art is then sampled at that unfolded point. Every face is 64 px per stud, so
   distances carry straight across. Tests check that pixels on either side of a seam map to
   neighbouring points, on both the torso and the sleeves.
5. Picking (`layer_at`) uses the same mapping, so wrapped parts of the art can be clicked too.

The 2D view uses the same face coordinates: template pixel → `template::locate()` → part, face
and position on the face.

## Undo / redo

Every edit pushes a snapshot of the document (base template, layers, selection) before changing
it. Source images are `Rc`-shared, so a snapshot copies small structs, not pixels. Edits of the
same kind in a row form one undo step: one slider drag, a run of arrow-key nudges, or one art drag
(recorded on the first movement). History is capped at 200 steps.

## Known shortcuts (grep `shortcut:`)

- Wrapping only reaches faces that share an edge with the home face. It doesn't go around corners
  onto a second face, or onto the opposite face. Art doesn't wrap between separate body parts
  either, which matches Roblox, where torso and arms are separate meshes.
- New GPU render targets are created on every redraw. Redraws only happen on demand, so this is
  fine for now.

## Remaining technical risks

1. ~~The UV layout isn't verified in Roblox Studio yet.~~ Checked in Studio on 2026-11-12 and
   it looked right (see UV_MAPPING.md). Re-check after any change to
   `template::region()` or `avatar::face_frame()`.
2. The Slint wgpu integration is marked *unstable* (see above).
3. **Seam wrapping on R15.** The unfolding works on the R6 boxes. The R15 blocky rig's bevelled
   edges show the seam slightly differently from a sharp box edge.
4. **Sampling.** The 3D preview uses nearest-pixel sampling. Roblox may filter differently, so
   small details can look slightly different in-game.
5. **Skin color.** The avatar's skin tone is a fixed constant. Real avatars vary, and that changes
   how transparent short-sleeve areas look.
6. **R15 rigs.** R15 avatars wrap classic shirts onto different meshes. Placement verified on R6
   may stretch on R15.
7. **GPU fallback.** Machines without DX12 or Vulkan won't start. Slint's software renderer could
   be a fallback, but the 3D view would need a CPU rasterizer.

## Recommendations for Milestone 1

1. ~~Get the Studio check done first.~~ Done (2026-11-12).
2. ~~Placement on every face~~, ~~layers panel~~, ~~undo/redo~~, ~~2D dragging~~ and ~~seam
   wrapping~~ are done.
3. Grayscale, brightness and contrast per layer.
4. Save and load JSON projects. Store source images as files next to the project and keep them
   unmodified.
5. Pasting artwork from the clipboard (the `arboard` crate).
6. A hover highlight for the face under the cursor.
