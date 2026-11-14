# Image Quality & Rendering Pipeline Audit

Audit of Clothing Studio 1.0.3 (commit `b157fe5`). Audit only: no source changes, no
benchmarks were run. Every "confirmed" item cites code; "risk" items are hypotheses that the
tests in §8 should settle.

## A. Current architecture

| Concern | Implementation |
|---|---|
| GUI | Slint 1.18 (`renderer-femtovg-wgpu`, `unstable-wgpu-30`), `ui/app.slint` |
| GPU | wgpu 30 borrowed from Slint; offscreen avatar render in `src/render.rs` |
| Image I/O | `image` 0.25 (png, jpeg, webp only), 8-bit `RgbaImage` everywhere |
| Compositing | CPU, `compose::compose()` in `src/compose.rs`, straight-alpha `f32` blend per pixel |
| Template | `src/template.rs`: hard-coded 585×559 face rects, 64 px/stud on every face |
| 3D preview | `src/avatar.rs` box mesh with template UVs + `render.rs` shader |
| AI upscaler | `src/upscale.rs`: Real-ESRGAN **general x4v3** (SRVGG compact, ~5 MB), Qualcomm AI Hub ONNX float export, via `ort` 2.0-rc.15, CPU session from `bgremove::load_session` |
| BG removal | `src/bgremove.rs`: ISNet at 1024², mask resized back with Lanczos3 |
| Project files | `src/project.rs`: JSON with originals as lossless base64 PNG, de-duplicated |
| Export | `main.rs::save_png` → `image` PNG encoder, size asserted 585×559 |
| Tests | Unit tests in `compose`, `template`, `upscale`, `bgremove`, `project`; `#[ignore]`d perf and quality-sheet tests (`compose.rs` `perf`, `quality_preview`, `perf2`; `upscale.rs` `real_model_sharpens_a_small_picture`). Fixtures: `docs/test_art.png`, `docs/quality_comparison.png`. No objective quality metrics. |

### Pipeline as traced

```
import  image::open().to_rgba8()                         main.rs:1674, 2057, 2155
  │     Layer::new → source: Rc<RgbaImage> (never mutated)  compose.rs:197
  ▼
work    working_copy(): premultiply in u8, then if >1024 px  compose.rs:632
        Triangle-resize to 1024 max side
        (source, or ×4 upscaled copy, with optional BG mask) effective_source() :229
  ▼
scale   scaled_art(): resize work → exact on-template w×h     compose.rs:552
        Sharp=Lanczos3+unsharpen(0.6,1) | Smooth=Triangle | Pixel=Nearest
        then flips, then invert/grayscale/brightness/contrast; LRU cache of 32
  ▼
place   draw_layer(): for every template texel of each reachable face,
        inverse-map to layer-local coords, NEAREST lookup     compose.rs:525-531
        into the scaled art, blend() straight alpha × opacity
  ▼
overlay shading::apply over everything; tattoo underlay below base
  ▼
585×559 RgbaImage  ──► 2D view: Slint Image, `image-rendering: pixelated` (app.slint:471)
                   ├─► 3D: write_texture to Rgba8Unorm, default sampler (Nearest, no mips)
                   └─► PNG export (lossless, 8-bit RGBA)
```

All three outputs come from the same `compose()` result, so preview/export drift is
structurally impossible for the texture itself.

## B. Confirmed quality issues

1. **Rotated art is point-sampled (jagged edges).** `draw_layer` (`compose.rs:528-529`) does
   `scaled.get_pixel(lx as u32, ly as u32)`: the art is resampled well *once* at axis-aligned size,
   then a rotation is applied with nearest-neighbour lookup. Any angle that isn't a multiple of 90°
   gets stair-stepped edges and dropped/duplicated texels. Unrotated art is unaffected apart from
   sub-pixel position snapping (≤0.5 px).
2. **All resampling happens in gamma (sRGB) space.** `working_copy`, `resize_premultiplied`,
   `bgremove`'s mask resize and the upscaler's alpha resize all filter the encoded 8-bit values.
   Downscaling by 4–10× (a 1024 px picture to a 77–128 px chest graphic is typical) darkens thin
   bright lines on dark backgrounds and lightens thin dark lines on light ones. Gothic white-on-black
   linework is the worst case. How visible it is has not been measured.
