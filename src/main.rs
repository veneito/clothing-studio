#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod avatar;
mod bgremove;
mod compose;
mod library;
mod project;
mod render;
mod roblox;
mod shading;
mod template;
mod text;
mod updater;
mod upscale;

use compose::{Document, Layer};
use image::{Rgba, RgbaImage};
use slint::{Model, ModelRc, VecModel};
use std::{cell::RefCell, error::Error, path::Path, rc::Rc};
use template::{Face, Garment, Part};

slint::include_modules!();

#[derive(Clone, Copy, PartialEq)]
enum View {
    ThreeD,
    TwoD,
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    Orbit,
    Pan,
    Zoom,
    Pan2D,
    /// Moving the selected layer; offset = grab point minus layer center, in template pixels.
    Art { view: View, offset: [f32; 2], moved: bool },
    /// Resizing the selected layer by a handle in the 2D view.
    Resize { handle: compose::Handle, start: compose::ResizeStart, moved: bool },
}

struct State {
    doc: Document,
    selected: Option<usize>,
    composed: RgbaImage,
    cam: avatar::Camera,
    /// Placement/raycast geometry (always the R6 boxes).
    rig: Vec<avatar::BoxPart>,
    /// Display mesh: the block rig, or a loaded OBJ rig projected onto the same boxes.
    mesh: (Vec<avatar::Vertex>, Vec<u32>),
    /// Text of the loaded OBJ rig, kept so the mesh can be rebuilt when parts are hidden/shown.
    obj_text: Option<Rc<String>>,
    renderer: Option<render::Renderer>,
    drag: Drag,
    last: (f32, f32),
    /// Undo stack (oldest first) and redo stack. Images inside are Rc-shared, so steps are cheap.
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Edits with the same key in a row (one slider drag, a run of nudges) form one undo step.
    last_key: Option<&'static str>,
    /// Saved templates shown after the built-in presets in the template dropdown.
    library: Vec<(String, std::path::PathBuf)>,
    /// The user's shading overlays (%APPDATA%\clothing-studio\shading), listed before the built-in ones.
    shading_library: Vec<(String, std::path::PathBuf)>,
    /// The user's tattoo templates (%APPDATA%\clothing-studio\tattoos).
    tattoo_library: Vec<(String, std::path::PathBuf)>,
    /// Project file this work belongs to (None = never saved).
    project_path: Option<std::path::PathBuf>,
    /// Counts edits; unsaved when it differs from `saved_edits`. `autosaved_edits` avoids
    /// rewriting the auto-save when nothing changed.
    edits: u64,
    saved_edits: u64,
    autosaved_edits: u64,
    png_cache: project::PngCache,
    /// The base on screen matches no dropdown entry (opened project, deleted template): the
    /// dropdown shows an extra "Custom template" entry for it.
    custom_template: bool,
    /// Polls a running background-removal job (None when idle).
    bg_job: Option<slint::Timer>,
    /// Polls a running Roblox request (key check or upload); None when idle.
    net_job: Option<slint::Timer>,
    /// A redraw is already queued for this frame (drag moves are merged into one per frame).
    refresh_queued: bool,
    /// Shirt face under the cursor in the 3D view (tinted as a placement hint).
    hover: Option<(Part, Face)>,
    /// The garment being edited. `doc`, `selected`, `composed`, undo/redo and `custom_template`
    /// above belong to it; the other garment's copies wait in `other` (swapped on switching).
    garment: Garment,
    other: Stash,
}

/// Everything per-garment for the garment that is *not* being edited.
struct Stash {
    doc: Document,
    selected: Option<usize>,
    composed: RgbaImage,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    custom_template: bool,
    /// Its template's dropdown name (the active garment's comes from the dropdown itself).
    template_name: Option<String>,
}

