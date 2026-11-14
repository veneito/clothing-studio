# Image-quality baseline (Phase 1 / audit M1)

Recorded 2026-11-14 on commit `b157fe5` (1.0.3) with rendering unchanged. The harness is
[src/quality.rs](../../src/quality.rs).

Reproduce:

```bash
CS_QUALITY_DIR=docs/quality/baseline cargo test --release quality_baseline -- --ignored --nocapture
```

Images in [baseline/](baseline/) are torso-front crops enlarged 4× with nearest-neighbour scaling.
`ideal_rotated.png` is the analytic reference.

## Fixtures

| Fixture | Setup | Ideal |
|---|---|---|
| Rotated | 512² opaque red square, width 0.6, 30°, on white | Exact area coverage (16×16 subsamples), mixed in linear light |
| Thin lines | 1024² black, 4 px white lines every 64 px, drawn 128 px wide (8× down, lines 0.5 px) | Linear mean = 1/16 = 0.0625 |
| 27× downscale | 2048² alternating 1 px black/white columns, drawn ~77 px | Flat linear 50 % grey = sRGB 188, std 0 |
| Soft edge | 256² red disc, alpha fades to 0 over 40 px, on white and on black | Every pixel exactly on the line from the base colour to red (fringe 0) |

## Results

| Quality | Rotated PSNR dB | Rotated edge MAE | Lines lin. mean (ideal 0.0625) | 27× mean sRGB (ideal 188) | 27× std | Soft/white fringe max, mean | Soft/black fringe max, mean | compose ms* |
|---|---|---|---|---|---|---|---|---|
| Sharp | 29.22 | 36.99 | 0.0349 | 128.0 | 0.00 | 2.4, 0.35 | 1.2, 0.37 | 11.0 |
| Smooth | 29.22 | 36.99 | 0.0166 | 128.0 | 0.00 | 1.0, 0.28 | 0.9, 0.36 | 9.9 |
| Pixel | 29.22 | 36.99 | 0.0000 | 128.0 | 0.00 | 0.4, 0.25 | 0.7, 0.34 | 8.9 |

\* The rotated fixture: one compose call plus the 512² resize (cold cache), release build.

## What the numbers show

- **Rotation (audit B1 confirmed).** Edge pixels are off by ~37/255 on average, and the result
  is identical for all three quality modes, because rotation is a nearest-neighbour lookup after
  resizing. The image shows plain stair-steps with no anti-aliasing.
- **Thin lines (B2 confirmed).** Sharp keeps only 56 % of the lines' light energy, and Smooth 27 %.
  **Pixel drops every line completely** (0.0000): nearest-neighbour skips the line columns at 8×
  down. White gothic linework comes out as dim grey, or disappears.
- **Large downscale (B2 confirmed).** A 50 % linear grey becomes sRGB 128 instead of 188, so the
  result is ~32 % too dark. Std 0 means there is no moiré: the 1024 px Triangle prefilter averages
  the stripes before the final resize.
- **Soft edges.** The fringe is small (≤2.4/255) and only on Sharp. 8-bit premultiplication (B3)
  is measurable but minor, so it's a low priority compared with the two items above.

## Invariant tests (run on every `cargo test`)

- `export_is_template_size_and_full_face_art_aligns_exactly`: 585×559; full-face art fills the
  torso front exactly with nothing bleeding into the 2 px gaps.
- `png_export_round_trips_losslessly`.
- `repeated_transforms_do_not_degrade`: after 50 random move/scale/rotate/stretch edits, restoring
  the start values gives byte-identical output.

All 64 tests pass (7 ignored perf/model/report tests).

## Phase 2: filtered rotation (2026-11-14)

`draw_layer` now uses a premultiplied bilinear lookup for off-grid rotations (`compose.rs`
`sample_bilinear`). Right angles and Pixel quality keep the exact nearest-neighbour path, so
unrotated output is byte-identical. Images: [phase2/](phase2/).

| Quality | Rotated PSNR dB | Edge MAE (linear ideal) | Edge MAE (sRGB-mixed ideal: geometry only) | Warm rotated compose |
|---|---|---|---|---|
| Sharp | 29.22 → **35.49** | 36.99 → **20.36** | 29.55 → **3.04** | 0.63 → 0.80 ms |
| Smooth | 29.22 → **35.49** | 36.99 → **20.36** | 29.55 → **3.04** | 0.63 → 0.82 ms |
| Pixel | unchanged (by design) | 36.99 | 29.55 | unchanged |

The rest of the linear-ideal edge error is gamma: coverage is mixed in sRGB. That is Phase 3.
Other columns (thin lines, downscale, soft edge) are unchanged, as expected.
`perf2::drag_frame_cost` (unrotated, 3 layers) is within run-to-run noise: Sharp 5.7 → 5.8 ms, Smooth 2.7 → 2.8 ms, Pixel 1.3 → 1.4 ms.
65 tests pass (new: `rotated_edges_are_antialiased`).

## Phase 3: linear-light f32 resampling (2026-11-14)

- `Layer::work` is now `Rgba32FImage`: linear light, premultiplied, f32 (`working_copy`).
  Sources over 2048 px are area-averaged in linear light by a whole factor while converting, so
  no full-size f32 copy is made, then Triangle-filtered to ≤1024.
- `resize_premultiplied` filters in linear f32. 8-bit sRGB happens once, at the final drawn size
  (`to_display`).
- Sharp: Lanczos3 → **Catmull-Rom**, and the unsharp mask now runs in perceptual (sRGB-encoded)
  premultiplied space. Lanczos3 in linear light left visible grey ringing bands two and five
  pixels from thin bright lines on black (sRGB 39 and 19; they were 3 and 1 before). Catmull-Rom
  gives 0. I also tried sharpening in linear light, which made bright halos too.