3. **Premultiplication is done in 8 bits** (`compose.rs:635-638`, unpremultiply at `:667-673`).
   At low alpha the colour collapses to a few levels (alpha 10 → 11 possible values per channel),
   so soft edges, glows and feathered BG-removal masks can posterize or shift colour.
4. **The AI upscaler usually can't change the export.** Final art on a torso front is at most
   128 px wide (`template.rs:104`, `Layer::default_width` = 0.6 → ~77 px). The upscaled copy is
   capped back to 1024 px by `working_copy` (Triangle), then shrunk to that size again. Upscaling
   only adds information when the source has **fewer pixels than its drawn size**. A 300 px
   Pinterest image drawn at 77 px is already downsampled ~4×, so ×4 upscaling it just feeds a
   bigger image into the same shrink. This is the main reason for "little visible improvement":
   the cause is downsampling afterwards, not a model or implementation bug. The only real benefit
   left for mid-sized sources is the model's denoising and JPEG-artefact cleanup.
5. **Upscaler sees hidden colours under transparency.** `upscale_tiled` (`upscale.rs:~120`)
   feeds RGB of fully transparent pixels (often black or arbitrary) to the network. Convolution
   bleeds them into visible edge pixels, while alpha is Lanczos-upscaled separately, so the two
   edges don't match exactly. Premultiplying afterwards limits it, but dark or coloured fringes on
   transparent PNGs are likely.
6. **3D preview is not representative of in-game filtering.** `render.rs:101`
   `SamplerDescriptor::default()` = nearest mag/min, `mip_level_count: 1`, `sample_count: 1`.
   Zoomed out, the avatar shimmers and aliases; zoomed in, it looks blockier than Roblox (which
   filters bilinearly with mips). Lighting is done in sRGB numbers on an `Rgba8Unorm` target.
   This affects the preview only, never the export; ARCHITECTURE.md already lists it as risk 4.

## C. Potential quality risks (not verified)

- **Sharp mode halos on opaque interiors.** Lanczos3 ringing plus `unsharpen(0.6, 1)`. The
  `colour ≤ alpha` clamp only prevents halos against *transparency*. Photos with hard internal
  edges may get light/dark rims. The existing test (`sharp_adds_no_dark_halo_on_transparent_edges`)
  only covers the transparency case.
- **Double resample for big sources:** Triangle to 1024, then Lanczos to target. With ≥8× total
  reduction this is near-lossless, so low risk.
- **No EXIF orientation:** `image::open` doesn't apply it, so phone JPEGs may import sideways.
- **No ICC handling:** Display-P3/Adobe RGB images are treated as sRGB, so saturated colours shift.
- **16-bit PNGs** are truncated to 8 bits at import. This is harmless for an 8-bit export.
- **Upscaled-mask mismatch:** the BG mask is made at source size and Triangle-stretched ×4
  (`compose.rs:237`), which gives softer cut-outs than masking the upscaled image directly.
- **Unpremultiplied blend in sRGB** (`blend`, `compose.rs:613`). This matches how Roblox composites
  in practice, so it is probably *correct* for fidelity and shouldn't be "fixed" to linear without
  an in-Studio comparison.

### Verified as **not** problems

- **Originals preserved:** `Layer::source` is never written (test `source_is_preserved`, and
  `adjustments_apply_to_output_not_source`). Projects store originals losslessly (`project.rs:2`).
- **No cumulative degradation:** transforms are parameters (`center`, `width`, `stretch`,
  `rotation`, flips) re-applied from `work` every compose. Dragging uses a cache proven equal to a
  fresh render (`cached_drag_matches_fresh_render`).
- **Template geometry is exact:** integer rects, tested for size, bounds and overlap
  (`template.rs` tests). User templates are only accepted at exactly 585×559 (`main.rs:1452`) and
  are used unresampled. Studio check done 2026-11-12.