impl Stash {
    fn new(garment: Garment) -> Self {
        let doc = Document { base: Rc::new(garment.preset(0)), layers: vec![], overlay: None, underlay: None };
        Stash {
            composed: compose::compose(&doc),
            doc,
            selected: None,
            undo: vec![],
            redo: vec![],
            custom_template: false,
            template_name: Some(garment.preset_names()[0].to_owned()),
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    base: Rc<RgbaImage>,
    layers: Vec<Layer>,
    overlay: Option<compose::Overlay>,
    underlay: Option<compose::Overlay>,
    selected: Option<usize>,
}

const MAX_UNDO: usize = 200;

impl State {
    fn snapshot(&self) -> Snapshot {
        Snapshot { base: self.doc.base.clone(), layers: self.doc.layers.clone(), overlay: self.doc.overlay.clone(), underlay: self.doc.underlay.clone(), selected: self.selected }
    }
    fn restore(&mut self, snap: Snapshot) {
        (self.doc.base, self.doc.layers, self.doc.overlay, self.doc.underlay, self.selected) = (snap.base, snap.layers, snap.overlay, snap.underlay, snap.selected);
    }
    /// Record the state *before* an edit. `key` merges consecutive edits of the same kind.
    fn checkpoint(&mut self, key: Option<&'static str>) {
        self.edits += 1; // every edit counts as a change, even when merged into one undo step
        if key.is_some() && key == self.last_key {
            return;
        }
        let snap = self.snapshot();
        self.undo.push(snap);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_key = key;
    }
}

type Shared = Rc<RefCell<State>>;

/// Wire a Slint callback to a handler that gets the app and the shared state.
macro_rules! on {
    ($app:ident, $st:ident, $setter:ident, |$a:ident, $s:ident $(, $arg:ident)*| $body:expr) => {{
        let (w, st2) = ($app.as_weak(), $st.clone());
        $app.$setter(move |$($arg),*| {
            if let Some($a) = w.upgrade() {
                let $s = &st2;
                $body
            }
        });
    }};
}

fn main() {
    if let Err(e) = run() {
        // Launched by double-click there is no console, so show startup failures in a dialog.
        eprintln!("Clothing Studio could not start: {e}");
        rfd::MessageDialog::new()
            .set_title("Clothing Studio")
            .set_description(format!("Clothing Studio could not start:

{e}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--sample") {
        let out = args.get(2).map(String::as_str).unwrap_or("sample_shirt.png");
        return export_sample(Path::new(out));
    }
    if args.get(1).map(String::as_str) == Some("--diagnostic") {
        let out = Path::new(args.get(2).map(String::as_str).unwrap_or("diagnostic_shirt.png"));
        save_png(&template::diagnostic(), out)?;
        println!("wrote {}", out.display());
        return Ok(());
    }

    let mut gpu = slint::wgpu_30::WGPUSettings::default();
    // wgpu's default backend choice crashed on some laptops; DX12 is the reliable one on
    // Windows. Setting WGPU_BACKEND (e.g. "vulkan") still overrides it.
    if cfg!(windows) && slint::wgpu_30::wgpu::Backends::from_env().is_none() {
        gpu.backends = slint::wgpu_30::wgpu::Backends::DX12;
    }
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Automatic(gpu))
        .select()?;
    let app = AppWindow::new()?;
    let doc = Document { base: Rc::new(Garment::Shirt.preset(0)), layers: vec![], overlay: None, underlay: None };
    let st: Shared = Rc::new(RefCell::new(State {
        composed: compose::compose(&doc),
        doc,
        selected: None,
        cam: avatar::Camera::default(),
        mesh: avatar::mesh(&avatar::rig()),
        obj_text: None,
        rig: avatar::rig(),
        renderer: None,
        drag: Drag::None,
        last: (0.0, 0.0),
        undo: Vec::new(),
        redo: Vec::new(),
        last_key: None,
        library: library::templates_dir().map(|d| library::list(&d)).unwrap_or_default(),
        shading_library: library::shading_dir().map(|d| library::list(&d)).unwrap_or_default(),
        tattoo_library: library::tattoos_dir().map(|d| library::list(&d)).unwrap_or_default(),
        project_path: None,
        edits: 0,
        saved_edits: 0,
        autosaved_edits: 0,
        png_cache: project::PngCache::default(),
        custom_template: false,
        bg_job: None,
        net_job: None,
        refresh_queued: false,
        hover: None,
        garment: Garment::Shirt,
        other: Stash::new(Garment::Pants),
    }));
    app.set_status("Pick a preset or load your own 585×559 template, then import artwork.".into());
    let fonts: Vec<slint::SharedString> = text::available_fonts().iter().map(|f| f.0.into()).collect();
    app.set_font_names(ModelRc::new(VecModel::from(fonts)));
    // [--template t.png] [--rig r.obj] [art.png ...]   (a bare path also enables "Open with…")
    // Reopen the last rig unless one is given on the command line.
    if let Some([h, a, l]) = library::read_setting("show_parts").and_then(|v| parse_parts(&v)) {
        app.set_show_head(h);
        app.set_show_arms(a);
        app.set_show_legs(l);
        apply_part_visibility(&mut st.borrow_mut(), [h, a, l]);
        let m = avatar::mesh(&st.borrow().rig); // an OBJ rig loaded below replaces this
        st.borrow_mut().mesh = m;
    }
    if !args.iter().any(|a| a == "--rig") {
        if let Some(p) = library::read_setting("last_rig").filter(|p| Path::new(p).is_file()) {
            open_rig(&app, &st, Path::new(&p));
        }
    }
    // Reopen the last used templates (a preset or a saved one), shirt and pants.
    {
        let mut s = st.borrow_mut();
        let pants = library::read_setting(template_setting(Garment::Pants));
        if let Some(img) = pants.as_deref().and_then(|n| template_image(&s, Garment::Pants, n)) {
            s.other.doc.base = Rc::new(img);
            s.other.composed = compose::compose(&s.other.doc);
            s.other.template_name = pants;
        }
    }
    sync_template_list(&app, &st.borrow());
    let last_template = library::read_setting(template_setting(Garment::Shirt));
    let last_index = last_template.and_then(|name| template_names(&st.borrow()).iter().position(|n| *n == name));
    if let Some(i) = last_index {
        choose_template(&app, &st, i);
    }
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--template" => it.next().iter().for_each(|p| open_template(&app, &st, Path::new(p))),
            "--rig" => it.next().iter().for_each(|p| open_rig(&app, &st, Path::new(p))),
            p if project::is_project_file(p) => {
                if let Err(e) = open_project_path(&app, &st, Path::new(p)) {
                    app.set_status(format!("Could not open {p}: {e}").into());
                }
            }
            p => open_art(&app, &st, Path::new(p)),
        }
    }
    // Startup restores are not user edits: start with an empty history and nothing unsaved.
    {
        let mut s = st.borrow_mut();
        s.undo.clear();
        s.saved_edits = s.edits;
        s.autosaved_edits = s.edits;
    }
    refresh(&app, &st);
    sync_layers(&app, &st.borrow());

    {
        let (aw, st) = (app.as_weak(), st.clone());
        app.window().set_rendering_notifier(move |state, api| {
            let (slint::RenderingState::RenderingSetup, slint::GraphicsAPI::WGPU30 { device, queue, .. }) = (state, api)
            else {
                return;
            };
            let r = {
                let s = st.borrow();
                render::Renderer::new(device, queue, &s.mesh.0, &s.mesh.1)
            };
            {
                let s = st.borrow();
                r.upload(s.garment, &s.composed);
                r.upload(other_garment(s.garment), &s.other.composed);
            }
            st.borrow_mut().renderer = Some(r);
            if let Some(app) = aw.upgrade() {
                redraw(&app, &st);
            }
        })?;
    }

    // ---- files ----
    on!(app, st, on_load_template, |a, s| pick_then(&a, s, &["png", "jpg", "jpeg", "webp"], open_template));
    on!(app, st, on_load_rig, |a, s| pick_then(&a, s, &["obj"], open_rig));
    on!(app, st, on_import_art, |a, s| pick_then(&a, s, &["png", "jpg", "jpeg", "webp"], open_art));
    on!(app, st, on_export_png, |a, s| export_png(&a, s));
    on!(app, st, on_preset_chosen, |a, s, i| choose_template(&a, s, i as usize));
    on!(app, st, on_garment_chosen, |a, s, g| switch_garment(&a, s, if g == 1 { Garment::Pants } else { Garment::Shirt }));
    on!(app, st, on_save_template, |a, s| save_template(&a, s));
    on!(app, st, on_parts_changed, |a, s| {
        let shown = [a.get_show_head(), a.get_show_arms(), a.get_show_legs()];
        apply_part_visibility(&mut s.borrow_mut(), shown);
        library::write_setting("show_parts", &shown.map(|b| if b { "1" } else { "0" }).join(","));
        rebuild_mesh(&a, s);
    });
    on!(app, st, on_delete_template, |a, s| delete_template(&a, s));

    // ---- 3D view ----
    on!(app, st, on_vp_resized, |a, s| redraw(&a, s));
    on!(app, st, on_vp_down, |a, s, x, y, button, shift, ctrl| on_down(&a, s, View::ThreeD, x, y, button, shift, ctrl));
    on!(app, st, on_vp_move, |a, s, x, y| {
        set_hover(&a, s, x, y);
        on_move(&a, s, x, y, false)
    });
    on!(app, st, on_vp_leave, |a, s| set_hover(&a, s, f32::NAN, f32::NAN));
    on!(app, st, on_vp_up, |_a, s| s.borrow_mut().drag = Drag::None);
    on!(app, st, on_vp_scroll, |a, s, dy| {
        s.borrow_mut().cam.zoom(dy);
        redraw(&a, s);
    });

    // ---- 2D view ----
    on!(app, st, on_tpl_down, |a, s, x, y, button, shift, ctrl| on_down(&a, s, View::TwoD, x, y, button, shift, ctrl));
    on!(app, st, on_tpl_move, |a, s, x, y, shift| {
        set_tpl_cursor(&a, &s.borrow(), x, y);
        on_move(&a, s, x, y, shift)
    });
    on!(app, st, on_tpl_up, |_a, s| s.borrow_mut().drag = Drag::None);
    on!(app, st, on_tpl_scroll, |a, _s, dy, x, y| {
        let old = a.get_zoom();
        a.invoke_take_manual_view();
        let new = (old * (dy * 0.002).exp()).clamp(0.1, 8.0);
        // keep the template pixel under the cursor fixed
        a.set_user_pan_x(x - (x - a.get_pan_x()) * new / old);
        a.set_user_pan_y(y - (y - a.get_pan_y()) * new / old);
        a.set_user_zoom(new);
    });

    // ---- keyboard ----
    on!(app, st, on_key, |a, s, text, shift, ctrl| on_key(&a, s, text.as_str(), shift, ctrl));

    // ---- layers panel ----
    on!(app, st, on_layer_select, |a, s, row| {
        let mut b = s.borrow_mut();
        b.selected = row_to_index(&b, row);
        drop(b);
        selection_changed(&a, s);
    });
    on!(app, st, on_layer_visible, |a, s, row, v| {
        let mut b = s.borrow_mut();
        if let Some(i) = row_to_index(&b, row) {
            b.checkpoint(None);
            b.doc.layers[i].visible = v;
        }
        drop(b);
        layers_changed(&a, s);
    });
    on!(app, st, on_layer_up, |a, s| move_selected(&a, s, 1));
    on!(app, st, on_layer_down, |a, s| move_selected(&a, s, -1));
    on!(app, st, on_layer_duplicate, |a, s| duplicate_selected(&a, s));
    on!(app, st, on_layer_delete, |a, s| delete_selected(&a, s));
    on!(app, st, on_layer_rename, |a, s, row, name| {
        let name = name.trim();
        let mut b = s.borrow_mut();
        let Some(i) = row_to_index(&b, row) else { return };
        if name.is_empty() || b.doc.layers[i].name == name {
            drop(b);
            return sync_layers(&a, &s.borrow()); // restore the shown name
        }
        b.checkpoint(None);
        b.doc.layers[i].name = name.to_owned();
        drop(b);
        layers_changed(&a, s);
    });
    on!(app, st, on_undo, |a, s| undo_redo(&a, s, true));
    on!(app, st, on_redo, |a, s| undo_redo(&a, s, false));
    on!(app, st, on_prop_width, |a, s, v| edit_selected(&a, s, Some("width"), |l| l.width = v));
    on!(app, st, on_prop_rotation, |a, s, v| edit_selected(&a, s, Some("rotation"), |l| l.rotation = v.round()));
    on!(app, st, on_prop_opacity, |a, s, v| edit_selected(&a, s, Some("opacity"), |l| l.opacity = v));
    on!(app, st, on_prop_brightness, |a, s, v| edit_selected(&a, s, Some("brightness"), |l| l.brightness = v));
    on!(app, st, on_prop_contrast, |a, s, v| edit_selected(&a, s, Some("contrast"), |l| l.contrast = v));
    on!(app, st, on_prop_remove_bg, |a, s, v| remove_bg_toggled(&a, s, v));
    on!(app, st, on_prop_upscale, |a, s, v| upscale_toggled(&a, s, v));
    on!(app, st, on_add_text, |a, s| add_text_layer(&a, s));
    on!(app, st, on_match_base, |a, s| match_base_to_art(&a, s));
    on!(app, st, on_roblox_key_save, |a, s, k| roblox_key_save(&a, s, k.to_string()));
    on!(app, st, on_roblox_key_remove, |a, _s| {
        match roblox::delete_key() {
            Ok(()) => {
                library::write_setting("roblox_user_id", "");
                a.set_roblox_has_key(false);
                a.set_roblox_status("Key removed. Uploading is off until you add a key again.".into());
            }
            Err(e) => a.set_roblox_status(e.into()),
        }
    });
    on!(app, st, on_upload_roblox, |a, s| upload_to_roblox(&a, s));
    if roblox::load_key().is_some() {
        app.set_roblox_has_key(true);
        app.set_roblox_status(format!("A key is saved (Roblox user {}).", library::read_setting("roblox_user_id").unwrap_or_default()).into());
    }
    on!(app, st, on_prop_text, |a, s, t| edit_text(&a, s, Some("text"), |sp| sp.text = t.to_string()));
    on!(app, st, on_prop_font, |a, s, i| {
        let fonts = text::available_fonts();
        let Some(f) = usize::try_from(i).ok().and_then(|i| fonts.get(i)) else { return };
        let name = f.0.to_owned();
        edit_text(&a, s, None, |sp| sp.font = name)
    });
    on!(app, st, on_prop_text_color, |a, s, c| edit_text(&a, s, None, |sp| sp.color = [c.red(), c.green(), c.blue()]));
    on!(app, st, on_prop_outline_color, |a, s, c| edit_text(&a, s, None, |sp| sp.outline_color = [c.red(), c.green(), c.blue()]));
    on!(app, st, on_prop_outline, |a, s, v| edit_text(&a, s, Some("outline"), |sp| sp.outline = v));
    on!(app, st, on_shading_chosen, |a, s, i| shading_chosen(&a, s, i));
    on!(app, st, on_tattoo_chosen, |a, s, i| tattoo_chosen(&a, s, i));
    on!(app, st, on_tattoo_import, |a, s| pick_then(&a, s, &["png"], import_tattoo));
    on!(app, st, on_tattoo_strength_changed, |a, s, v| edit_tattoo(&a, s, Some("tattoo"), |u| u.strength = v));
    on!(app, st, on_tattoo_ink_changed, |a, s, v| edit_tattoo(&a, s, None, |u| u.ink = v));
    on!(app, st, on_shading_import, |a, s| pick_then(&a, s, &["png"], import_shading));
    on!(app, st, on_shading_strength_changed, |a, s, v| {
        let mut b = s.borrow_mut();
        if b.doc.overlay.is_none() {
            return;
        }
        b.checkpoint(Some("shading"));
        if let Some(o) = b.doc.overlay.as_mut() {
            o.strength = v;
        }
        drop(b);
        refresh(&a, s);
    });
    on!(app, st, on_prop_quality, |a, s, i| {
        let q = compose::Quality::ALL[(i.max(0) as usize).min(2)];
        edit_selected(&a, s, None, |l| l.quality = q)
    });
    on!(app, st, on_prop_invert, |a, s, v| edit_selected(&a, s, None, |l| l.invert = v));
    on!(app, st, on_prop_grayscale, |a, s, v| edit_selected(&a, s, None, |l| l.grayscale = v));
    on!(app, st, on_paste_art, |a, s| paste_art(&a, s));
    on!(app, st, on_prop_flip_h, |a, s, v| edit_selected(&a, s, None, |l| l.flip_h = v));
    on!(app, st, on_prop_flip_v, |a, s, v| edit_selected(&a, s, None, |l| l.flip_v = v));
    // "Wrap edges" (around the part) and "Across front" (onto the sleeves) exclude each other.
    on!(app, st, on_prop_wrap, |a, s, v| edit_selected(&a, s, None, |l| {
        l.wrap = v;
        if v {
            l.across = None;
        }
    }));
    on!(app, st, on_prop_across, |a, s, v| {
        let garment = s.borrow().garment;
        edit_selected(&a, s, None, |l| {
            l.across = v.then_some(garment);
            if v {
                l.wrap = false;
            }
        });
        if v && !st_selected_is_front_or_back(&s.borrow()) {
            a.set_status("Across front works for art on the front or back; drag it onto the chest or back.".into());
        }
    });
    on!(app, st, on_prop_reset, |a, s| edit_selected(&a, s, None, |l| {
        // default size is defined on the front torso; convert to the current face's width
        let front_w = Layer::default_width(&l.source);
        l.width = front_w * template::region(Part::Torso, Face::Front).w as f32 / template::region(l.part, l.face).w as f32;
        (l.rotation, l.flip_h, l.flip_v, l.opacity, l.stretch) = (0.0, false, false, 1.0, 1.0);
        (l.grayscale, l.brightness, l.contrast, l.invert) = (false, 0.0, 0.0, false);
    }));

    // ---- projects ----
    on!(app, st, on_new_project, |a, s| confirm_unsaved(&a, s, new_project));
    on!(app, st, on_open_project, |a, s| confirm_unsaved(&a, s, open_project_dialog));
    on!(app, st, on_save_project, |a, s| save_project(&a, s, None));
    on!(app, st, on_save_project_as, |a, s| save_project_as(&a, s, None));
    {
        // Closing with unsaved changes asks first; a clean close removes the auto-save.
        let (w, st) = (app.as_weak(), st.clone());
        app.window().on_close_requested(move || {
            let Some(a) = w.upgrade() else { return slint::CloseRequestResponse::HideWindow };
            if !is_dirty(&st.borrow()) {
                remove_autosave();
                return slint::CloseRequestResponse::HideWindow;
            }
            confirm_unsaved(&a, &st, |a, _| {
                remove_autosave();
                let _ = a.hide();
            });
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    let autosave = slint::Timer::default();
    {
        let (w, st) = (app.as_weak(), st.clone());
        autosave.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(5), move || {
            if let Some(a) = w.upgrade() {
                autosave_tick(&a, &st);
            }
        });
    }
    offer_restore(&app, &st);
    on!(app, st, on_check_updates, |a, s| check_for_updates(&a, s, true));
    check_for_updates(&app, &st, false); // quietly, on every start

    app.run()?;
    Ok(())
}

// ---------------------------------------------------------------- pointer

fn vp_size(app: &AppWindow) -> (f32, f32) {
    (app.get_vp_width(), app.get_vp_height())
}

/// Shirt surface under the cursor in either view: (part, face, face-normalized uv).
fn surface_at(app: &AppWindow, s: &State, view: View, x: f32, y: f32) -> Option<(Part, Face, [f32; 2])> {
    match view {
        View::ThreeD => {
            let (w, h) = vp_size(app);
            if w < 1.0 || h < 1.0 {
                return None;
            }
            let (o, d) = s.cam.ray(w / h, x / w, y / h);
            let hit = avatar::raycast(&s.rig, o, d)?;
            // Shirt mode places on torso/arms, pants mode on torso/legs.
            Some((hit.part.filter(|p| s.garment.parts().contains(p))?, hit.face, hit.uv))
        }
        View::TwoD => {
            let z = app.get_zoom();
            template::locate(s.garment, (x - app.get_pan_x()) / z, (y - app.get_pan_y()) / z)
        }
    }
}

fn to_px(part: Part, face: Face, uv: [f32; 2]) -> [f32; 2] {
    let r = template::region(part, face);
    [r.x as f32 + uv[0] * r.w as f32, r.y as f32 + uv[1] * r.h as f32]
}

/// Blender-style navigation. Left-drag on artwork selects and moves it (any shirt face, either
/// view); left-drag elsewhere orbits in 3D (handy on trackpads).
fn on_down(app: &AppWindow, st: &Shared, view: View, x: f32, y: f32, button: i32, shift: bool, ctrl: bool) {
    let mut s = st.borrow_mut();
    s.last = (x, y);
    let mut selection = false;
    s.drag = match (view, button) {
        (View::TwoD, 1 | 2) => Drag::Pan2D,
        (View::ThreeD, 2) if shift => Drag::Pan,
        (View::ThreeD, 2) if ctrl => Drag::Zoom,
        (View::ThreeD, 2) => Drag::Orbit,
        (View::ThreeD, 1) => Drag::Pan,
        // a resize handle of the selected art (2D view) wins over grabbing the art itself
        (View::TwoD, 0) if handle_under(app, &s, x, y).is_some() => {
            let (i, handle) = handle_under(app, &s, x, y).unwrap();
            Drag::Resize { handle, start: s.doc.layers[i].resize_start(), moved: false }
        }
        _ => {
            let grab = surface_at(app, &s, view, x, y).and_then(|(p, f, uv)| {
                let px = to_px(p, f, uv);
                // The selected layer can be grabbed anywhere inside its box (transparent parts
                // too, e.g. after background removal); otherwise pick the art under the cursor.
                let sel = s.selected.filter(|&i| {
                    let l = &s.doc.layers[i];
                    l.visible && l.box_contains(p, f, px[0], px[1])
                });
                let i = sel.or_else(|| compose::layer_at(&s.doc, p, f, px[0], px[1]))?;
                let c = s.doc.layers[i].center_px();
                Some((i, [px[0] - c.0, px[1] - c.1]))
            });
            match grab {
                Some((i, offset)) => {
                    selection = s.selected != Some(i);
                    s.selected = Some(i);
                    Drag::Art { view, offset, moved: false }
                }
                None if view == View::ThreeD => Drag::Orbit,
                None => Drag::None,
            }
        }
    };
    drop(s);
    if selection {
        selection_changed(app, st);
    }
}

/// Track the shirt face under the cursor (when not dragging) and redraw only when it changes.
/// NaN coordinates mean the cursor left the 3D view.
fn set_hover(app: &AppWindow, st: &Shared, x: f32, y: f32) {
    let mut s = st.borrow_mut();
    let over = (s.drag == Drag::None && x.is_finite())
        .then(|| surface_at(app, &s, View::ThreeD, x, y).map(|(p, f, _)| (p, f)))
        .flatten();
    if over != s.hover {
        s.hover = over;
        drop(s);
        redraw(app, st);
    }
}

/// 2D view: the selected layer's resize handle under (x, y), if any.
fn handle_under(app: &AppWindow, s: &State, x: f32, y: f32) -> Option<(usize, compose::Handle)> {
    let i = s.selected?;
    let l = s.doc.layers.get(i).filter(|l| l.visible)?;
    let z = app.get_zoom();
    let (px, py) = ((x - app.get_pan_x()) / z, (y - app.get_pan_y()) / z);
    l.handle_at(px, py, 6.0 / z).map(|h| (i, h)) // ~6 screen pixels, whatever the zoom
}

/// Resize-arrow cursor over a handle in the 2D view (while not dragging something else).
fn set_tpl_cursor(app: &AppWindow, s: &State, x: f32, y: f32) {
    let handle = match s.drag {
        Drag::Resize { handle, .. } => Some(handle),
        Drag::None => handle_under(app, s, x, y).map(|(_, h)| h),
        _ => None,
    };
    let rotation = s.selected.and_then(|i| s.doc.layers.get(i)).map_or(0.0, |l| l.rotation);
    app.set_tpl_cursor(handle.map_or(0, |h| resize_cursor(h, rotation)));
}

/// Which of the 4 arrow cursors points along handle `h` once the art is rotated.
fn resize_cursor((sx, sy): compose::Handle, rotation: f32) -> i32 {
    let angle = (sy as f32).atan2(sx as f32).to_degrees() + rotation; // direction from the centre
    let a = angle.rem_euclid(180.0);
    match a {
        a if !(22.5..157.5).contains(&a) => 3, // ↔
        a if a < 67.5 => 1,                    // ↖↘ (y grows downward)
        a if a < 112.5 => 4,                   // ↕
        _ => 2,                                // ↗↙
    }
}

fn on_move(app: &AppWindow, st: &Shared, x: f32, y: f32, shift: bool) {
    let mut s = st.borrow_mut();
    let (dx, dy) = (x - s.last.0, y - s.last.1);
    s.last = (x, y);
    match s.drag {
        Drag::None => return,
        Drag::Orbit => s.cam.orbit(dx, dy),
        Drag::Pan => s.cam.pan(dx, dy),
        Drag::Zoom => s.cam.zoom(-dy * 4.0),
        Drag::Pan2D => {
            app.invoke_take_manual_view();
            app.set_user_pan_x(app.get_user_pan_x() + dx);
            app.set_user_pan_y(app.get_user_pan_y() + dy);
            return;
        }
        Drag::Resize { handle, start, moved } => {
            let Some(i) = s.selected else { return };
            if !moved {
                s.checkpoint(None); // one undo step per resize drag
                s.drag = Drag::Resize { handle, start, moved: true };
            }
            let z = app.get_zoom();
            let (px, py) = ((x - app.get_pan_x()) / z, (y - app.get_pan_y()) / z);
            s.doc.layers[i].resize_from(&start, handle, px, py, shift);
            drop(s);
            return queue_refresh(app, st);
        }
        Drag::Art { view, offset, moved } => {
            // Off-shirt positions are ignored.
            let Some((p, f, uv)) = surface_at(app, &s, view, x, y) else { return };
            let Some(i) = s.selected else { return };
            if !moved {
                // one undo step per drag, recorded only once the art actually moves
                s.checkpoint(None);
                s.drag = Drag::Art { view, offset, moved: true };
            }
            let r = template::region(p, f);
            let center = [uv[0] - offset[0] / r.w as f32, uv[1] - offset[1] / r.h as f32];
            s.doc.layers[i].move_to(p, f, center);
            drop(s);
            return queue_refresh(app, st);
        }
    }
    drop(s);
    redraw(app, st);
}

fn on_key(app: &AppWindow, st: &Shared, key: &str, shift: bool, ctrl: bool) {
    let step = if shift { 10.0 } else { 1.0 };
    let nudge = match key {
        "\u{F700}" => Some([0.0, -step]),
        "\u{F701}" => Some([0.0, step]),
        "\u{F702}" => Some([-step, 0.0]),
        "\u{F703}" => Some([step, 0.0]),
        _ => None,
    };
    // Ctrl+letter may arrive as the letter or as its ASCII control code (Ctrl+Z = 0x1A).
    match (key, ctrl) {
        ("\u{7f}" | "\u{8}", _) => delete_selected(app, st),
        ("d" | "D" | "\u{4}", true) => duplicate_selected(app, st),
        ("v" | "V" | "\u{16}", true) => paste_art(app, st),
        ("s" | "S" | "\u{13}", true) if shift => save_project_as(app, st, None),
        ("s" | "S" | "\u{13}", true) => save_project(app, st, None),
        ("o" | "O" | "\u{f}", true) => confirm_unsaved(app, st, open_project_dialog),
        ("n" | "N" | "\u{e}", true) => confirm_unsaved(app, st, new_project),
        ("z" | "Z" | "\u{1a}", true) => undo_redo(app, st, !shift),
        ("y" | "Y" | "\u{19}", true) => undo_redo(app, st, false),
        _ if nudge.is_some() => {
            let [nx, ny] = nudge.unwrap();
            edit_selected(app, st, Some("nudge"), |l| {
                let r = template::region(l.part, l.face);
                l.center[0] += nx / r.w as f32;
                l.center[1] += ny / r.h as f32;
            })
        }
        _ => {
            if st.borrow_mut().cam.view_key(key, ctrl) {
                redraw(app, st);
            }
        }
    }
}

// ---------------------------------------------------------------- layers

fn row_to_index(s: &State, row: i32) -> Option<usize> {
    let n = s.doc.layers.len() as i32;
    (0..n).contains(&row).then(|| (n - 1 - row) as usize)
}

fn undo_redo(app: &AppWindow, st: &Shared, undo: bool) {
    let mut s = st.borrow_mut();
    let (from, label) = if undo { (s.undo.pop(), "Undo") } else { (s.redo.pop(), "Redo") };
    let Some(snap) = from else { return };
    let current = s.snapshot();
    if undo { s.redo.push(current) } else { s.undo.push(current) }
    let base_before = s.doc.base.clone();
    s.restore(snap);
    s.last_key = None;
    s.edits += 1;
    if !Rc::ptr_eq(&base_before, &s.doc.base) {
        select_matching_template(app, &mut s, None); // undoing a template change updates the dropdown
    }
    drop(s);
    app.set_status(label.into());
    layers_changed(app, st);
}

fn edit_selected(app: &AppWindow, st: &Shared, key: Option<&'static str>, f: impl FnOnce(&mut Layer)) {
    let mut s = st.borrow_mut();
    let Some(i) = s.selected else { return };
    s.checkpoint(key);
    f(&mut s.doc.layers[i]);
    drop(s);
    refresh(app, st);
    sync_props(app, &st.borrow());
}

fn move_selected(app: &AppWindow, st: &Shared, dir: i32) {
    let mut s = st.borrow_mut();
    let Some(i) = s.selected else { return };
    let j = i as i32 + dir;
    if j < 0 || j >= s.doc.layers.len() as i32 {
        return;
    }
    s.checkpoint(None);
    s.doc.layers.swap(i, j as usize);
    s.selected = Some(j as usize);
    drop(s);
    layers_changed(app, st);
}

fn duplicate_selected(app: &AppWindow, st: &Shared) {
    let mut s = st.borrow_mut();
    let Some(i) = s.selected else { return };
    s.checkpoint(None);
    let mut copy = s.doc.layers[i].clone();
    copy.name = format!("{} copy", copy.name);
    copy.center = [copy.center[0] + 0.06, copy.center[1] + 0.06];
    s.doc.layers.insert(i + 1, copy);
    s.selected = Some(i + 1);
    drop(s);
    layers_changed(app, st);
}

fn delete_selected(app: &AppWindow, st: &Shared) {
    let mut s = st.borrow_mut();
    let Some(i) = s.selected else { return };
    s.checkpoint(None);
    let removed = s.doc.layers.remove(i);
    // keep a neighbour selected so repeated Delete works like Photopea
    s.selected = if s.doc.layers.is_empty() { None } else { Some(i.saturating_sub(1)) };
    drop(s);
    app.set_status(format!("Deleted layer \"{}\" (Ctrl+Z to undo)", removed.name).into());
    layers_changed(app, st);
}

/// Layer stack changed (add/remove/reorder/visibility): recompose and rebuild the list.
fn layers_changed(app: &AppWindow, st: &Shared) {
    refresh(app, st);
    sync_layers(app, &st.borrow());
}

/// Only the selection changed: update highlight, outline and properties.
fn selection_changed(app: &AppWindow, st: &Shared) {
    let s = st.borrow();
    let n = s.doc.layers.len() as i32;
    app.set_selected_row(s.selected.map_or(-1, |i| n - 1 - i as i32));
    set_template_image(app, &s);
    sync_props(app, &s);
}

fn sync_layers(app: &AppWindow, s: &State) {
    let items: Vec<LayerItem> = s
        .doc
        .layers
        .iter()
        .rev()
        .map(|l| {
            let t = image::imageops::thumbnail(&*l.work, 32, 32);
            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(t.as_raw(), t.width(), t.height());
            LayerItem { name: l.name.as_str().into(), visible: l.visible, thumb: slint::Image::from_rgba8(buf) }
        })
        .collect();
    let model: ModelRc<LayerItem> = ModelRc::new(VecModel::from(items));
    debug_assert_eq!(model.row_count(), s.doc.layers.len());
    app.set_layers(model);
    let n = s.doc.layers.len() as i32;
    app.set_selected_row(s.selected.map_or(-1, |i| n - 1 - i as i32));
    sync_props(app, s);
    sync_shading(app, s);
}

fn sync_props(app: &AppWindow, s: &State) {
    let Some(l) = s.selected.and_then(|i| s.doc.layers.get(i)) else { return };
    app.set_sel_width(l.width);
    app.set_sel_brightness(l.brightness);
    app.set_sel_contrast(l.contrast);
    app.set_sel_grayscale(l.grayscale);
    app.set_sel_invert(l.invert);
    app.set_sel_remove_bg(l.remove_bg);
    app.set_sel_upscale(l.upscale);
    app.set_sel_is_text(l.text.is_some());
    if let Some(t) = &l.text {
        if app.get_sel_text() != t.text.as_str() {
            app.set_sel_text(t.text.as_str().into()); // don't reset the caret while typing
        }
        app.set_sel_font(text::available_fonts().iter().position(|f| f.0 == t.font).map_or(-1, |i| i as i32));
        app.set_sel_text_color(slint::Color::from_rgb_u8(t.color[0], t.color[1], t.color[2]));
        app.set_sel_outline_color(slint::Color::from_rgb_u8(t.outline_color[0], t.outline_color[1], t.outline_color[2]));
        app.set_sel_outline(t.outline);
    }
    app.set_sel_quality(compose::Quality::ALL.iter().position(|q| *q == l.quality).unwrap_or(0) as i32);
    app.set_sel_rotation(l.rotation);
    app.set_sel_opacity(l.opacity);
    app.set_sel_flip_h(l.flip_h);
    app.set_sel_flip_v(l.flip_v);
    app.set_sel_wrap(l.wrap);
    app.set_sel_across(l.across.is_some());
    app.set_sel_where(where_label(l.part, l.face).into());
}

fn where_label(part: Part, face: Face) -> String {
    let p = match part {
        Part::Torso => "Torso",
        Part::RightArm => "Right sleeve",
        Part::LeftArm => "Left sleeve",
        Part::RightLeg => "Right leg",
        Part::LeftLeg => "Left leg",
    };
    format!("{p} {face:?}").to_lowercase().replacen(&p.to_lowercase(), p, 1)
}

// ---------------------------------------------------------------- rendering

/// Refresh once on the next event-loop turn, however many mouse moves arrive before it. Mouse
/// events can come faster than frames; redrawing for each one made drags lag behind the cursor.
fn queue_refresh(app: &AppWindow, st: &Shared) {
    if std::mem::replace(&mut st.borrow_mut().refresh_queued, true) {
        return;
    }
    let (w, st) = (app.as_weak(), st.clone());
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        st.borrow_mut().refresh_queued = false;
        if let Some(app) = w.upgrade() {
            refresh(&app, &st);
            sync_props(&app, &st.borrow());
        }
    });
}

/// Recompose the document, then push the result to both views.
fn refresh(app: &AppWindow, st: &Shared) {
    {
        let mut s = st.borrow_mut();
        s.composed = compose::compose(&s.doc);
        if let Some(r) = &s.renderer {
            r.upload(s.garment, &s.composed);
        }
    }
    let s = st.borrow();
    update_title(app, &s);
    app.set_can_undo(!s.undo.is_empty());
    app.set_can_redo(!s.redo.is_empty());
    set_template_image(app, &s);
    drop(s);
    redraw(app, st);
}

/// 2D view: the composed texture plus a selection outline (preview only, never exported).
fn set_template_image(app: &AppWindow, s: &State) {
    let mut img = s.composed.clone();
    if let Some(l) = s.selected.and_then(|i| s.doc.layers.get(i)) {
        draw_outline(&mut img, l);
    }
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), img.width(), img.height());
    app.set_template_image(slint::Image::from_rgba8(buf));
}

