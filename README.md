# Clothing Studio

A lightweight native Windows editor for Roblox **classic shirts**. Place artwork on a 3D block
avatar and get a correctly laid-out 585 × 559 shirt template. The 3D view and the 2D template are
both drawn from the same composed texture that gets exported.

Status: **Milestone 0 (technical validation)**. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Download

Get **`Clothing.Studio_<version>_x64-setup.exe`** from the
[Releases](https://github.com/veneito/clothing-studio/releases) page and run it. It installs for
your Windows user (no admin rights): Start menu and desktop shortcuts, `.clothing` project files,
and an entry in **Settings → Apps** to uninstall. Your projects, templates and settings are kept if
you uninstall.

**Making a release:** bump `version` in `Cargo.toml`, commit, then tag and push:
`git tag v1.0.1 && git push origin v1.0.1`. The **Release** workflow tests, builds the installer and
publishes it. **Actions → Test build → Run workflow** builds an installer to try out without
publishing it.

## Install from source (Windows)

Double-click **`install.cmd`**. It builds the release version and installs it for your Windows
user, with no admin rights needed:

- The app goes in `%LOCALAPPDATA%\Programs\Clothing Studio`.
- **Start menu** and **desktop** shortcuts are added.
- **`.clothing` project files** open in Clothing Studio when double-clicked.

Run `install.cmd` again to update after pulling changes (close the app first). `uninstall.cmd`
removes the app, its shortcuts and the file type, and keeps your projects and your
templates/settings in `%APPDATA%\clothing-studio`.

Requirements: Rust (to build), and the Microsoft Visual C++ runtime, which most PCs already have.
The installed app needs `DirectML.dll` next to it; the installer copies it there.

## Build

Requirements: Windows 10/11, Rust 1.85+ (developed on 1.98.1, MSVC toolchain), a GPU with
DirectX 12 or Vulkan.

```bash
cargo build --release
cargo run --release
```

Run the tests:

```bash
cargo test
```

Command-line helpers:

| Command | What it does |
|---|---|
| `clothing-studio <art.png>` | Start with artwork loaded, ready to place |
| `clothing-studio --sample out.png` | Write a sample shirt (white short sleeve + badge) |
| `clothing-studio --diagnostic out.png` | Write a color-coded test template for checking in Roblox Studio |

## Using it

1. Pick a template from the dropdown: one of the built-in presets or one of your saved templates.
   **Load template…** opens any 585 × 559 PNG and saves a copy into your template library.
   **Save as template…** saves the current design (template plus layers) into the library as a
   new template. **Delete template** removes the selected saved template (after a confirmation;
   presets can't be deleted). Saved templates live in `%APPDATA%\clothing-studio\templates`. The app reopens
   your last template and last rig on start.
2. **Import artwork…** (PNG, JPG or WebP; transparency is kept), or **Ctrl+V** / **Paste** to add
   an image from the clipboard: "Copy image" in a browser, a screenshot, or image files copied in
   File Explorer. Each one becomes a new layer on the chest.
3. Drag the artwork anywhere on the shirt, in the 3D or the 2D view (in 3D, the face under the cursor is tinted blue): front, back, sides, top,
   bottom, and both sleeves. Clicking a piece of art selects its layer.
   In the 2D view the selected art has handles: drag a corner to resize it, hold **Shift** to keep
   its proportions, or drag a side to stretch it one way. The opposite corner or side stays put.
4. In the **Layers** panel: show or hide, reorder (▲/▼), Duplicate, Delete. Double-click a layer to rename it (Enter saves, Esc cancels). **Properties** has
   size, rotation, opacity, brightness, contrast, grayscale, invert, flip H/V and **Remove background (AI)**. Adjustments only change
   the result; your original images are never modified.
5. **Export PNG…** writes the 585 × 559 shirt, without the selection outline.

Shortcuts: Ctrl+Z undoes, Ctrl+Y or Ctrl+Shift+Z redoes, Delete removes the selected layer,
Ctrl+D duplicates it, and the arrow keys nudge it by 1 px (10 px with Shift).

**Wrap edges** (on by default) continues art that crosses a seam onto the neighbouring faces of
the same body part. For example, a logo near the chest's edge carries on round the side. Turn it
off to cut the art at the face edge instead.

**Across front: torso + sleeves** (in Pants mode, torso + legs): for one big graphic across the
whole front. Art on the front (or back) of the torso continues flat onto the fronts (or backs) of
both sleeves, lined up as on the avatar, and nothing goes onto the sides. It replaces Wrap edges
for that layer.

**3D view (Blender-style):**

| Input | Action |
|---|---|
| Middle-drag | Orbit |
| Shift + middle-drag | Pan |
| Wheel, or Ctrl + middle-drag | Zoom |
| Numpad 1 / 3 / 7 | Front / side / top view (hold Ctrl for the opposite side) |
| Numpad 2 / 4 / 6 / 8 | Rotate 15° |
| Home or `.` | Reset the camera |
| Left-click the chest | Place or move artwork |
| Left-drag elsewhere / right-drag | Orbit / pan (handy on a trackpad) |

Click the 3D view once before using the keyboard shortcuts. The numpad keys need Num Lock on.

**Quality:** classic shirt surfaces are small (the chest is 128 × 128 px), so art is always shrunk
a lot. **Quality** in Properties picks how: **Sharp** (default: crisp logos and text), **Smooth**
(soft; photos and gradients) or **Pixel** (no smoothing; pixel art). Making art larger on the shirt
helps more than a bigger source image. Comparison, enlarged 4× (Sharp | Smooth | Pixel):

![Sharp, Smooth and Pixel quality](docs/quality_comparison.png)

**Tattoos:** the **Tattoos (under the template)** section in the Layers panel puts a 585 × 559
tattoo template *under* the shirt template, so it only shows where the template is transparent:
the bare arms of a short-sleeve shirt, like in Roblox. **Import…** adds a tattoo PNG to your
tattoo library (`%APPDATA%\clothing-studio	attoos`), and it then stays in the menu. **Strength**
sets how strong it is. **White = skin** turns a white background see-through and dark lines into
ink, for tattoo art drawn on white; leave it off for PNGs that already have transparency. Each
garment has its own tattoo, saved in projects.

**Upscale ×4 (AI):** for small pictures (an icon, a tiny crop) that would look blurry stretched
across the shirt. Tick it in Properties and the picture is enlarged 4× by an AI model that
rebuilds sharp edges and lettering, instead of just blurring. It uses Real-ESRGAN general x4v3
(BSD-3), Qualcomm AI Hub's ONNX export, run locally and offline. The first time, the app asks
before downloading it once (4.5 MB, checked against a fixed SHA-256). It's non-destructive
(untick to compare, saved in projects). Pictures over 1024 px don't need it, so the app says so;
small imports get a hint to try it.

**Match shirt to art background:** for a picture with a plain background (a photo, a screenshot),
this Properties button recolours the garment's template to the colour around the picture's edge,
so the background blends into the shirt instead of showing as a box. It keeps the template's
shape (bare arms stay bare) but replaces details like stripes; Ctrl+Z undoes it.
To remove the background instead, use **Remove background (AI)**.

**Background removal:** tick **Remove background (AI)** on a layer. It uses the ISNet model
(general use, Apache-2.0) through ONNX Runtime, entirely on this PC. The first time, the app asks
before downloading the model once (179 MB, from the rembg project's GitHub release, checked against
a fixed SHA-256) into `%APPDATA%\clothing-studio\models`. A removal takes about a second on a typical
CPU and runs in the background, so the app stays usable. It's non-destructive: the original image
is kept, unticking brings the background back, and the mask is saved in the project. Nothing keeps
running after you close the app.

**Projects:** File → Save project (Ctrl+S) stores everything in one `.clothing` file (JSON inside): the
template, every layer and its settings, and your original artwork (lossless, embedded). Open it
again with File → Open project (Ctrl+O), or start over with New project (Ctrl+N). The title bar
shows the project name, with • while there are unsaved changes. The app asks before closing or
opening something else with unsaved work. It also auto-saves unsaved work every few seconds to
`%APPDATA%\clothing-studio`, and if it was closed without saving (a crash, say), it offers to
restore that work on the next start. You can also open a project from the command line:
`clothing-studio "My shirt.clothing"` (older `.clothing.json` projects still open).

**Body parts:** the **Head / Arms / Legs** checkboxes at the bottom-left of the 3D view hide or
show those parts, for example to see the torso on its own or reach the inner side of a sleeve. Hidden
parts can't be clicked, and the setting is remembered. The torso always shows, and the legs are
always black.

**Your own rig:** **Load rig (OBJ)…** accepts an R15 or R6 blocky OBJ exported from Blender
(objects named like `UpperTorso_Geo`, `LeftUpperArm_Geo`, `Head_Geo`, standing on y = 0 and
facing +Z). Its own UVs target Roblox's body atlas, so they're ignored. The shirt is projected
onto each body part by face instead, using the same regions as the block rig. You can also start
with `--rig path.obj` and `--template path.png`.

**Text:** **Text** in the Layers panel adds a text layer. With it selected, Properties → Text sets
the words, font (from fonts installed with Windows: Arial Black, Impact, Comic Sans and more),
fill and outline colour, and outline thickness. It's a normal layer otherwise: drag, size, rotate,
wrap. Projects keep the text settings, and the rendered text too, so they still open where the
font is missing.

**Shading:** the **Shading** section in the Layers panel adds soft shadows and folds over the whole
garment: **Soft** (rounded edges), **Wrinkles** (plus elbow/knee creases and torso folds) or
**Strong**. These are generated by the app, not copied from anywhere. **Import…** adds your own
585 × 559 shading PNG (black darkens, white lightens, transparency = amount) to your shading
library in `%APPDATA%\clothing-studio\shading`. Library shadings are listed first in the menu, and
you can also drop PNGs into that folder yourself. They aren't bundled with the app. **Strength**
sets how much. Shading only lands where there's clothing, so bare skin stays clean. Each garment
has its own, and it's saved in projects.

**Shirt and pants:** the **Shirt | Pants** buttons at the left of the toolbar choose what you're
editing. Each garment has its own template, layers and undo history, and **Export PNG…** exports
the one you're editing (`shirt.png` / `pants.png`). The 3D avatar always shows both, layered like
Roblox: pants cover the torso and legs, and the shirt covers the torso and arms, on top. Art can
be placed on the torso and arms in Shirt mode, and on the torso and legs in Pants mode. A project
file stores both garments. Pants templates use the same 585 × 559 layout, with the right and left
legs where a shirt has the right and left arms.

**Getting it into Roblox Studio:**

- **Without setup (default):** **Export PNG…** opens File Explorer with the file selected. Drag it
  into Studio (or use Asset Manager → Bulk Import), copy its ID, and paste it into
  `Shirt.ShirtTemplate` (or `Pants.PantsTemplate`).
- **One click (optional):** **Upload to Roblox** uploads the garment you're editing to your Roblox
  account and copies `rbxassetid://…` to the clipboard. It needs an Open Cloud API key, set up
  once through File → Roblox upload settings…:
  1. Go to create.roblox.com → Open Cloud → API Keys → Create API Key.
  2. Add the **Assets** API with **Read** and **Write**, and allow IP `0.0.0.0/0` (or your IP).
  3. Paste the key into the app. It checks the key with Roblox and stores it in **Windows
     Credential Manager**, never in a file.

  Uploads go to the key owner's account as images (Roblox's API doesn't upload Shirt/Pants items
  themselves; listing on the Marketplace still happens on the Creator Hub). A new upload may
  look blank in Studio until Roblox's moderation approves it. Keys expire after 60 days unused.
  Remove the key any time in the same settings window.

**Window and panels:** the window can be resized freely. Drag the thin dividers between the 3D
view, the 2D view and the Layers panel to resize them (View → Reset panel sizes undoes that). The
less-used actions (load/save/delete template, load rig, paste, export) are in the **File** and
**Edit** menus, which keeps the toolbar short enough for small windows.

**2D view:** it fits the template to the panel automatically. Middle-drag pans and the wheel zooms
(that switches automatically fitting off); **Fit** or View → Fit 2D template turns it back on.

**Updates:** on start the app checks GitHub for a newer release and offers to install it
(Help → Check for updates… does it on demand). The installer is checked against GitHub's SHA-256
before it runs. Only public release info is requested; nothing about you is sent.

## Layout

```
src/template.rs   585×559 region map, presets, diagnostic template
src/compose.rs    document model + deterministic compositing (used by 3D, 2D and export)
src/avatar.rs     R6 block rig, UV mesh, raycasting, orbit camera
src/render.rs     wgpu offscreen renderer sharing Slint's GPU device
src/main.rs       Slint UI glue
ui/app.slint      UI layout
docs/             architecture, UV mapping, sample outputs
```

## Third-party components

- [ONNX Runtime](https://github.com/microsoft/onnxruntime) (MIT), via the [`ort`](https://github.com/pykeio/ort) crate (MIT/Apache-2.0), built into the app.
- [Real-ESRGAN](https://github.com/xinntao/Real-ESRGAN) general x4v3 (BSD-3), in [Qualcomm AI Hub](https://huggingface.co/qualcomm/Real-ESRGAN-General-x4v3)'s ONNX export. It isn't in this repository; the app downloads it on first use, with permission.
- [ISNet / DIS](https://github.com/xuebinqin/DIS) general-use model (Apache-2.0), in the ONNX export distributed by [rembg](https://github.com/danielgatis/rembg). It isn't included in this repository; the app downloads it on first use, with permission.
- [Slint](https://slint.dev) (GUI), [wgpu](https://wgpu.rs), [image](https://github.com/image-rs/image) and the other crates listed in `Cargo.toml`.