- **3D UVs = export:** the 3D texture is the composed export image itself, so there is no
  separate atlas.
- **Seam wrapping** maps neighbouring faces with an affine map at 64 px/stud. For unrotated art
  that map is a translation or flip, so nearest lookup is exact there. Continuity is tested.
- **Export** is lossless 8-bit PNG, size-checked.

## D. Recommended architecture

Keep the current design. It is already non-destructive and resolution-independent. Fix the
**sampling** rather than the **resolution**:

```
source (untouched) ─► [optional: AI ×4, only when drawn size > source size]
     ─► work copy: sRGB→linear, premultiplied, f32 (Rgba32F), capped at max(1024, 4×drawn)
     ─► scaled_art at exact drawn size (linear, premultiplied, quality filter)   [cached]
     ─► draw_layer: inverse affine map + FILTERED sample
            (bilinear for scale≈1; 2×2–4×4 supersample only for non-90° rotation)
     ─► linear→sRGB, straight alpha, 8-bit
     ─► blend into 585×559 (sRGB, as today, to match Roblox)
     ─► export / 2D / 3D (3D: linear filter + mips + MSAA, preview only)
```

Why not a 4× internal canvas: the export is 585×559, and every template texel already maps 1:1.
For unrotated art, the current direct "source → exact size" resize is the best possible result.
Rendering at 2340×2236 and box-downsampling would be *softer* than a Lanczos resize straight to
size. The one thing supersampling fixes, rotation aliasing, can be fixed locally inside
`draw_layer` at a fraction of the cost.

## E. AI upscaling assessment

| Property | Value (from `upscale.rs`) |
|---|---|
| Model | Real-ESRGAN `realesr-general-x4v3` (SRVGGNetCompact), Qualcomm AI Hub v0.66.0, float ONNX, BSD-3 |
| Runtime | ONNX Runtime via `ort`; CPU (no execution provider configured) |
| Input | 1×3×128×128 RGB float 0..1, NCHW; output 512×512 |
| Factor | fixed ×4 |
| Tiling | 128 tiles, 12 px context discarded, border-clamped; tested seam-free |
| Alpha | not modelled; Lanczos3 ×4 separately |
| Colour | raw sRGB bytes /255, no conversion (correct for this model) |
| Limits | refused above 1024 px side (`MAX_SIDE`) |
| After inference | Triangle-capped to 1024 by `working_copy`, then shrunk to drawn size |