fn draw_outline(img: &mut RgbaImage, l: &Layer) {
    draw_box(img, l);
    // resize handles: small white squares with a dark rim
    for h in compose::HANDLES {
        let (hx, hy) = l.handle_px(h);
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let (x, y) = (hx.round() as i32 + dx, hy.round() as i32 + dy);
                if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                    let rim = dx.abs() == 2 || dy.abs() == 2;
                    img.put_pixel(x as u32, y as u32, if rim { Rgba([20, 30, 40, 255]) } else { Rgba([255, 255, 255, 255]) });
                }
            }
        }
    }
}

fn draw_box(img: &mut RgbaImage, l: &Layer) {
    let (w, h) = l.size_px();
    let (cx, cy) = l.center_px();
    let (s, c) = l.rotation.to_radians().sin_cos();
    let corner = |lx: f32, ly: f32| (cx + lx * c - ly * s, cy + lx * s + ly * c);
    let (hw, hh) = (w as f32 / 2.0, h as f32 / 2.0);
    let pts = [corner(-hw, -hh), corner(hw, -hh), corner(hw, hh), corner(-hw, hh)];
    for k in 0..4 {
        let (a, b) = (pts[k], pts[(k + 1) % 4]);
        let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as usize).max(1);
        for t in 0..=steps {
            let f = t as f32 / steps as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            if x >= 0.0 && y >= 0.0 && (x as u32) < img.width() && (y as u32) < img.height() {
                // dashed cyan/black so it shows on any color
                let on = (t / 3) % 2 == 0;
                img.put_pixel(x as u32, y as u32, if on { Rgba([0, 200, 255, 255]) } else { Rgba([0, 0, 0, 255]) });
            }
        }
    }
}