- Layer blending is still sRGB (`blend`), to match how Roblox composites. Adjustments
  (invert/grayscale/brightness/contrast) still act on the final sRGB pixels, as before.
- Project format unchanged; `project.rs` only builds the new work-copy type on load.

| Quality | Lines lin. mean (ideal 0.0625) | 27× mean sRGB (ideal 188) | Soft/white fringe max | Rotated edge MAE (linear ideal) |
|---|---|---|---|---|
| Sharp | 0.0349 → **0.1002** (crisper than ideal, by design; no ringing) | 128 → **188** | 2.4 → **1.0** | 20.36 (unchanged: blend stays sRGB) |
| Smooth | 0.0166 → **0.0621** | 128 → **188** | 1.0 → **0.4** | 20.36 |
| Pixel | 0.0000 (nearest skips 0.5 px lines, by definition) | 128 → **188** | 0.4 | 36.99 |

Thin-line row of pixels (sRGB, black background), one line period:

| | before | after |
|---|---|---|
| Smooth | `0 … 0 32 96 0` | `0 … 0 99 165 0` |
| Sharp | `0 3 0 0 1 0 30 141 0` | `0 0 0 0 0 0 102 215 0` |

Images: [phase3/](phase3/) (thin lines, 27× downscale, soft edges) and
`test_art_before.png` / `test_art_after.png` (Sharp | Smooth | Pixel on `docs/test_art.png`;
the old Sharp light rim is gone, otherwise near-identical on this flat graphic).

Performance (release):

| | Phase 2 | Phase 3 |
|---|---|---|
| `drag_frame_cost` 3 layers, Sharp / Smooth / Pixel | 5.8 / 2.8 / 1.4 ms | 3.8 / 2.6 / 1.4 ms |
| Warm rotated compose | 0.80 ms | 0.73 ms |
| `working_copy` of a 4000×3000 picture (once per add/mask toggle) | not measured | 66 ms |
| Work copy memory, 1024×768 layer | 3 MB | 12.6 MB |

65 tests pass. Existing projects load unchanged but render slightly differently: thin and fine
detail is brighter, now at the correct brightness.

## Phase 4: upscaler transparency (2026-11-14)

`upscale.rs::upscale_tiled` now runs on a copy where every fully transparent pixel takes the RGB
of its nearest visible pixel (`bleed_colours`, breadth-first). The model is unchanged
(Real-ESRGAN general x4v3), tiling is unchanged, and alpha is still the original transparency
enlarged ×4 with Lanczos3. Before, whatever colour was hidden under transparency (usually black)
was fed to the network and smeared into the visible edges.

Real model, 64 px anti-aliased red disc (220,20,20) on transparent black, ×4:

| | Semi-transparent edge pixels | Max colour drift | Mean drift | Darkened (R < 180) |
|---|---|---|---|---|
| Before | 7376 | 220 | 95.6 | **3321** |
| After | 7376 | **3** | **3.0** | **0** |

Image: [phase4/upscale_edge_before_after.png](phase4/upscale_edge_before_after.png) (on white,
before | after). New test `hidden_colours_under_transparency_do_not_fringe_the_edges` uses a
blurring stand-in model. It fails without the fix (fringe 147,13,13) and passes with it.
Cost: one extra pass over the source (≤1024², a few ms) per upscale; inference time unchanged
(~55 ms for this picture). 66 tests pass.

Not changed: the background-removal mask is still made at source size and stretched ×4
(Triangle) onto the upscaled copy. The upscaler still sees the photo's real background next to
the subject, so a cut-out edge can carry some background colour. That was already true without
upscaling, and fixing it would mean re-running background removal on the 4× image.

## Phase 5: 3D preview filtering and anti-aliasing (2026-11-14)

Preview only (`render.rs`). `compose()` output, the 2D view and the export are untouched; only the
GPU copy made in `Renderer::upload` changes.

- Sampler: nearest → **trilinear + 4× anisotropic**, the way Roblox draws clothing.
- Textures are uploaded **premultiplied** with **3 mip levels** (585 → 292 → 146, each a 2×2
  average in linear light). The shader composites with `col·(1−a) + rgb`, so filtering never
  pulls in colours hidden under transparent pixels. Mips stop at 3 so faces don't blend across
  the 2 px template gaps.
- **4× MSAA** on geometry edges (multisampled colour and depth, resolved into the same texture
  Slint shows).
- New test `mips_are_premultiplied_and_ignore_hidden_colours`; ignored
  `render::tests::preview_report` renders the avatar wearing `docs/test_art.png` to
  `$CS_QUALITY_DIR` and times it (needs a GPU).

Images: [phase5/](phase5/) `cmp_*.png` (before | after; small renders enlarged with nearest).
At 800 px the silhouette and diagonal texture edges are smooth instead of stair-stepped; at
160 px from far away the shirt no longer breaks into jagged pixels.

GPU time per frame (50 frames, this PC, includes per-frame target allocation):

| Viewport | Before | After |
|---|---|---|
| 800×800 | 0.43 ms | 0.59 ms |
| 400×400, side view | 0.36 ms | 0.42 ms |
| 200×200 | 0.37 ms | 0.36 ms |
| 160×160, far | 0.36 ms | 0.38 ms |

Memory: the 4× colour and depth targets add 32 bytes per viewport pixel (≈20 MB at 800×800,
allocated per frame as before); mips add a third to the two 1.3 MB clothing textures. Upload
now builds the mips on the CPU on every edit (not timed; two 585×559 passes plus two small ones).

67 tests pass (8 ignored). Not checked in the running app: the change only affects the
offscreen texture Slint already displays, but nobody has looked at it on screen yet.