Suitability (based on the model's published training scope, not tested here):

| Content | Fit |
|---|---|
| Photos, compressed Pinterest JPEGs | Good: built for real-world degradations, denoises |
| Low-res illustrations | OK |
| Logos, flat graphics, text | Fair: may round corners and soften flat fills; "Pixel"/"Sharp" without AI often wins |
| Anime / line art | Fair: an anime model (`realesr-animevideov3`, also compact; or `RealESRGAN_x4plus_anime_6B`) preserves lines better |
| Gothic intricate patterns | Fair: fine filigree can be hallucinated or smoothed |
| Pixel art | Poor: should never be AI-upscaled; use Pixel quality |

**Verdict:** keep the model. It is small, fast and permissively licensed. Change *when* it is
used (point 4 in §B), fix the transparency bleed (point 5), and optionally add an anime/line-art
model. 4x-UltraSharp (ESRGAN RRDB, ~67 MB, slow on CPU, community licence that needs checking) and
SwinIR (transformer, heavy) cost a lot on integrated GPUs. They also can't fix the
downsample-after problem, so they're not recommended.

## F. Prioritized improvements

| # | Pri | Change | Benefit | Complexity | Perf | Regression risk |
|---|---|---|---|---|---|---|
| 1 | P0 | Quality test harness (§8) | Makes 2–5 measurable | Low | none | none |
| 2 | P0 | Filtered sampling in `draw_layer` for rotated art (supersample only when `rotation % 90 ≠ 0`) | No jagged rotated edges | Low–Med | ~4–16× on rotated layers only; cached scale unaffected | Pixel-exact tests for unrotated must still pass |
| 3 | P1 | Linear-light, f32 premultiplied resampling in `working_copy`/`resize_premultiplied` | Correct thin-line brightness, clean soft edges, no 8-bit premult posterization | Med | `Rgba32F` work copy 16 MB per 1024² layer; resize ~2–3× slower but cached | Colours of existing projects shift slightly; keep a "legacy" path or accept |
| 4 | P1 | Upscale gating: offer/apply only when drawn px > source px; show "effective resolution" (source px per template px) in the UI | Stops a misleading feature; tells users when a source is too small | Low | saves work | none |
| 5 | P1 | Upscaler edge bleed: fill transparent RGB with nearest opaque colour (dilate) before inference; mask the upscaled image directly | Removes fringes on transparent PNGs | Low | negligible | Upscale outputs change; tiling test still valid |
| 6 | P1 | 3D preview: linear sampler + mip chain + 4× MSAA; shader mix on premultiplied texels | Preview closer to in-game, no shimmer | Med | small on any DX12 iGPU | Hover-pick unaffected (CPU raycast) |
| 7 | P2 | EXIF orientation on import | Phone photos upright | Low | none | none |
| 8 | P2 | Sharp mode: reduce/skip unsharpen when downscale ratio < ~2, or make strength a setting | Fewer halos on photos | Low | none | `sharp_has_more_edge_contrast_than_smooth` threshold |
| 9 | P2 | Optional anime/line-art upscale model | Better line art | Med (download + model choice) | same | none |
| 10 | P2 | Non-destructive crop + editable mask brush | Workflow, not raw quality | Med–High | none | project format bump |
| 11 | P2 | ICC conversion to sRGB (new dependency, e.g. `qcms`) | Correct colours for P3 sources | Med | import only | dependency |

## G. Implementation milestones (proposed, not started)

**M1 – Quality harness (P0)**
- Objective: objective, reproducible numbers before any change.
- Files: new `tests/quality.rs` or `compose.rs` test module; fixtures in `tests/fixtures/`.
- Changes: synthetic fixtures generated in code (stripes, 1-px white lines on black, disc with soft
  alpha, checker pixel art, wide graphic spanning torso+sleeves) plus 3–4 real sample PNGs.
  Metrics: mean gradient magnitude (sharpness), max fringe darkness at alpha edges, mean luminance
  of thin-line regions vs ideal (gamma-correctness), PSNR/SSIM vs a 16× supersampled f32 reference
  render, idempotence after 50 random move/scale/rotate round-trips back to the start, rotation
  edge stair-step count.
- Deps: none (implement PSNR/SSIM in a few dozen lines).
- Acceptance: baseline numbers recorded in the repo; all current tests still pass.
- Regression tests: the harness itself. Rollback: delete test module.

**M2 – Filtered placement (P0)**
- Files: `compose.rs::draw_layer`.
- Changes: for non-90° rotation, average N×N sub-samples per texel (bilinear lookup into
  `scaled`); for 0/90/180/270, keep the current exact path.
- Acceptance: stair-step metric and PSNR vs reference improve on rotated fixtures; unrotated
  outputs byte-identical; `drag_frame_cost` within 2× on rotated layers.
- Rollback: one function; gate behind a const.

**M3 – Linear f32 resampling (P1)**
- Files: `compose.rs` (`working_copy`, `resize_premultiplied`, `adjust` order), `upscale.rs`
  alpha path.
- Changes: `Rgba32FImage` work copies (sRGB→linear, premultiply in f32), resize in f32, convert
  back at `scaled_art`. Keep `blend` in sRGB to match Roblox.
- Acceptance: thin-line luminance error and soft-edge banding improve in M1; transparency
  tests pass; memory ≤ ~16 MB per large layer.
- Regression: existing projects render slightly differently. Compare M1 sheets and do one Studio
  upload check. Rollback: feature const.

**M4 – Upscaler correctness & gating (P1)**
- Files: `upscale.rs::upscale_tiled`, `main.rs` upscale handler (~1755), `app.slint`.
- Changes: dilate opaque colour into transparent pixels before inference; enable/recommend only
  when the drawn size exceeds the source; show effective resolution; mask applied after upscaling.
- Acceptance: no fringe on the transparent-disc fixture; tiling test still green.
- Rollback: independent of M2/M3.

**M5 – 3D preview fidelity (P1)**
- Files: `render.rs` (sampler, texture mip levels + generation, MSAA target + resolve, shader).
- Acceptance: GPU readback of a flat front-facing view at 1:1 matches the export texels within
  filter tolerance; no shimmer zoomed out; still runs on an Intel iGPU.
- Rollback: sampler/MSAA consts.

**M6 – Import hygiene (P2):** EXIF orientation (`image::ImageDecoder::orientation` exists in
0.25), optional ICC. **M7 – crop/mask editing, anime model (P2).**

## 8. Image-quality testing strategy

| Test | Kind | Method |
|---|---|---|
| Sharpness | Objective | mean Sobel magnitude inside the art box vs a 16× supersampled f32 reference |
| Alpha-edge artefacts | Objective | over black and over white bases, no edge pixel outside the [source colour, base] range |
| Scaling consistency | Objective | same art at widths 0.2..1.0, PSNR vs reference stays above a floor |
| Repeated transforms | Objective | 50 random edits returned to start must equal the original compose byte-for-byte (already true by design, so lock it in) |
| Colour accuracy | Objective | flat colour patches unchanged ±1; grey-ramp downscale vs linear reference |
| 3D vs export | Objective | wgpu readback of an orthographic front view, sample texel centres, compare to export |
| Template alignment | Objective | existing region tests + diagnostic texture (`template::diagnostic`) |
| Export fidelity | Objective | PNG round-trip equals composed buffer |
| Visual look | Subjective | extend `quality_preview` to write side-by-side sheets per fixture; judge in-app and once in Studio |

The fixtures listed in the brief (gothic hi-res, low-res Pinterest JPEG, sharp-edged transparent
PNG, cloth texture, pixel art, multi-face graphic) should live in `tests/fixtures/` with their
licence noted. Only `docs/test_art.png` exists today.

## 9. Performance notes

CPU compositing is the right choice: the target is 585×559 (0.33 MP), placement is cached per
(work, size, quality, flips, adjustments), and only `draw_layer`'s per-texel loop runs on drags.
GPU compositing would add a second implementation without a quality gain. M2's supersampling
only applies to rotated layers. M3's f32 work copies are the main memory cost (16 MB per 1024²
layer instead of 4 MB). Capping at 1024 keeps that bounded. Dirty-region rendering isn't needed
at this canvas size. The 3D changes in M5 (mips + 4× MSAA on a ~viewport-sized target) are
cheap even on integrated GPUs. Export speed is dominated by PNG encoding of 0.33 MP, which is
negligible.