fn redraw(app: &AppWindow, st: &Shared) {
    let s = st.borrow();
    let Some(r) = &s.renderer else { return };
    let (w, h) = vp_size(app);
    let scale = app.window().scale_factor();
    let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);
    if pw == 0 || ph == 0 {
        return;
    }
    let highlight = s.hover.map(|(p, f)| {
        let r = template::region(p, f);
        let (tw, th) = (template::W as f32, template::H as f32);
        ([r.x as f32 / tw, r.y as f32 / th, (r.x + r.w) as f32 / tw, (r.y + r.h) as f32 / th], avatar::texture_flag(Some(p)))
    });
    let tex = r.render(pw, ph, s.cam.view_proj(w / h), highlight);
    match slint::Image::try_from(tex) {
        Ok(img) => app.set_viewport_image(img),
        Err(e) => app.set_status(format!("Viewport error: {e:?}").into()),
    }
}

// ---------------------------------------------------------------- files

/// Open-file dialog, then `then` on the UI thread. rfd's async dialog runs on its own thread:
/// on the UI thread (shared with winit/wgpu) the Windows shell dialog was very slow to list
/// folders and froze the app meanwhile.
fn pick_then(app: &AppWindow, st: &Shared, exts: &[&str], then: fn(&AppWindow, &Shared, &Path)) {
    let dialog = rfd::AsyncFileDialog::new().add_filter("Supported files", exts);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        if let (Some(file), Some(app)) = (dialog.pick_file().await, w.upgrade()) {
            then(&app, &st, file.path());
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the file dialog: {e}").into());
    }
}

fn parse_parts(v: &str) -> Option<[bool; 3]> {
    let b: Vec<bool> = v.split(',').map(|x| x.trim() == "1").collect();
    (b.len() == 3).then(|| [b[0], b[1], b[2]])
}

fn apply_part_visibility(s: &mut State, [head, arms, legs]: [bool; 3]) {
    for (idx, show) in [(&avatar::HEAD[..], head), (&avatar::ARMS[..], arms), (&avatar::LEGS[..], legs)] {
        idx.iter().for_each(|&i| s.rig[i].hidden = !show);
    }
}

/// Rebuild the display mesh (block rig or loaded OBJ) after part visibility changed.
fn rebuild_mesh(app: &AppWindow, st: &Shared) {
    let mut s = st.borrow_mut();
    let mesh = match s.obj_text.clone() {
        Some(text) => match avatar::load_obj(&text, avatar::R15_OFFSET, &s.rig) {
            Ok(m) => m,
            Err(e) => return app.set_status(format!("Could not rebuild rig: {e}").into()),
        },
        None => avatar::mesh(&s.rig),
    };
    if let Some(r) = s.renderer.as_mut() {
        r.set_mesh(&mesh.0, &mesh.1);
    }
    s.mesh = mesh;
    drop(s);
    redraw(app, st);
}

fn open_rig(app: &AppWindow, st: &Shared, path: &Path) {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string());
    let res = text.and_then(|t| avatar::load_obj(&t, avatar::R15_OFFSET, &st.borrow().rig).map(|m| (t, m)));
    match res {
        Ok((text, mesh)) => {
            let mut s = st.borrow_mut();
            if let Some(r) = s.renderer.as_mut() {
                r.set_mesh(&mesh.0, &mesh.1);
            }
            app.set_status(format!("Loaded rig {} ({} triangles)", path.display(), mesh.1.len() / 3).into());
            s.mesh = mesh;
            s.obj_text = Some(Rc::new(text));
            // Remember it for next launch; failure only costs the convenience.
            library::write_setting("last_rig", &path.to_string_lossy());
            drop(s);
            redraw(app, st);
        }
        Err(e) => app.set_status(format!("Could not load rig {}: {e}", path.display()).into()),
    }
}

// ---------------------------------------------------------------- background removal

enum BgMsg {
    Progress(String),
    Done(Result<image::GrayImage, String>),
}

/// "Remove background" checkbox. Instant when the layer already has a mask; otherwise runs ISNet
/// on a worker thread, downloading the model first (after asking) if it isn't there yet.
fn remove_bg_toggled(app: &AppWindow, st: &Shared, on: bool) {
    let has_mask = {
        let s = st.borrow();
        let Some(l) = s.selected.and_then(|i| s.doc.layers.get(i)) else { return };
        l.mask.is_some()
    };
    if !on || has_mask {
        return edit_selected(app, st, None, |l| {
            l.remove_bg = on;
            l.rebuild_work();
        });
    }
    if st.borrow().bg_job.is_some() {
        return; // already working on one
    }
    if bgremove::model_present() {
        return start_bg_job(app, st, false);
    }
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Download background-removal model?")
        .set_description(format!(
            "Background removal uses the ISNet AI model ({:.0} MB), downloaded once from github.com/danielgatis/rembg.\n\nIt is saved in %APPDATA%\\clothing-studio\\models and afterwards runs entirely on this PC, offline. Download it now?\n\n(Model {})",
            bgremove::MODEL_BYTES as f64 / 1_000_000.0,
            bgremove::model_problem().unwrap_or_else(|| "found".into())
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .set_level(rfd::MessageLevel::Info);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        let yes = dialog.show().await == rfd::MessageDialogResult::Yes;
        let Some(app) = w.upgrade() else { return };
        if yes {
            start_bg_job(&app, &st, true);
        } else {
            app.set_status("Background removal needs the model; nothing was downloaded.".into());
            sync_props(&app, &st.borrow()); // untick the checkbox
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the dialog: {e}").into());
    }
}

fn start_bg_job(app: &AppWindow, st: &Shared, download: bool) {
    let source = {
        let s = st.borrow();
        let Some(l) = s.selected.and_then(|i| s.doc.layers.get(i)) else { return };
        l.source.clone()
    };
    let img = (*source).clone(); // owned copy for the worker thread
    let (tx, rx) = std::sync::mpsc::channel::<BgMsg>();
    std::thread::spawn(move || {
        if download {
            let total = bgremove::MODEL_BYTES as f64 / 1_000_000.0;
            let last = std::cell::Cell::new(0u64);
            let res = bgremove::download_model(|n| {
                let mb = n / 1_000_000;
                if mb != last.get() {
                    last.set(mb);
                    let _ = tx.send(BgMsg::Progress(format!("Downloading background-removal model… {mb} / {total:.0} MB")));
                }
            });
            if let Err(e) = res {
                let _ = tx.send(BgMsg::Done(Err(e)));
                return;
            }
        }
        let _ = tx.send(BgMsg::Progress("Removing background…".into()));
        let _ = tx.send(BgMsg::Done(bgremove::compute_mask(&img)));
    });
    app.set_bg_busy(true);
    app.set_status(if download { "Starting download…" } else { "Removing background…" }.into());
    let started = std::time::Instant::now();
    let timer = slint::Timer::default();
    let (w, st2) = (app.as_weak(), st.clone());
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), move || {
        let Some(app) = w.upgrade() else { return };
        let result = loop {
            match rx.try_recv() {
                Ok(BgMsg::Progress(p)) => app.set_status(p.into()),
                Ok(BgMsg::Done(r)) => break r,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break Err("the background task stopped unexpectedly".into()),
            }
        };
        finish_bg_job(&app, &st2, &source, result, started.elapsed());
    });
    st.borrow_mut().bg_job = Some(timer);
}

fn finish_bg_job(app: &AppWindow, st: &Shared, source: &Rc<RgbaImage>, result: Result<image::GrayImage, String>, took: std::time::Duration) {
    // Stop polling. The timer is dropped on the next event-loop turn, not inside its own callback.
    if let Some(t) = st.borrow().bg_job.as_ref() {
        t.stop();
    }
    let st2 = st.clone();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || st2.borrow_mut().bg_job = None);
    app.set_bg_busy(false);
    match result {
        Ok(mask) => {
            let mask = Rc::new(bgremove::mask_to_rgba(&mask));
            let mut s = st.borrow_mut();
            s.checkpoint(None);
            // Every layer using this image (e.g. duplicates) gets the mask.
            for l in s.doc.layers.iter_mut().filter(|l| Rc::ptr_eq(&l.source, source)) {
                l.mask = Some(mask.clone());
                l.remove_bg = true;
                l.rebuild_work();
            }
            drop(s);
            app.set_status(format!("Background removed in {:.1} s. Untick to bring it back.", took.as_secs_f32()).into());
            layers_changed(app, st);
        }
        Err(e) => {
            app.set_status(format!("Background removal failed: {e}").into());
            sync_props(app, &st.borrow());
        }
    }
}

// ---------------------------------------------------------------- projects

type Then = Option<fn(&AppWindow, &Shared)>;

fn autosave_path() -> Option<std::path::PathBuf> {
    library::app_dir().map(|d| d.join(format!("autosave.{}", project::EXTENSION)))
}

fn remove_autosave() {
    if let Some(p) = autosave_path() {
        let _ = std::fs::remove_file(p);
    }
}

fn is_dirty(s: &State) -> bool {
    s.edits != s.saved_edits
}

fn project_name(s: &State) -> String {
    s.project_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| project::strip_extension(&n.to_string_lossy()).to_owned())
        .unwrap_or_else(|| "Untitled".into())
}

fn update_title(app: &AppWindow, s: &State) {
    let mark = if is_dirty(s) { " •" } else { "" };
    app.set_window_title(format!("{}{mark} — Clothing Studio", project_name(s)).into());
}

/// Name of the dropdown entry currently selected, if any (stored in the project for reference).
fn current_template_name(app: &AppWindow, s: &State) -> Option<String> {
    usize::try_from(app.get_preset_index()).ok().and_then(|i| template_names(s).get(i).cloned())
}

fn save_to(app: &AppWindow, st: &Shared, path: &Path) -> Result<(), String> {
    let mut s = st.borrow_mut();
    let json = project_json(app, &mut s)?;
    project::write_atomic(path, &json).map_err(|e| e.to_string())?;
    s.project_path = Some(path.to_path_buf());
    s.saved_edits = s.edits;
    s.autosaved_edits = s.edits;
    library::write_setting("last_project", &path.to_string_lossy());
    update_title(app, &s);
    drop(s);
    remove_autosave();
    app.set_status(format!("Saved {}", path.display()).into());
    Ok(())
}

/// Save to the current file, or ask where (Save As) if the project has never been saved.
fn save_project(app: &AppWindow, st: &Shared, then: Then) {
    let path = st.borrow().project_path.clone();
    match path {
        Some(p) => match save_to(app, st, &p) {
            Ok(()) => then.into_iter().for_each(|f| f(app, st)),
            Err(e) => app.set_status(format!("Could not save: {e}").into()),
        },
        None => save_project_as(app, st, then),
    }
}

fn save_project_as(app: &AppWindow, st: &Shared, then: Then) {
    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter("Clothing Studio project", &[project::EXTENSION])
        .set_file_name(format!("{}.{}", project_name(&st.borrow()), project::EXTENSION));
    if let Some(dir) = st.borrow().project_path.as_ref().and_then(|p| p.parent()) {
        dialog = dialog.set_directory(dir);
    }
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        let (Some(file), Some(app)) = (dialog.save_file().await, w.upgrade()) else { return };
        let mut path = file.path().to_path_buf();
        // Always save with the current extension: "My shirt" / "My shirt.json" -> "My shirt.clothing"
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        path.set_file_name(format!("{}.{}", project::strip_extension(&name), project::EXTENSION));
        match save_to(&app, &st, &path) {
            Ok(()) => then.into_iter().for_each(|f| f(&app, &st)),
            Err(e) => app.set_status(format!("Could not save: {e}").into()),
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the file dialog: {e}").into());
    }
}

/// Run `then` now, or after asking to save unsaved changes (Save / Don't save / Cancel).
fn confirm_unsaved(app: &AppWindow, st: &Shared, then: fn(&AppWindow, &Shared)) {
    if !is_dirty(&st.borrow()) {
        return then(app, st);
    }
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Unsaved changes")
        .set_description(format!("Save changes to \"{}\" first?", project_name(&st.borrow())))
        .set_buttons(rfd::MessageButtons::YesNoCancelCustom("Save".into(), "Don't save".into(), "Cancel".into()))
        .set_level(rfd::MessageLevel::Warning);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        let answer = dialog.show().await;
        let Some(app) = w.upgrade() else { return };
        match answer {
            rfd::MessageDialogResult::Yes => save_project(&app, &st, Some(then)),
            rfd::MessageDialogResult::Custom(c) if c == "Save" => save_project(&app, &st, Some(then)),
            rfd::MessageDialogResult::No => then(&app, &st),
            rfd::MessageDialogResult::Custom(c) if c == "Don't save" => then(&app, &st),
            _ => {}
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the dialog: {e}").into());
    }
}

/// Start over with no layers, keeping the current template.
fn new_project(app: &AppWindow, st: &Shared) {
    let mut s = st.borrow_mut();
    s.doc.layers.clear();
    s.selected = None;
    s.undo.clear();
    s.redo.clear();
    // the other garment starts over too (templates are kept)
    s.other.doc.layers.clear();
    s.other.selected = None;
    s.other.undo.clear();
    s.other.redo.clear();
    s.other.composed = compose::compose(&s.other.doc);
    upload_other(&s);
    s.project_path = None;
    s.edits += 1;
    s.saved_edits = s.edits;
    s.autosaved_edits = s.edits;
    drop(s);
    remove_autosave();
    app.set_status("New project.".into());
    layers_changed(app, st);
}

fn open_project_dialog(app: &AppWindow, st: &Shared) {
    pick_then(app, st, &[project::EXTENSION, "json"], |a, s, p| {
        if let Err(e) = open_project_path(a, s, p) {
            a.set_status(format!("Could not open {}: {e}", p.display()).into());
        }
    });
}

fn open_project_path(app: &AppWindow, st: &Shared, path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let ((shirt, shirt_name), pants) = project::from_json(&text)?;
    let pants = pants.unwrap_or_else(|| {
        let fresh = Stash::new(Garment::Pants); // older, shirt-only project
        (fresh.doc, fresh.template_name)
    });
    let mut s = st.borrow_mut();
    let ((doc, template_name), (other_doc, other_name)) = match s.garment {
        Garment::Shirt => ((shirt, shirt_name), pants),
        Garment::Pants => (pants, (shirt, shirt_name)),
    };
    s.selected = doc.layers.len().checked_sub(1);
    s.doc = doc;
    s.undo.clear();
    s.redo.clear();
    s.other = Stash {
        selected: other_doc.layers.len().checked_sub(1),
        composed: compose::compose(&other_doc),
        doc: other_doc,
        undo: vec![],
        redo: vec![],
        custom_template: false,
        template_name: other_name,
    };
    upload_other(&s);
    s.project_path = Some(path.to_path_buf());
    s.edits += 1;
    s.saved_edits = s.edits;
    s.autosaved_edits = s.edits;
    select_matching_template(app, &mut s, template_name.as_deref());
    drop(s);
    library::write_setting("last_project", &path.to_string_lossy());
    app.set_status(format!("Opened {}", path.display()).into());
    layers_changed(app, st);
    Ok(())
}

/// Every few seconds: keep a copy of unsaved work so a crash or forced close loses nothing.
fn autosave_tick(app: &AppWindow, st: &Shared) {
    let mut s = st.borrow_mut();
    if !is_dirty(&s) || s.edits == s.autosaved_edits {
        return;
    }
    let Some(path) = autosave_path() else { return };
    let res = project_json(app, &mut s)
        .and_then(|json| {
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            project::write_atomic(&path, &json).map_err(|e| e.to_string())
        });
    match res {
        Ok(()) => {
            s.autosaved_edits = s.edits;
            let project = s.project_path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            library::write_setting("autosave_project", &project);
        }
        Err(e) => app.set_status(format!("Auto-save failed: {e}").into()),
    }
}

/// On start: offer to restore work that was never saved (the app was closed unexpectedly).
fn offer_restore(app: &AppWindow, st: &Shared) {
    let Some(path) = autosave_path().filter(|p| p.is_file()) else { return };
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Restore unsaved work?")
        .set_description("Clothing Studio was closed with unsaved changes. Restore them?")
        .set_buttons(rfd::MessageButtons::YesNo)
        .set_level(rfd::MessageLevel::Info);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        let answer = dialog.show().await;
        let Some(app) = w.upgrade() else { return };
        if answer != rfd::MessageDialogResult::Yes {
            return remove_autosave();
        }
        match open_project_path(&app, &st, &path) {
            Ok(()) => {
                let mut s = st.borrow_mut();
                // It's unsaved work: point back at the original file (if any) and mark it dirty.
                s.project_path = library::read_setting("autosave_project").map(Into::into);
                s.saved_edits = s.edits.wrapping_sub(1);
                update_title(&app, &s);
                drop(s);
                app.set_status("Restored your unsaved work. Save it with Ctrl+S.".into());
            }
            Err(e) => app.set_status(format!("Could not restore unsaved work: {e}").into()),
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the dialog: {e}").into());
    }
}

// ---------------------------------------------------------------- garments

fn other_garment(g: Garment) -> Garment {
    match g {
        Garment::Shirt => Garment::Pants,
        Garment::Pants => Garment::Shirt,
    }
}

/// Setting that remembers each garment's last template.
fn template_setting(g: Garment) -> &'static str {
    match g {
        Garment::Shirt => "last_template",
        Garment::Pants => "last_pants_template",
    }
}

/// A template by dropdown name for `garment`: one of its presets, or a saved template.
fn template_image(s: &State, garment: Garment, name: &str) -> Option<RgbaImage> {
    if let Some(i) = garment.preset_names().iter().position(|n| *n == name) {
        return Some(garment.preset(i));
    }
    let (_, path) = s.library.iter().find(|(n, _)| n == name)?;
    image::open(path).ok().map(|i| i.to_rgba8()).filter(|i| i.dimensions() == (template::W, template::H))
}

fn upload_other(s: &State) {
    if let Some(r) = &s.renderer {
        r.upload(other_garment(s.garment), &s.other.composed);
    }
}

/// Both garments as one project file (shirt first, whichever is being edited).
fn project_json(app: &AppWindow, s: &mut State) -> Result<String, String> {
    let active_name = current_template_name(app, s);
    let State { doc, other, png_cache, garment, .. } = s;
    let active = (&*doc, active_name.as_deref());
    let parked = (&other.doc, other.template_name.as_deref());
    let (shirt, pants) = if *garment == Garment::Shirt { (active, parked) } else { (parked, active) };
    project::to_json(shirt, pants, png_cache)
}

/// Switch between editing the shirt and the pants. Each keeps its own template, layers,
/// selection and undo history; the 3D view always shows both.
fn switch_garment(app: &AppWindow, st: &Shared, to: Garment) {
    {
        let mut guard = st.borrow_mut();
        if guard.garment == to {
            return;
        }
        let active_name = current_template_name(app, &guard);
        let s = &mut *guard;
        std::mem::swap(&mut s.doc, &mut s.other.doc);
        std::mem::swap(&mut s.selected, &mut s.other.selected);
        std::mem::swap(&mut s.composed, &mut s.other.composed);
        std::mem::swap(&mut s.undo, &mut s.other.undo);
        std::mem::swap(&mut s.redo, &mut s.other.redo);
        std::mem::swap(&mut s.custom_template, &mut s.other.custom_template);
        let name = std::mem::replace(&mut s.other.template_name, active_name);
        s.garment = to;
        s.last_key = None;
        s.hover = None;
        app.set_garment(if to == Garment::Pants { 1 } else { 0 });
        sync_template_list(app, s);
        select_matching_template(app, s, name.as_deref());
        app.set_status(format!("Editing the {}.", to.name()).into());
    }
    layers_changed(app, st); // layers panel, 2D view and 3D show the other garment now
}

// ---------------------------------------------------------------- templates