## H. Final answers

1. **Is the 585×559 pipeline causing quality loss?** Not because of its resolution. That is
   the export size, and art is resampled straight to it. Loss comes from nearest-neighbour
   placement of rotated art, gamma-space resampling and 8-bit premultiplication.
2. **Are originals preserved?** Yes, in memory and in project files (lossless PNG).
3. **Would non-destructive layers improve quality?** The editor already is non-destructive
   for transforms and adjustments. Adding crop/mask editing helps workflow, not pixel quality.
4. **Would 4× internal rendering improve the export?** Only for rotated art, and filtered
   sampling inside `draw_layer` gets the same result for much less cost. For unrotated art it
   would make output slightly softer. Not recommended as a global canvas scale.
5. **Is the AI upscaler appropriate?** Yes for small photos and JPEGs. It is used where it can't
   help (sources already larger than their drawn size), and it bleeds colour at transparent edges.
   Keep it, gate it, fix the edges. Optionally add an anime/line-art model.
6. **Single highest-impact improvement?** For everyday unrotated graphics: **linear-light,
   f32 premultiplied resampling (M3)**. It touches every layer and the thin-line gothic case.
   Confirm with the M1 harness first. If users rotate art often, M2 comes first.
7. **Without rebuilding the application?** Yes. M2–M5 are local changes to `compose.rs`,
   `upscale.rs` and `render.rs`. The document model, project format and template code stay as
   they are.