/// Dropdown entries: built-in presets, then saved templates.
fn template_names(s: &State) -> Vec<String> {
    let custom = s.custom_template.then(|| CUSTOM_TEMPLATE.to_owned());
    s.garment.preset_names().iter().map(|n| n.to_string()).chain(s.library.iter().map(|(n, _)| n.clone())).chain(custom).collect()
}

const CUSTOM_TEMPLATE: &str = "Custom template";

/// Point the dropdown at the entry whose pixels equal the template on screen (trying `hint`, a
/// name, first), or at "Custom template" if none does.
fn select_matching_template(app: &AppWindow, s: &mut State, hint: Option<&str>) {
    let names = template_names(s);
    let presets = s.garment.preset_names().len();
    let image_at = |i: usize| -> Option<RgbaImage> {
        if i < presets {
            Some(s.garment.preset(i))
        } else {
            s.library.get(i - presets).and_then(|(_, p)| image::open(p).ok()).map(|i| i.to_rgba8())
        }
    };
    let hinted = hint.and_then(|n| names.iter().position(|x| x == n));
    let found = hinted.into_iter().chain(0..names.len()).find(|&i| image_at(i).is_some_and(|img| img == *s.doc.base));
    match found {
        Some(i) => {
            if s.custom_template {
                s.custom_template = false; // "Custom template" is last, so `i` stays valid
                sync_template_list(app, s);
            }
            app.set_preset_index(i as i32);
        }
        None => mark_custom_template(app, s),
    }
}

/// Show "Custom template" (the base on screen isn't any preset or saved template) and select it.
fn mark_custom_template(app: &AppWindow, s: &mut State) {
    s.custom_template = true;
    sync_template_list(app, s);
    app.set_preset_index((template_names(s).len() - 1) as i32);
}

fn sync_template_list(app: &AppWindow, s: &State) {
    let names: Vec<slint::SharedString> = template_names(s).into_iter().map(Into::into).collect();
    app.set_template_names(ModelRc::new(VecModel::from(names)));
}

/// Apply dropdown entry `i` as the base template and remember it for next launch.
fn choose_template(app: &AppWindow, st: &Shared, i: usize) {
    let garment = st.borrow().garment;
    let presets = garment.preset_names().len();
    let (name, img) = if i < presets {
        (garment.preset_names()[i].to_owned(), Ok(garment.preset(i)))
    } else {
        let Some((name, path)) = st.borrow().library.get(i - presets).cloned() else { return };
        (name, image::open(&path).map(|i| i.to_rgba8()))
    };
    match img {
        Ok(img) if img.dimensions() == (template::W, template::H) => {
            let mut s = st.borrow_mut();
            s.checkpoint(None);
            s.doc.base = Rc::new(img);
            drop(s);
            if st.borrow().custom_template {
                let mut s = st.borrow_mut();
                s.custom_template = false;
                sync_template_list(app, &s);
            }
            app.set_preset_index(i as i32);
            library::write_setting(template_setting(garment), &name);
            app.set_status(format!("Template: {name}").into());
            refresh(app, st);
        }
        Ok(_) => app.set_status(format!("Saved template \"{name}\" is not 585×559.").into()),
        Err(e) => app.set_status(format!("Could not open saved template \"{name}\": {e}").into()),
    }
}

/// Add a template image to the library (deduplicated), refresh the dropdown and select it.
fn add_to_library(app: &AppWindow, st: &Shared, name: &str, img: &RgbaImage) -> Result<(), String> {
    let dir = library::templates_dir().ok_or("no APPDATA folder to save templates in")?;
    let saved = library::save(&dir, name, img).map_err(|e| e.to_string())?;
    let mut s = st.borrow_mut();
    s.library = library::list(&dir);
    let i = s.library.iter().position(|(_, p)| *p == saved).ok_or("saved template not found")?;
    sync_template_list(app, &s);
    drop(s);
    let presets = st.borrow().garment.preset_names().len();
    choose_template(app, st, presets + i);
    Ok(())
}

/// Delete the selected saved template after confirmation. The shirt on screen is left as is, so
/// nothing in the current design is lost.
fn delete_template(app: &AppWindow, st: &Shared) {
    let presets = template::PRESET_NAMES.len();
    let selected = usize::try_from(app.get_preset_index()).ok().and_then(|i| i.checked_sub(presets));
    let Some((name, path)) = selected.and_then(|i| st.borrow().library.get(i).cloned()) else {
        return app.set_status("Select one of your saved templates to delete it (built-in presets can't be deleted).".into());
    };
    let confirm = rfd::AsyncMessageDialog::new()
        .set_title("Delete template")
        .set_description(format!("Delete the saved template \"{name}\"?\n\nThe file is removed from your template library and can't be undone. The shirt currently on screen is not changed."))
        .set_buttons(rfd::MessageButtons::YesNo)
        .set_level(rfd::MessageLevel::Warning);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        if confirm.show().await != rfd::MessageDialogResult::Yes {
            return;
        }
        let Some(app) = w.upgrade() else { return };
        if let Err(e) = std::fs::remove_file(&path) {
            return app.set_status(format!("Could not delete \"{name}\": {e}").into());
        }
        for g in Garment::ALL {
            if library::read_setting(template_setting(g)).as_deref() == Some(name.as_str()) {
                library::write_setting(template_setting(g), "");
            }
        }
        let mut s = st.borrow_mut();
        s.library = library::templates_dir().map(|d| library::list(&d)).unwrap_or_default();
        sync_template_list(&app, &s);
        mark_custom_template(&app, &mut s); // no entry matches what's on screen any more
        app.set_status(format!("Deleted template \"{name}\". The shirt on screen is unchanged.").into());
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the confirmation dialog: {e}").into());
    }
}

/// Save the current design (template + layers, no outline) into the library as a new template.
fn save_template(app: &AppWindow, st: &Shared) {
    let Some(dir) = library::templates_dir() else { return app.set_status("No APPDATA folder to save templates in.".into()) };
    let _ = std::fs::create_dir_all(&dir);
    let dialog = rfd::AsyncFileDialog::new().add_filter("PNG", &["png"]).set_directory(&dir).set_file_name("My template.png");
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        let (Some(file), Some(app)) = (dialog.save_file().await, w.upgrade()) else { return };
        let img = compose::compose(&st.borrow().doc);
        let name = file.path().file_stem().map_or("Template".into(), |s| s.to_string_lossy().into_owned());
        // Saved outside the library? Keep a copy in it anyway so it shows in the dropdown.
        let res = save_png(&img, file.path()).map_err(|e| e.to_string());
        if let Err(e) = res.and_then(|_| add_to_library(&app, &st, &name, &img)) {
            app.set_status(format!("Could not save template: {e}").into());
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the file dialog: {e}").into());
    }
}

fn open_template(app: &AppWindow, st: &Shared, path: &Path) {
    match image::open(path) {
        Ok(img) if img.width() == template::W && img.height() == template::H => {
            let name = path.file_stem().map_or("Template".into(), |s| s.to_string_lossy().into_owned());
            if let Err(e) = add_to_library(app, st, &name, &img.to_rgba8()) {
                // Still usable this session even if it couldn't be saved.
                let mut s = st.borrow_mut();
                s.checkpoint(None);
                s.doc.base = Rc::new(img.to_rgba8());
                drop(s);
                app.set_status(format!("Loaded {} but could not save it to your templates: {e}", path.display()).into());
                refresh(app, st);
            }
        }
        Ok(img) => app.set_status(
            format!("{} is {}×{}; classic shirts must be {}×{}.", path.display(), img.width(), img.height(), template::W, template::H)
                .into(),
        ),
        Err(e) => app.set_status(format!("Could not open {}: {e}", path.display()).into()),
    }
}

fn open_art(app: &AppWindow, st: &Shared, path: &Path) {
    match image::open(path) {
        Ok(img) => {
            let name = path.file_stem().map_or("Artwork".into(), |s| s.to_string_lossy().into_owned());
            add_layer(app, st, name, img.to_rgba8());
        }
        Err(e) => app.set_status(format!("Could not open {}: {e}", path.display()).into()),
    }
}

fn st_selected_is_front_or_back(s: &State) -> bool {
    s.selected.and_then(|i| s.doc.layers.get(i)).is_some_and(|l| matches!(l.face, Face::Front | Face::Back))
}

// ---------------------------------------------------------------- updates

/// Ask GitHub for a newer release. Quiet on start (no message when offline or up to date);
/// `manual` (Help menu) reports every outcome.
fn check_for_updates(app: &AppWindow, st: &Shared, manual: bool) {
    let current = env!("CARGO_PKG_VERSION");
    if manual {
        app.set_status("Checking for updates…".into());
    }
    spawn_net_job(app, st, |_, _| {}, move |_| updater::check(current), move |app, st, res| match res {
        Ok(Some(release)) => offer_update(app, st, release),
        Ok(None) if manual => app.set_status(format!("You have the latest version ({current}).").into()),
        Err(e) if manual => app.set_status(format!("Couldn't check for updates: {e}").into()),
        _ => {}
    });
}

fn offer_update(app: &AppWindow, st: &Shared, release: updater::Release) {
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Update available")
        .set_description(format!(
            "Clothing Studio {} is available (you have {}).

Download and install it now? Clothing Studio will close; any unsaved work is kept and offered back when it reopens.",
            release.version,
            env!("CARGO_PKG_VERSION")
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .set_level(rfd::MessageLevel::Info);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        let yes = dialog.show().await == rfd::MessageDialogResult::Yes;
        let Some(app) = w.upgrade() else { return };
        if !yes {
            return app.set_status(format!("Update {} skipped. Help → Check for updates… to get it later.", release.version).into());
        }
        let version = release.version.clone();
        spawn_net_job(
            &app,
            &st,
            |_, _| {},
            move |progress| updater::download(&release, |n| progress(format!("Downloading update {version}… {:.1} MB", n as f64 / 1_000_000.0))),
            |app, st, res| match res {
                Ok(installer) => {
                    autosave_tick(app, st); // keep unsaved work; it's offered back after the update
                    match std::process::Command::new(&installer).spawn() {
                        Ok(_) => slint::quit_event_loop().unwrap_or(()),
                        Err(e) => app.set_status(format!("Couldn't start the installer ({e}); it's at {}", installer.display()).into()),
                    }
                }
                Err(e) => app.set_status(format!("Update failed: {e}").into()),
            },
        );
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the dialog: {e}").into());
    }
}

// ---------------------------------------------------------------- AI upscaling

/// "Upscale ×4 (AI)" checkbox: instant if this picture was upscaled before; otherwise runs the
/// model on a worker thread, downloading it first (after asking) if needed.
fn upscale_toggled(app: &AppWindow, st: &Shared, on: bool) {
    let (has, source) = {
        let s = st.borrow();
        let Some(l) = s.selected.and_then(|i| s.doc.layers.get(i)) else { return };
        (l.upscaled.is_some(), l.source.clone())
    };
    if !on || has {
        return edit_selected(app, st, None, |l| {
            l.upscale = on;
            l.rebuild_work();
        });
    }
    if source.width().max(source.height()) > upscale::MAX_SIDE {
        app.set_status(format!("This picture is already {}×{}: big enough that upscaling won't add detail on the shirt.", source.width(), source.height()).into());
        return sync_props(app, &st.borrow());
    }
    if upscale::model_present() {
        return start_upscale(app, st, source, false);
    }
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Download upscaling model?")
        .set_description(format!(
            "AI upscaling uses Real-ESRGAN (general x4v3, BSD-3 licence) from Qualcomm AI Hub, {:.1} MB, downloaded once.\n\nIt is saved in %APPDATA%\\clothing-studio\\models and afterwards runs entirely on this PC, offline. Download it now?",
            upscale::ZIP_BYTES as f64 / 1_000_000.0
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .set_level(rfd::MessageLevel::Info);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        let yes = dialog.show().await == rfd::MessageDialogResult::Yes;
        let Some(app) = w.upgrade() else { return };
        if yes {
            start_upscale(&app, &st, source, true);
        } else {
            app.set_status("Upscaling needs the model; nothing was downloaded.".into());
            sync_props(&app, &st.borrow());
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the dialog: {e}").into());
    }
}

fn start_upscale(app: &AppWindow, st: &Shared, source: Rc<RgbaImage>, download: bool) {
    let img = (*source).clone(); // owned copy for the worker thread
    let started = std::time::Instant::now();
    spawn_net_job(
        app,
        st,
        AppWindow::set_upscale_busy,
        move |progress| {
            if download {
                let total = upscale::ZIP_BYTES as f64 / 1_000_000.0;
                upscale::download_model(|n| progress(format!("Downloading upscaling model… {:.1} / {total:.1} MB", n as f64 / 1_000_000.0)))?;
            }
            upscale::upscale(&img, progress)
        },
        move |app, st, res| match res {
            Ok(big) => {
                let (w, h) = big.dimensions();
                let big = Rc::new(big);
                let mut s = st.borrow_mut();
                s.checkpoint(None);
                // every layer using this picture (e.g. duplicates) gets the upscaled copy
                for l in s.doc.layers.iter_mut().filter(|l| Rc::ptr_eq(&l.source, &source)) {
                    l.upscaled = Some(big.clone());
                    l.upscale = true;
                    l.rebuild_work();
                }
                drop(s);
                app.set_status(format!("Upscaled to {w}×{h} in {:.1} s. Untick to compare with the original.", started.elapsed().as_secs_f32()).into());
                layers_changed(app, st);
            }
            Err(e) => {
                app.set_status(format!("Upscaling failed: {e}").into());
                sync_props(app, &st.borrow());
            }
        },
    );
}

// ---------------------------------------------------------------- Roblox upload

/// Run `work` on a worker thread (it may report progress text), then `done` on the UI thread.
/// One network job at a time.
fn spawn_net_job<T: Send + 'static>(
    app: &AppWindow,
    st: &Shared,
    busy: fn(&AppWindow, bool),
    work: impl FnOnce(&dyn Fn(String)) -> T + Send + 'static,
    done: impl FnOnce(&AppWindow, &Shared, T) + 'static,
) {
    if st.borrow().net_job.is_some() {
        return app.set_status("Still busy with the last request — wait for it to finish.".into());
    }
    enum Msg<T> {
        Progress(String),
        Done(T),
    }
    let (tx, rx) = std::sync::mpsc::channel::<Msg<T>>();
    std::thread::spawn(move || {
        let ptx = tx.clone();
        let result = work(&move |p| {
            let _ = ptx.send(Msg::Progress(p));
        });
        let _ = tx.send(Msg::Done(result));
    });
    busy(app, true);
    let mut done = Some(done);
    let timer = slint::Timer::default();
    let (w, st2) = (app.as_weak(), st.clone());
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), move || {
        let Some(app) = w.upgrade() else { return };
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress(p)) => app.set_status(p.into()),
                Ok(Msg::Done(result)) => {
                    if let Some(t) = st2.borrow().net_job.as_ref() {
                        t.stop();
                    }
                    let st3 = st2.clone();
                    slint::Timer::single_shot(std::time::Duration::ZERO, move || st3.borrow_mut().net_job = None);
                    busy(&app, false);
                    if let Some(f) = done.take() {
                        f(&app, &st2, result);
                    }
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if let Some(t) = st2.borrow().net_job.as_ref() {
                        t.stop();
                    }
                    let st3 = st2.clone();
                    slint::Timer::single_shot(std::time::Duration::ZERO, move || st3.borrow_mut().net_job = None);
                    busy(&app, false);
                    app.set_status("The background task stopped unexpectedly.".into());
                    return;
                }
            }
        }
    });
    st.borrow_mut().net_job = Some(timer);
}

/// Check a pasted key with Roblox, then store it in Credential Manager.
fn roblox_key_save(app: &AppWindow, st: &Shared, key: String) {
    let key = key.trim().to_owned();
    if key.is_empty() {
        return app.set_roblox_status("Paste your API key first.".into());
    }
    app.set_roblox_status("Checking the key with Roblox…".into());
    let check = key.clone();
    spawn_net_job(app, st, AppWindow::set_roblox_busy, move |_| roblox::introspect(&check), move |app, _st, res| match res {
        Ok(info) => match roblox::save_key(&key) {
            Ok(()) => {
                library::write_setting("roblox_user_id", &info.user_id);
                app.set_roblox_key_input("".into()); // don't keep the secret in the UI
                app.set_roblox_has_key(true);
                app.set_roblox_status(format!("Saved \"{}\". Uploads go to Roblox user {}. You can close this.", info.name, info.user_id).into());
            }
            Err(e) => app.set_roblox_status(e.into()),
        },
        Err(e) => app.set_roblox_status(format!("That key didn't work: {e}").into()),
    });
}

/// Upload the garment being edited; copy rbxassetid://ID for pasting into Studio.
fn upload_to_roblox(app: &AppWindow, st: &Shared) {
    let (Some(key), Some(user)) = (roblox::load_key(), library::read_setting("roblox_user_id")) else {
        app.set_roblox_status("Add your Open Cloud API key to upload. Until then, Export PNG… and drag the file into Studio.".into());
        return app.invoke_show_roblox_settings();
    };
    let (png, name, template) = {
        let s = st.borrow();
        let img = compose::compose(&s.doc);
        let mut png = Vec::new();
        if let Err(e) = img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png) {
            return app.set_status(format!("Could not encode the PNG: {e}").into());
        }
        let template = if s.garment == Garment::Pants { "Pants.PantsTemplate" } else { "Shirt.ShirtTemplate" };
        (png, format!("{} {}", project_name(&s), s.garment.name()), template)
    };
    spawn_net_job(app, st, AppWindow::set_roblox_busy, move |progress| roblox::upload_png(&key, &user, &name, &png, progress), move |app, _st, res| match res {
        Ok(up) => {
            let id = format!("rbxassetid://{}", up.asset_id);
            let copied = arboard::Clipboard::new().and_then(|mut c| c.set_text(id.clone())).is_ok();
            let review = match up.moderation.as_deref() {
                Some(m) if m.ends_with("APPROVED") => "",
                Some(_) => " Roblox is still reviewing it; it may look blank in Studio until approved.",
                None => "",
            };
            app.set_status(
                format!(
                    "Uploaded: {id}{}. In Studio, paste it into {template}.{review}",
                    if copied { " (copied)" } else { "" }
                )
                .into(),
            );
        }
        Err(e) => app.set_status(format!("Upload failed: {e}").into()),
    });
}

/// Recolour the garment's template to the selected picture's background (its edge colour), so
/// a photo with a plain background blends into the shirt. Keeps the template's shape; undoable.
fn match_base_to_art(app: &AppWindow, st: &Shared) {
    let mut s = st.borrow_mut();
    let Some(src) = s.selected.and_then(|i| s.doc.layers.get(i)).map(|l| l.source.clone()) else { return };
    let Some(rgb) = template::border_colour(&src) else {
        return app.set_status("That picture's edges are transparent, so there's no background colour to match.".into());
    };
    s.checkpoint(None);
    s.doc.base = Rc::new(template::recolour(&s.doc.base, rgb));
    mark_custom_template(app, &mut s);
    let garment = s.garment.name();
    drop(s);
    refresh(app, st);
    app.set_status(format!("The {garment} is now #{:02x}{:02x}{:02x} to match the picture's background (Ctrl+Z to undo).", rgb[0], rgb[1], rgb[2]).into());
}

// ---------------------------------------------------------------- text layers

/// Layer list name for a text layer: its text, shortened.
fn text_layer_name(t: &str) -> String {
    let t = t.trim();
    let short: String = t.chars().take(28).collect();
    if t.is_empty() { "Text".into() } else if short.len() < t.len() { format!("{short}…") } else { short }
}

fn add_text_layer(app: &AppWindow, st: &Shared) {
    let spec = text::TextSpec::default();
    match text::render(&spec) {
        Ok(img) => {
            add_layer(app, st, text_layer_name(&spec.text), img);
            if let Some(l) = st.borrow_mut().doc.layers.last_mut() {
                l.text = Some(spec);
            }
            sync_props(app, &st.borrow());
            app.set_status("Text added — type in Properties → Text, then drag it into place.".into());
        }
        Err(e) => app.set_status(format!("Could not add text: {e}").into()),
    }
}

/// Change the selected text layer's settings and re-render it (undoable; same `key` edits merge).
fn edit_text(app: &AppWindow, st: &Shared, key: Option<&'static str>, f: impl FnOnce(&mut text::TextSpec)) {
    let mut spec = {
        let s = st.borrow();
        let Some(t) = s.selected.and_then(|i| s.doc.layers.get(i)).and_then(|l| l.text.clone()) else { return };
        t
    };
    f(&mut spec);
    match text::render(&spec) {
        Ok(img) => {
            let name = text_layer_name(&spec.text);
            edit_selected(app, st, key, |l| {
                l.source = Rc::new(img);
                (l.mask, l.remove_bg) = (None, false); // a mask from old text wouldn't fit
                l.name = name;
                l.text = Some(spec);
                l.rebuild_work();
            });
            sync_layers(app, &st.borrow()); // name and thumbnail
        }
        Err(e) => app.set_status(format!("Could not render the text: {e}").into()),
    }
}

// ---------------------------------------------------------------- shading

/// Shading menu: None, the user's shading library, the built-in generated ones, and (if the
/// current overlay came from elsewhere, e.g. an older project) an "Imported" entry.
// ---------------------------------------------------------------- tattoos

/// Tattoos menu: None, the user's tattoo library, and an "Imported" entry for a tattoo that isn't
/// in the library (e.g. from a project made elsewhere).
fn sync_tattoo(app: &AppWindow, s: &State) {
    let lib = &s.tattoo_library;
    let mut names: Vec<slint::SharedString> = std::iter::once("None".to_owned()).chain(lib.iter().map(|(n, _)| n.clone())).map(Into::into).collect();
    let index = match &s.doc.underlay {
        None => 0,
        Some(u) => match lib.iter().position(|(n, _)| *n == u.name) {
            Some(i) => 1 + i,
            None => {
                names.push(format!("Imported: {}", u.name).into());
                names.len() - 1
            }
        },
    };
    app.set_tattoo_names(ModelRc::new(VecModel::from(names)));
    app.set_tattoo_index(index as i32);
    app.set_tattoo_strength(s.doc.underlay.as_ref().map_or(1.0, |u| u.strength));
    app.set_tattoo_ink(s.doc.underlay.as_ref().is_some_and(|u| u.ink));
}

fn tattoo_chosen(app: &AppWindow, st: &Shared, i: i32) {
    let Ok(i) = usize::try_from(i) else { return };
    let mut s = st.borrow_mut();
    if i > s.tattoo_library.len() {
        return; // the "Imported" entry: already applied
    }
    let (strength, ink) = s.doc.underlay.as_ref().map_or((1.0, false), |u| (u.strength, u.ink));
    let underlay = if i == 0 {
        None
    } else {
        let (name, path) = s.tattoo_library[i - 1].clone();
        match image::open(&path).map(|img| img.to_rgba8()) {
            Ok(img) if img.dimensions() == (template::W, template::H) => Some(compose::Overlay { image: Rc::new(img), strength, preset: None, name, ink }),
            Ok(_) => return app.set_status(format!("Tattoo \"{name}\" isn't 585×559.").into()),
            Err(e) => return app.set_status(format!("Could not open tattoo \"{name}\": {e}").into()),
        }
    };
    s.checkpoint(None);
    s.doc.underlay = underlay;
    drop(s);
    refresh(app, st);
    sync_tattoo(app, &st.borrow());
}

/// Add a 585x559 tattoo PNG to the tattoo library and put it under the template.
fn import_tattoo(app: &AppWindow, st: &Shared, path: &Path) {
    match image::open(path) {
        Ok(img) if (img.width(), img.height()) == (template::W, template::H) => {
            let img = img.to_rgba8();
            let mut name = path.file_stem().map_or("tattoo".into(), |s| s.to_string_lossy().into_owned());
            if let Some(dir) = library::tattoos_dir() {
                match library::save(&dir, &name, &img) {
                    Ok(saved) => {
                        name = saved.file_stem().map_or(name, |s| s.to_string_lossy().into_owned());
                        st.borrow_mut().tattoo_library = library::list(&dir);
                    }
                    Err(e) => app.set_status(format!("Using the tattoo, but couldn't save it to your library: {e}").into()),
                }
            }
            let mut s = st.borrow_mut();
            s.checkpoint(None);
            let (strength, ink) = s.doc.underlay.as_ref().map_or((1.0, false), |u| (u.strength, u.ink));
            s.doc.underlay = Some(compose::Overlay { image: Rc::new(img), strength, preset: None, name: name.clone(), ink });
            let bare = s.doc.base.pixels().any(|p| p[3] < 255);
            drop(s);
            app.set_status(
                if bare {
                    format!("Tattoo \"{name}\" added under the template; it shows where the template is transparent (bare arms).")
                } else {
                    format!("Tattoo \"{name}\" added, but this template has no transparent areas, so it's hidden. Pick a short-sleeve template.")
                }
                .into(),
            );
            refresh(app, st);
            sync_tattoo(app, &st.borrow());
        }
        Ok(img) => app.set_status(format!("A tattoo template must be 585×559; that image is {}×{}.", img.width(), img.height()).into()),
        Err(e) => app.set_status(format!("Could not open {}: {e}", path.display()).into()),
    }
}

fn edit_tattoo(app: &AppWindow, st: &Shared, key: Option<&'static str>, f: impl FnOnce(&mut compose::Overlay)) {
    let mut s = st.borrow_mut();
    if s.doc.underlay.is_none() {
        return;
    }
    s.checkpoint(key);
    if let Some(u) = s.doc.underlay.as_mut() {
        f(u);
    }
    drop(s);
    refresh(app, st);
}

fn sync_shading(app: &AppWindow, s: &State) {
    let lib = &s.shading_library;
    let mut names: Vec<slint::SharedString> = std::iter::once("None".to_owned())
        .chain(lib.iter().map(|(n, _)| n.clone()))
        .chain(shading::PRESETS.iter().map(|p| format!("{p} (built-in)")))
        .map(Into::into)
        .collect();
    let index = match &s.doc.overlay {
        None => 0,
        Some(o) => match o.preset {
            Some(p) => 1 + lib.len() + p,
            None => match lib.iter().position(|(n, _)| *n == o.name) {
                Some(i) => 1 + i,
                None => {
                    names.push(format!("Imported: {}", o.name).into());
                    names.len() - 1
                }
            },
        },
    };
    sync_tattoo(app, s);
    app.set_shading_names(ModelRc::new(VecModel::from(names)));
    app.set_shading_index(index as i32);
    app.set_shading_strength(s.doc.overlay.as_ref().map_or(0.5, |o| o.strength));
}

fn shading_chosen(app: &AppWindow, st: &Shared, i: i32) {
    let Ok(i) = usize::try_from(i) else { return };
    let lib_len = st.borrow().shading_library.len();
    let mut s = st.borrow_mut();
    let strength = s.doc.overlay.as_ref().map_or(0.5, |o| o.strength);
    let overlay = if i == 0 {
        None
    } else if i <= lib_len {
        let (name, path) = s.shading_library[i - 1].clone();
        match image::open(&path).map(|img| img.to_rgba8()) {
            Ok(img) if img.dimensions() == (template::W, template::H) => Some(compose::Overlay { image: Rc::new(img), strength, preset: None, name, ink: false }),
            Ok(_) => return app.set_status(format!("Shading \"{name}\" isn't 585×559.").into()),
            Err(e) => return app.set_status(format!("Could not open shading \"{name}\": {e}").into()),
        }
    } else if i <= lib_len + shading::PRESETS.len() {
        let p = i - 1 - lib_len;
        Some(compose::Overlay { image: Rc::new(shading::preset(s.garment, p)), strength, preset: Some(p), name: shading::PRESETS[p].into(), ink: false })
    } else {
        return; // the "Imported" entry: already applied
    };
    s.checkpoint(None);
    s.doc.overlay = overlay;
    drop(s);
    refresh(app, st);
    sync_shading(app, &st.borrow());
}

fn import_shading(app: &AppWindow, st: &Shared, path: &Path) {
    match image::open(path) {
        Ok(img) if (img.width(), img.height()) == (template::W, template::H) => {
            let img = img.to_rgba8();
            let mut name = path.file_stem().map_or("overlay".into(), |s| s.to_string_lossy().into_owned());
            // Keep it in the shading library so it stays in the menu (same pixels aren't duplicated).
            if let Some(dir) = library::shading_dir() {
                match library::save(&dir, &name, &img) {
                    Ok(saved) => {
                        name = saved.file_stem().map_or(name, |s| s.to_string_lossy().into_owned());
                        st.borrow_mut().shading_library = library::list(&dir);
                    }
                    Err(e) => app.set_status(format!("Using the shading, but couldn't save it to your library: {e}").into()),
                }
            }
            let mut s = st.borrow_mut();
            s.checkpoint(None);
            let strength = s.doc.overlay.as_ref().map_or(0.6, |o| o.strength);
            s.doc.overlay = Some(compose::Overlay { image: Rc::new(img), strength, preset: None, name: name.clone(), ink: false });
            drop(s);
            app.set_status(format!("Shading \"{name}\" applied. Adjust Strength to taste.").into());
            refresh(app, st);
            sync_shading(app, &st.borrow());
        }
        Ok(img) => app.set_status(format!("A shading overlay must be 585×559; that image is {}×{}.", img.width(), img.height()).into()),
        Err(e) => app.set_status(format!("Could not open {}: {e}", path.display()).into()),
    }
}

fn add_layer(app: &AppWindow, st: &Shared, name: String, img: RgbaImage) {
    let mut s = st.borrow_mut();
    s.checkpoint(None);
    let mut layer = Layer::new(name, img);
    let place = match s.garment {
        Garment::Shirt => "the chest",
        Garment::Pants => {
            // the shirt usually covers the torso, so pants art starts where it can be seen
            layer.move_to(Part::RightLeg, Face::Front, [0.5, 0.35]);
            "the right leg"
        }
    };
    s.doc.layers.push(layer);
    s.selected = Some(s.doc.layers.len() - 1);
    let garment = s.garment.name();
    let small = s.doc.layers.last().map(|l| l.source.dimensions()).filter(|(w, h)| (*w).max(*h) < 256);
    drop(s);
    match small {
        Some((w, h)) => app.set_status(format!("Added a small picture ({w}×{h}) to {place}. For crisper edges, tick Upscale ×4 (AI) in Properties.").into()),
        None => app.set_status(format!("Artwork added to {place} — drag it anywhere on the {garment} (3D or 2D).").into()),
    }
    layers_changed(app, st);
}

/// Ctrl+V: image files copied in Explorer, or image data (browser "Copy image", screenshots).
fn paste_art(app: &AppWindow, st: &Shared) {
    let mut cb = match arboard::Clipboard::new() {
        Ok(cb) => cb,
        Err(e) => return app.set_status(format!("Could not open the clipboard: {e}").into()),
    };
    let is_image = |p: &&std::path::PathBuf| {
        p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["png", "jpg", "jpeg", "webp"].contains(&e.to_ascii_lowercase().as_str()))
    };
    if let Ok(files) = cb.get().file_list() {
        let images: Vec<_> = files.iter().filter(is_image).collect();
        if !images.is_empty() {
            return images.into_iter().for_each(|p| open_art(app, st, p));
        }
    }
    match cb.get_image() {
        Ok(data) => {
            let (w, h) = (data.width as u32, data.height as u32);
            match RgbaImage::from_raw(w, h, data.bytes.into_owned()) {
                Some(img) => {
                    let n = st.borrow().doc.layers.iter().filter(|l| l.name.starts_with("Pasted image")).count() + 1;
                    add_layer(app, st, format!("Pasted image {n}"), img);
                }
                None => app.set_status("The clipboard image has an unexpected size.".into()),
            }
        }
        Err(_) => app.set_status("No image on the clipboard. Copy an image (or an image file) first.".into()),
    }
}

fn export_png(app: &AppWindow, st: &Shared) {
    let file_name = format!("{}.png", st.borrow().garment.name());
    let dialog = rfd::AsyncFileDialog::new().add_filter("PNG", &["png"]).set_file_name(file_name);
    let (w, st) = (app.as_weak(), st.clone());
    let task = async move {
        if let (Some(file), Some(app)) = (dialog.save_file().await, w.upgrade()) {
            write_export(&app, &st, file.path());
        }
    };
    if let Err(e) = slint::spawn_local(task) {
        app.set_status(format!("Could not open the file dialog: {e}").into());
    }
}

fn write_export(app: &AppWindow, st: &Shared, path: &Path) {
    let img = compose::compose(&st.borrow().doc);
    let template = if st.borrow().garment == Garment::Pants { "Pants → PantsTemplate" } else { "Shirt → ShirtTemplate" };
    let msg = match save_png(&img, path) {
        Ok(()) => {
            // Show the file so it can be dragged straight into Studio.
            let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
            format!(
                "Exported {}. In Studio: drag it into the viewport (or Asset Manager → Bulk Import), copy its ID, paste into {template}. Or use Upload to Roblox.",
                path.display()
            )
        }
        Err(e) => format!("Export failed: {e}"),
    };
    app.set_status(msg.into());
}

fn save_png(img: &RgbaImage, path: &Path) -> Result<(), Box<dyn Error>> {
    if img.dimensions() != (template::W, template::H) {
        return Err(format!("composed image is {:?}, expected 585×559", img.dimensions()).into());
    }
    img.save_with_format(path, image::ImageFormat::Png)?;
    Ok(())
}

/// Headless: white short-sleeve preset + a generated transparent badge on the front torso.
fn export_sample(out: &Path) -> Result<(), Box<dyn Error>> {
    let badge = RgbaImage::from_fn(256, 256, |x, y| {
        let (dx, dy) = (x as f32 - 127.5, y as f32 - 127.5);
        let d = (dx * dx + dy * dy).sqrt();
        match d {
            d if d < 90.0 => Rgba([220, 40, 60, 255]),
            d if d < 120.0 => Rgba([30, 30, 30, 160]),
            _ => Rgba([0, 0, 0, 0]),
        }
    });
    let doc = Document {
        base: Rc::new(Garment::Shirt.preset(0)),
        layers: vec![Layer { center: [0.5, 0.45], ..Layer::new("badge".into(), badge) }],
        overlay: None,
        underlay: None,
    };
    let img = compose::compose(&doc);
    save_png(&img, out)?;
    println!("wrote {} ({}x{})", out.display(), img.width(), img.height());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            doc: Document { base: Rc::new(Garment::Shirt.preset(0)), layers: vec![], overlay: None, underlay: None },
            selected: None,
            composed: RgbaImage::new(1, 1),
            cam: avatar::Camera::default(),
            rig: vec![],
            mesh: (vec![], vec![]),
            obj_text: None,
            renderer: None,
            drag: Drag::None,
            last: (0.0, 0.0),
            undo: vec![],
            redo: vec![],
            last_key: None,
            library: vec![],
            shading_library: vec![],
            tattoo_library: vec![],
            project_path: None,
            edits: 0,
            saved_edits: 0,
            autosaved_edits: 0,
            png_cache: project::PngCache::default(),
            custom_template: false,
            bg_job: None,
            net_job: None,
            refresh_queued: false,
            hover: None,
            garment: Garment::Shirt,
            other: Stash::new(Garment::Pants),
        }
    }

    #[test]
    fn resize_cursor_follows_rotation() {
        assert_eq!(resize_cursor((1, 0), 0.0), 3);
        assert_eq!(resize_cursor((0, 1), 0.0), 4);
        assert_eq!(resize_cursor((1, 1), 0.0), 1);
        assert_eq!(resize_cursor((1, -1), 0.0), 2);
        assert_eq!(resize_cursor((1, 0), 90.0), 4, "a rotated right side points down");
    }

    #[test]
    fn same_key_edits_merge_into_one_undo_step() {
        let mut s = state();
        s.checkpoint(None);
        s.doc.layers.push(Layer::new("a".into(), RgbaImage::new(4, 4)));
        for w in [0.2, 0.3, 0.4] {
            s.checkpoint(Some("width")); // one slider drag
            s.doc.layers[0].width = w;
        }
        s.checkpoint(None);
        s.doc.layers[0].flip_h = true;
        assert_eq!(s.undo.len(), 3, "add, width drag, flip");
        // undo the flip, then the whole width drag
        for _ in 0..2 {
            let snap = s.undo.pop().unwrap();
            let cur = s.snapshot();
            s.redo.push(cur);
            s.restore(snap);
        }
        assert!(!s.doc.layers[0].flip_h);
        assert_eq!(s.doc.layers[0].width, Layer::new("a".into(), RgbaImage::new(4, 4)).width);
        // undo snapshots share the source image instead of copying it
        assert!(Rc::ptr_eq(&s.doc.layers[0].source, &s.redo[0].layers[0].source));
        // a new edit clears redo
        s.checkpoint(None);
        assert!(s.redo.is_empty());
    }
}
