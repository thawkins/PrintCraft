//! The main window: page tabs on top, the 3D plate with the Printer box and model tools,
//! Filament / Process settings (or slice results) on the right, and a status bar.
//! The Prepare screen's parts live in prepare_ui.rs.

mod prepare_ui;
mod project_ui;
mod settings_ui;

use crate::device_ui::{DevicePage, PrintJob};
use crate::gcode::GcodeOutput;
use crate::geometry::Vec3;
use crate::gizmo;
use crate::model::{Mesh, PlacedObject, SampleShape, Triangle};
use crate::pipeline;
use crate::profiles::{limits, Filament, PrintSettings, Printer, Quality};
use crate::slicer::{PathKind, SliceResult};
use crate::theme;
use crate::view3d::{self, Camera, DrawObject};
use crate::widgets::{card, row, settings_grid};
use eframe::egui::{self, Color32, RichText, Sense, Ui};
use prepare_ui::ProcessTab;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

/// The gap kept between a Ditto job and its copy: the job must be at least this much
/// narrower than the nozzle spacing.
const DITTO_GAP_MM: f32 = 3.0;
/// How far above the view's centre the Move / Scale / Rotate strip starts.
const TOOL_STRIP_ABOVE_CENTRE: f32 = 80.0;
/// How far the prime tower stands from the plate's back and right edges.
const PRIME_TOWER_MARGIN_MM: f32 = 10.0;
/// The gap kept between the prime tower and any part.
const PRIME_TOWER_CLEARANCE_MM: f32 = 3.0;

/// How the program was started.
#[derive(Default)]
pub struct StartOptions {
    pub models: Vec<PathBuf>,
    pub screenshot: Option<PathBuf>,
    /// Slice the opened models straight away and show the layer preview.
    pub slice_on_start: bool,
    /// Open the Printer page and connect to this address.
    pub connect: Option<String>,
    /// Open on the Device page.
    pub device_page: bool,
    /// Start with this selection tool ("move", "scale" or "rotate").
    pub tool: Option<String>,
    /// Show the File menu open (for checking pictures).
    pub file_menu: bool,
    /// Open the enlarged camera once it shows a picture; maximised when true.
    pub camera: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Prepare,
    Preview,
    Printer,
}

/// A button's action on the selected object (the toolbar and Objects panel share them).
#[derive(Clone, Copy, Debug)]
enum ObjectAction {
    LayFlat,
    Turn90,
    Duplicate,
    Delete,
}

struct Sliced {
    result: SliceResult,
    gcode: GcodeOutput,
    /// What the plate and settings looked like when this was sliced.
    fingerprint: String,
    /// The print file, shared so sending it to a printer doesn't copy it.
    shared_text: Arc<String>,
    /// The 300×300 picture written into the print file.
    picture: Option<Arc<image::RgbaImage>>,
    /// A Ditto job too wide for its copy (skirt and brim included), or whose copy lands off
    /// the plate: why it can't be printed or exported.
    problem: Option<String>,
}

struct SliceJob {
    progress: Arc<AtomicU32>,
    rx: Receiver<Sliced>,
}

enum Drag {
    None,
    Orbit,
    Pan,
    Move {
        index: usize,
        grab: Vec3,
    },
    /// Dragging one of the selected object's handles (move arrow, scale corner, rotate ring).
    Gizmo(GizmoDrag),
}

#[derive(Clone, Copy)]
struct GizmoDrag {
    index: usize,
    handle: gizmo::Handle,
    start_pointer: egui::Pos2,
    start_plate: Vec3,
    start_x: f32,
    start_y: f32,
    start_scale: f32,
    start_rotation: Vec3,
    /// The box centre on screen when the drag began.
    center: egui::Pos2,
    start_angle: f32,
}

struct ScreenshotTask {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

/// Frames drawn before the `--screenshot` picture is taken.
const SCREENSHOT_FRAMES: u32 = 120;
/// Gap left between models by "Arrange".
const ARRANGE_GAP_MM: f32 = 6.0;

pub struct PrintCraftApp {
    printers: Vec<Printer>,
    printer: Printer,
    filaments: Vec<Filament>,
    filament: Filament,
    /// Two-nozzle choices (used when the printer has two).
    dual: crate::profiles::DualNozzle,
    quality: Option<Quality>,
    settings: PrintSettings,
    objects: Vec<PlacedObject>,
    plate: Vec<Vec<Triangle>>,
    selected: Option<usize>,
    camera: Camera,
    mode: Mode,
    drag: Drag,
    job: Option<SliceJob>,
    sliced: Option<Sliced>,
    layer: usize,
    hidden_kinds: Vec<PathKind>,
    /// Two-nozzle printers: colour the preview by nozzle instead of by path type.
    colour_by_nozzle: bool,
    status: String,
    screenshot: Option<ScreenshotTask>,
    device: DevicePage,
    process_tab: ProcessTab,
    /// Show every process setting, not just the common ones.
    advanced: bool,
    /// "Send print" was pressed before slicing: send once the slice is ready.
    send_after_slice: bool,
    /// The project file this plate was opened from or saved to.
    project_path: Option<PathBuf>,
    /// The plate and settings as last saved or opened (to notice unsaved changes).
    saved_fingerprint: String,
    /// New / Open waiting for an answer about unsaved changes.
    pending_project: Option<project_ui::ProjectAction>,
    egui_ctx: egui::Context,
    /// The selection tool: move arrows, scale corners or rotate rings.
    tool: gizmo::Tool,
    gizmo_hover: Option<gizmo::Handle>,
    /// The last few projects opened or saved, listed in the File menu.
    recent: crate::recent::RecentProjects,
    /// Open the File menu on the first frame (`--file-menu`, for checking pictures).
    open_file_menu: bool,
}

impl PrintCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: StartOptions) -> Self {
        theme::apply(&cc.egui_ctx);
        let printers = Printer::presets();
        let filaments = Filament::presets();
        // The person's own printer (shown selected in reference/picture-2.png) comes first.
        let printer = printers
            .iter()
            .find(|p| p.name.contains("Ender-3 V3 KE"))
            .unwrap_or(&printers[0])
            .clone();
        let capturing = start.screenshot.is_some();
        let mut app = Self {
            camera: Camera::for_plate(printer.bed_x, printer.bed_y),
            printer,
            printers,
            filament: filaments[0].clone(),
            dual: Default::default(),
            filaments,
            quality: Some(Quality::Standard),
            settings: Quality::Standard.settings(),
            objects: Vec::new(),
            plate: Vec::new(),
            selected: None,
            mode: Mode::Prepare,
            drag: Drag::None,
            job: None,
            sliced: None,
            layer: 0,
            hidden_kinds: Vec::new(),
            colour_by_nozzle: false,
            status: "Open a model (STL, OBJ or 3MF) or add a shape to start.".into(),
            screenshot: start.screenshot.map(|path| ScreenshotTask {
                path,
                frames: 0,
                requested: false,
            }),
            // Taking a check picture never changes the person's saved printers.
            device: DevicePage::with_saved_printers(capturing),
            process_tab: ProcessTab::Global,
            advanced: false,
            send_after_slice: false,
            project_path: None,
            saved_fingerprint: String::new(),
            pending_project: None,
            egui_ctx: cc.egui_ctx.clone(),
            tool: gizmo::Tool::Move,
            gizmo_hover: None,
            // Taking a check picture never changes the person's recent projects either.
            recent: {
                let path = crate::recent::RecentProjects::default_path();
                if capturing {
                    crate::recent::RecentProjects::load_read_only(path)
                } else {
                    crate::recent::RecentProjects::load(path)
                }
            },
            open_file_menu: start.file_menu,
        };
        app.saved_fingerprint = app.fingerprint();
        app.load_paths(&start.models);
        if start.slice_on_start && !app.objects.is_empty() {
            app.start_slice();
        }
        if let Some(t) = start.tool.as_deref() {
            if let Some(found) = gizmo::Tool::ALL
                .into_iter()
                .find(|x| x.label().eq_ignore_ascii_case(t))
            {
                app.tool = found;
            }
        }
        if start.device_page {
            app.mode = Mode::Printer;
        }
        if let Some(host) = start.connect {
            app.device.connect_to(host, &cc.egui_ctx);
            app.mode = Mode::Printer;
        }
        if let Some(maximised) = start.camera {
            app.device.show_camera_when_ready(maximised);
        }
        app
    }

    // ----- plate and objects -------------------------------------------------------------

    fn refresh_plate(&mut self) {
        self.plate = self.objects.iter().map(|o| o.world_triangles()).collect();
    }

    /// Object `i`'s box on the plate (from its placed triangles).
    fn object_bounds(&self, i: usize) -> crate::geometry::Bounds {
        crate::geometry::Bounds::of(self.plate[i].iter().flatten().copied())
    }

    fn refresh_object(&mut self, i: usize) {
        if let Some(o) = self.objects.get(i) {
            self.plate[i] = o.world_triangles();
        }
    }

    fn add_object(&mut self, mut obj: PlacedObject) {
        // A new part is printed by the printer's primary nozzle until changed.
        obj.extruder = self.printer.primary_nozzle.min(1);
        self.objects.push(obj);
        self.selected = Some(self.objects.len() - 1);
        self.arrange();
        self.mode = Mode::Prepare;
    }

    fn load_paths(&mut self, paths: &[PathBuf]) {
        let mut errors = Vec::new();
        let mut loaded = 0;
        // A project file opens as a project (it replaces the plate).
        if let Some(p) = paths.iter().find(|p| crate::project::is_project(p)) {
            self.request(project_ui::ProjectAction::Open(Some(p.clone())));
        }
        for path in paths.iter().filter(|p| !crate::project::is_project(p)) {
            match Mesh::load(path) {
                Ok(mesh) => {
                    let name = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.add_object(PlacedObject::new(name, mesh));
                    loaded += 1;
                }
                Err(e) => errors.push(format!("{}: {e}", display_name(path))),
            }
        }
        if !errors.is_empty() {
            self.status = format!("Could not open {}", errors.join("; "));
        } else if loaded > 0 {
            self.status = format!(
                "Added {loaded} model{}.",
                if loaded == 1 { "" } else { "s" }
            );
        }
    }

    fn open_dialog(&mut self) {
        if self.screenshot.is_some() {
            return;
        }
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "3D models (STL, OBJ, 3MF)",
                &["stl", "STL", "obj", "OBJ", "3mf", "3MF"],
            )
            .pick_files()
        {
            self.load_paths(&paths);
        }
    }

    /// Lays the models out in rows, centred on the plate.
    fn arrange(&mut self) {
        let sizes: Vec<(f32, f32)> = self
            .objects
            .iter()
            .map(|o| {
                let s = o.world_bounds().size();
                (s.x, s.y)
            })
            .collect();
        let mut order: Vec<usize> = (0..sizes.len()).collect();
        order.sort_by(|&a, &b| sizes[b].1.total_cmp(&sizes[a].1));
        let usable = self.printer.bed_x - 10.0;
        let mut placed = vec![(0.0f32, 0.0f32); sizes.len()];
        let (mut x, mut y, mut row_depth, mut widest) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for &i in &order {
            let (w, d) = sizes[i];
            if x > 0.0 && x + w > usable {
                y += row_depth + ARRANGE_GAP_MM;
                x = 0.0;
                row_depth = 0.0;
            }
            placed[i] = (x + w / 2.0, y + d / 2.0);
            x += w + ARRANGE_GAP_MM;
            widest = widest.max(x - ARRANGE_GAP_MM);
            row_depth = row_depth.max(d);
        }
        let total_depth = y + row_depth;
        for (o, (px, py)) in self.objects.iter_mut().zip(placed) {
            o.x = px - widest / 2.0;
            o.y = total_depth / 2.0 - py;
        }
        self.refresh_plate();
    }

    /// Does a toolbar or Objects-panel button's action to the selected object.
    fn apply_to_selected(&mut self, action: ObjectAction) {
        let Some(i) = self.selected.filter(|&i| i < self.objects.len()) else {
            return;
        };
        match action {
            ObjectAction::LayFlat => {
                self.objects[i].lay_flat();
                self.refresh_object(i);
            }
            ObjectAction::Turn90 => {
                let o = &mut self.objects[i];
                o.rotation.z = (o.rotation.z + 90.0) % 360.0;
                self.refresh_object(i);
            }
            ObjectAction::Duplicate => self.duplicate_selected(),
            ObjectAction::Delete => self.delete_selected(),
        }
    }

    fn delete_selected(&mut self) {
        if let Some(i) = self.selected.take() {
            // A drag in progress may point at this object (or one after it).
            self.drag = Drag::None;
            let name = self.objects.remove(i).name;
            self.plate.remove(i);
            self.status = format!("Removed {name}.");
        }
    }

    fn duplicate_selected(&mut self) {
        if let Some(i) = self.selected {
            let mut copy = self.objects[i].clone();
            copy.name = format!("{} copy", copy.name);
            self.add_object(copy);
        }
    }

    fn fits_on_plate(&self, i: usize) -> bool {
        pipeline::fits_on_plate(&self.object_bounds(i), &self.printer)
    }

    // ----- slicing --------------------------------------------------------------------------

    fn fingerprint(&self) -> String {
        let mut s = format!(
            "{:?}|{:?}|{:?}|{:?}",
            self.settings, self.printer, self.filament, self.dual
        );
        for o in &self.objects {
            s += &format!(
                "|{}:{}:{}:{:?}:{}:{}:{}",
                o.name,
                o.x,
                o.y,
                o.rotation,
                o.scale_percent,
                o.mesh.triangles.len(),
                o.extruder
            );
        }
        s
    }

    /// Where a Ditto copy prints, along X (mm): one nozzle spacing towards the other
    /// nozzle, so to the left when the right nozzle (T0) prints, to the right when T1 does.
    fn ditto_offset(&self) -> f32 {
        let spacing = self.printer.nozzle_spacing;
        if self.printer.primary_tool() == 0 {
            -spacing
        } else {
            spacing
        }
    }

    /// Which nozzle prints what, from each part's nozzle and the two-nozzle settings. Err
    /// says why the plate can't be printed that way (a Ditto plate too wide for the copy).
    fn tools(&self) -> Result<crate::gcode::Tools, String> {
        if self.printer.extruders < 2 {
            return Ok(crate::gcode::Tools::default());
        }
        let parts: Vec<([f32; 4], u8)> = self
            .objects
            .iter()
            .map(|o| {
                let b = o.world_bounds();
                ([b.min.x, b.min.y, b.max.x, b.max.y], o.extruder)
            })
            .collect();
        if self.dual.ditto {
            let bounds: Vec<[f32; 4]> = parts.iter().map(|(b, _)| *b).collect();
            ditto_check(&bounds, self.printer.bed_x, self.ditto_offset())?;
        }
        let tower_at = tower_centre(&self.printer, &self.dual);
        let both_used = {
            let mut used = [false; 2];
            for (_, t) in &parts {
                used[(*t).min(1) as usize] = true;
            }
            if self.settings.supports {
                used[self.dual.support_tool.min(1) as usize] = true;
            }
            used[0] && used[1]
        };
        let tower = (self.dual.prime_tower && !self.dual.ditto && both_used).then_some(tower_at);
        if let Some(c) = tower {
            let bounds: Vec<[f32; 4]> = parts.iter().map(|(b, _)| *b).collect();
            tower_check(c, &bounds, self.printer.bed_x, self.printer.bed_y)?;
        }
        let shield = (self.dual.ooze_shield && !self.dual.ditto && both_used)
            .then_some(self.dual.shield_distance);
        Ok(crate::gcode::Tools {
            shield,
            tower,
            parts,
            support_tool: self.dual.support_tool,
            primary: self.printer.primary_tool(),
            second: self.dual.second_filament.clone(),
            ditto: self.dual.ditto,
        })
    }

    fn slice_is_current(&self) -> bool {
        self.sliced
            .as_ref()
            .is_some_and(|s| s.fingerprint == self.fingerprint())
    }

    fn start_slice(&mut self) {
        if self.job.is_some() {
            return;
        }
        if self.objects.is_empty() {
            self.status = "Add a model to the plate first.".into();
            return;
        }
        if let Some(i) = (0..self.objects.len()).find(|&i| !self.fits_on_plate(i)) {
            self.status = format!(
                "\"{}\" doesn't fit on the plate. Move, rotate or shrink it first.",
                self.objects[i].name
            );
            self.selected = Some(i);
            return;
        }
        let tris: Vec<Triangle> = self.plate.concat();
        let tools = match self.tools() {
            Ok(t) => t,
            Err(why) => {
                self.status = why;
                return;
            }
        };
        let (settings, printer, filament) = (
            self.settings.clone(),
            self.printer.clone(),
            self.filament.clone(),
        );
        let ditto_dx =
            (self.printer.extruders >= 2 && self.dual.ditto).then(|| self.ditto_offset());
        // The ooze shield's outline exists only once sliced: it must be on the plate and
        // clear of the prime tower.
        let shield_check = tools.shield.map(|_| tools.tower);
        let fingerprint = self.fingerprint();
        let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
        let (tx, rx) = mpsc::channel();
        let p = progress.clone();
        std::thread::spawn(move || {
            let pipeline::Sliced {
                result,
                gcode,
                picture,
            } = pipeline::slice_plate_with_tools(
                &tris,
                &printer,
                &filament,
                &settings,
                &tools,
                &|v| p.store(v.to_bits(), Ordering::Relaxed),
            );
            let shared_text = Arc::new(gcode.text.clone());
            // Ditto: the job as sliced (skirt, brim and raft included) must leave room for
            // its copy.
            let problem = ditto_dx
                .and_then(|dx| {
                    let extent = job_extent(&result)?;
                    ditto_check(&[extent], printer.bed_x, dx).err()
                })
                .or_else(|| {
                    let tower = shield_check?;
                    shield_check_fn(&result, tower, printer.bed_x, printer.bed_y).err()
                });
            let _ = tx.send(Sliced {
                result,
                gcode,
                fingerprint,
                shared_text,
                picture: picture.map(Arc::new),
                problem,
            });
        });
        self.job = Some(SliceJob { progress, rx });
        self.status = "Slicing…".into();
    }

    /// Empties the plate, dropping anything that pointed into it (selection, drag, slice).
    fn clear_plate(&mut self) {
        self.objects.clear();
        self.plate.clear();
        self.selected = None;
        self.drag = Drag::None;
        self.cancel_slice();
    }

    /// Forgets a slice in progress (its plate is gone); the worker's result is dropped.
    fn cancel_slice(&mut self) {
        self.job = None;
        self.send_after_slice = false;
    }

    fn poll_slice(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Ok(done) => {
                let e = done.gcode.estimate;
                self.status = format!(
                    "Sliced into {} layers: about {}, {:.1} m of filament ({:.0} g).",
                    e.layers,
                    e.time_text(),
                    e.filament_mm / 1000.0,
                    e.grams
                );
                self.layer = done.result.layers.len().saturating_sub(1);
                let problem = done.problem.clone();
                self.sliced = Some(done);
                self.mode = Mode::Preview;
                self.job = None;
                if let Some(why) = problem {
                    // Shown, and the job is held back from the printer and from export.
                    self.status = why;
                    self.send_after_slice = false;
                } else if std::mem::take(&mut self.send_after_slice) {
                    self.send_print();
                }
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "Slicing stopped unexpectedly.".into();
                self.job = None;
                self.send_after_slice = false;
            }
        }
    }

    /// The current slice, ready to send to a printer.
    fn print_job(&self) -> Option<PrintJob> {
        if !self.slice_is_current() {
            return None;
        }
        let sliced = self.sliced.as_ref()?;
        let name = self
            .objects
            .first()
            .map(|o| o.name.as_str())
            .unwrap_or("print");
        Some(PrintJob {
            name: crate::printer::safe_file_name(name),
            gcode: sliced.shared_text.clone(),
            layers: sliced.result.layers.len(),
            picture: sliced.picture.clone(),
        })
    }

    fn export(&mut self) {
        let Some(sliced) = &self.sliced else { return };
        if let Some(why) = &sliced.problem {
            self.status = why.clone();
            return;
        }
        if self.screenshot.is_some() {
            return;
        }
        let name = self
            .objects
            .first()
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "print".into());
        // A MakerBot / Sailfish printer takes X3G: translate first, so a problem shows
        // before the file dialog does.
        let (bytes, kind, ext, notes) = if self.printer.x3g {
            match crate::x3g::export(
                &sliced.gcode.text,
                &self.printer,
                &name,
                sliced.result.layers.len(),
            ) {
                Ok((bytes, warnings)) => (bytes, "X3G", "x3g", warnings),
                Err(e) => {
                    self.status = format!("Could not make the X3G file: {e}");
                    return;
                }
            }
        } else {
            (
                sliced.gcode.text.clone().into_bytes(),
                "G-code",
                "gcode",
                Vec::new(),
            )
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(kind, &[ext])
            .set_file_name(format!("{name}.{ext}"))
            .save_file()
        else {
            return;
        };
        self.status = match std::fs::write(&path, &bytes) {
            Ok(()) if notes.is_empty() => format!("Saved {}.", display_name(&path)),
            Ok(()) => format!("Saved {}. Note: {}", display_name(&path), notes.join(" ")),
            Err(e) => format!("Could not save the file: {e}"),
        };
    }

    // ----- panels ---------------------------------------------------------------------------

    /// The Objects tab: the list of objects on the plate, then the selected object's
    /// size, placement and buttons.
    fn objects_panel(&mut self, ui: &mut Ui) {
        self.object_list_card(ui);
        let Some(i) = self.selected.filter(|&i| i < self.objects.len()) else {
            return;
        };
        self.selected_object_card(ui, i);
    }

    /// "Objects on the plate": one selectable row per object, in the warning colour when
    /// it doesn't fit on the plate.
    fn object_list_card(&mut self, ui: &mut Ui) {
        card(ui, "Objects on the plate", |ui| {
            if self.objects.is_empty() {
                ui.label(
                    RichText::new("Nothing here yet. Open a model or add a shape.")
                        .color(theme::TEXT_DIM),
                );
            }
            for i in 0..self.objects.len() {
                let fits = self.fits_on_plate(i);
                let mut text = RichText::new(&self.objects[i].name);
                if !fits {
                    text = text.color(theme::OBJECT_OUTSIDE);
                }
                let r = ui.selectable_label(self.selected == Some(i), text);
                let r = if fits {
                    r
                } else {
                    r.on_hover_text("Doesn't fit on the plate")
                };
                if r.clicked() {
                    self.selected = Some(i);
                }
            }
        });
    }

    /// "Selected object": its size, position / rotation / scale, and its buttons.
    fn selected_object_card(&mut self, ui: &mut Ui, i: usize) {
        let (ox, oy) = (self.printer.bed_x / 2.0, self.printer.bed_y / 2.0);
        let mut changed = false;
        let mut action = None;
        let size = self.object_bounds(i).size();
        let two_nozzles = self.printer.extruders >= 2 && !self.dual.ditto;
        let primary = self.printer.primary_tool();
        card(ui, "Selected object", |ui| {
            let o = &mut self.objects[i];
            ui.label(
                RichText::new(format!(
                    "Size {:.1} × {:.1} × {:.1} mm",
                    size.x, size.y, size.z
                ))
                .color(theme::TEXT_DIM),
            );
            if two_nozzles {
                ui.horizontal(|ui| {
                    ui.label("Nozzle");
                    egui::ComboBox::from_id_salt("object_nozzle")
                        .selected_text(nozzle_name(o.extruder, primary))
                        .show_ui(ui, |ui| {
                            for t in [0u8, 1] {
                                ui.selectable_value(&mut o.extruder, t, nozzle_name(t, primary));
                            }
                        });
                });
            }
            changed |= object_placement_grid(ui, o, ox, oy);
            let (edited, clicked) = object_buttons(ui, o);
            changed |= edited;
            action = clicked;
        });
        if changed {
            self.refresh_object(i);
        }
        if let Some(action) = action {
            self.apply_to_selected(action);
        }
    }

    fn results_panel(&mut self, ui: &mut Ui) {
        let current = self.slice_is_current();
        let Some(sliced) = &self.sliced else {
            card(ui, "Preview", |ui| {
                ui.label(
                    RichText::new(
                        "Press \"Slice plate\" to see the layers and how long the print will take.",
                    )
                    .color(theme::TEXT_DIM),
                );
            });
            return;
        };
        let e = sliced.gcode.estimate;
        card(ui, "Print estimate", |ui| {
            if !current {
                ui.label(
                    RichText::new("Something changed since slicing. Slice again to update.")
                        .color(theme::WARNING),
                );
            }
            settings_grid(ui, "estimate_grid", |ui| {
                for (k, v) in [
                    ("Print time", e.time_text()),
                    ("Filament", format!("{:.2} m", e.filament_mm / 1000.0)),
                    ("Weight", format!("{:.1} g", e.grams)),
                    ("Cost", format!("{:.2}", e.cost)),
                    ("Layers", e.layers.to_string()),
                ] {
                    ui.label(RichText::new(k).color(theme::TEXT_DIM));
                    ui.label(RichText::new(v).strong());
                    ui.end_row();
                }
                if let Some(n) = e.per_nozzle {
                    for (t, (mm, g)) in n.iter().enumerate() {
                        ui.label(
                            RichText::new(nozzle_name(t as u8, self.printer.primary_tool()))
                                .color(theme::TEXT_DIM),
                        );
                        ui.label(
                            RichText::new(format!("{:.2} m · {g:.1} g", mm / 1000.0)).strong(),
                        );
                        ui.end_row();
                    }
                }
            });
        });
        card(ui, "Show", |ui| {
            if self.printer.extruders >= 2 && !self.dual.ditto {
                ui.checkbox(&mut self.colour_by_nozzle, "Colour by nozzle")
                    .on_hover_text("Right nozzle (T0) blue, left nozzle (T1) orange");
            }
            for kind in PathKind::ALL {
                let mut on = !self.hidden_kinds.contains(&kind);
                ui.horizontal(|ui| {
                    let (r, painter) = ui.allocate_painter(egui::vec2(14.0, 14.0), Sense::hover());
                    painter.rect_filled(r.rect.shrink(1.0), 3.0, theme::path_color(kind));
                    if ui.checkbox(&mut on, kind.label()).changed() {
                        if on {
                            self.hidden_kinds.retain(|k| *k != kind);
                        } else {
                            self.hidden_kinds.push(kind);
                        }
                    }
                });
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            if let Some(job) = &self.job {
                let p = f32::from_bits(job.progress.load(Ordering::Relaxed));
                ui.add(
                    egui::ProgressBar::new(p)
                        .desired_width(160.0)
                        .show_percentage(),
                );
            }
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} · {} · plate {:.0} × {:.0} mm",
                        self.filament.name,
                        self.printer.name,
                        self.printer.bed_x,
                        self.printer.bed_y
                    ))
                    .color(theme::TEXT_DIM),
                );
                if let Some(link) = self.device.summary() {
                    ui.label(RichText::new(format!("{link}  ·")).color(theme::ACCENT));
                }
            });
        });
    }

    // ----- 3D view --------------------------------------------------------------------------

    fn viewport(&mut self, ui: &mut Ui) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        paint_background(&painter, rect);
        view3d::draw_plate(
            &painter,
            &self.camera,
            rect,
            self.printer.bed_x,
            self.printer.bed_y,
            self.printer.max_z,
        );

        let preview = self.mode == Mode::Preview && self.sliced.is_some();
        if preview {
            let sliced = self.sliced.as_ref().unwrap();
            let hidden = &self.hidden_kinds;
            let tools = self.tools().unwrap_or_default();
            let dual = self.printer.extruders >= 2;
            let by_nozzle = dual && self.colour_by_nozzle && !self.dual.ditto;
            let color = |p: &crate::slicer::ToolPath| {
                if by_nozzle {
                    theme::nozzle_color(tools.tool_for(p))
                } else {
                    theme::path_color(p.kind)
                }
            };
            let look = view3d::PathLook {
                visible: &|k| !hidden.contains(&k),
                color: &color,
                copy_dx: (dual && self.dual.ditto).then(|| self.ditto_offset()),
            };
            view3d::draw_toolpaths(
                &painter,
                &self.camera,
                rect,
                &sliced.result,
                self.layer,
                &look,
            );
        } else {
            let draw: Vec<DrawObject> = (0..self.objects.len())
                .map(|i| DrawObject {
                    triangles: &self.plate[i],
                    color: if !self.fits_on_plate(i) {
                        theme::OBJECT_OUTSIDE
                    } else if self.selected == Some(i) {
                        theme::OBJECT_SELECTED
                    } else {
                        theme::OBJECT
                    },
                })
                .collect();
            view3d::draw_objects(&painter, &self.camera, rect, &draw);
        }

        self.handle_view_input(ui, rect, &response, preview);
        // Selection marks and the current tool's handles, over the models.
        if let Some((_, b)) = self.selected_bounds() {
            let active = match self.drag {
                Drag::Gizmo(g) => Some(g.handle),
                _ => None,
            };
            gizmo::draw(
                &painter,
                &self.camera,
                rect,
                &b,
                self.tool,
                self.gizmo_hover,
                active,
            );
            let size = b.size();
            if let Some((top, _)) = self
                .camera
                .project(Vec3::new(b.center().x, b.center().y, b.max.z), rect)
            {
                let text = format!("{:.1} × {:.1} × {:.1} mm", size.x, size.y, size.z);
                painter.text(
                    top - egui::vec2(0.0, 28.0),
                    egui::Align2::CENTER_BOTTOM,
                    text,
                    egui::FontId::proportional(13.0),
                    theme::TEXT,
                );
            }
        }
        self.view_overlays(ui, rect, preview);
        if !preview && self.mode == Mode::Prepare {
            self.tool_bar(ui, rect);
        }
    }

    /// The selected object and its box on the plate (Prepare page only).
    fn selected_bounds(&self) -> Option<(usize, crate::geometry::Bounds)> {
        if self.mode != Mode::Prepare {
            return None;
        }
        let i = self.selected.filter(|&i| i < self.plate.len())?;
        let b = self.object_bounds(i);
        b.is_valid().then_some((i, b))
    }

    /// Moves, scales or turns the object being dragged by one of its handles.
    fn apply_gizmo_drag(&mut self, g: &GizmoDrag, at: egui::Pos2, rect: egui::Rect) {
        use gizmo::Handle;
        let Some(o) = self.objects.get_mut(g.index) else {
            return;
        };
        match g.handle {
            Handle::MoveX | Handle::MoveY => {
                if let Some(p) = self.camera.plate_point(at, rect) {
                    if g.handle == Handle::MoveX {
                        o.x = g.start_x + (p.x - g.start_plate.x);
                    } else {
                        o.y = g.start_y + (p.y - g.start_plate.y);
                    }
                }
            }
            Handle::ScaleCorner(_) => {
                let ratio = gizmo::scale_ratio(g.center, g.start_pointer, at);
                o.scale_percent = (g.start_scale * ratio).clamp(1.0, 2000.0).round();
            }
            Handle::RotateX | Handle::RotateY | Handle::RotateZ => {
                let turn = gizmo::ring_turn(
                    &self.camera,
                    g.handle,
                    g.start_angle,
                    gizmo::screen_angle(g.center, at),
                );
                let wrap = |a: f32| {
                    let a = (a + 180.0).rem_euclid(360.0) - 180.0;
                    (a * 10.0).round() / 10.0
                };
                // The rings are drawn about the world axes, so they turn about them.
                let r = match g.handle {
                    Handle::RotateX => {
                        crate::model::rotation_turned_about(g.start_rotation, 0, turn)
                    }
                    Handle::RotateY => {
                        crate::model::rotation_turned_about(g.start_rotation, 1, turn)
                    }
                    _ => {
                        let mut r = g.start_rotation;
                        r.z += turn;
                        r
                    }
                };
                o.rotation = Vec3::new(wrap(r.x), wrap(r.y), wrap(r.z));
            }
        }
        self.refresh_object(g.index);
    }

    /// The Move / Scale / Rotate buttons at the left of the 3D view.
    fn tool_bar(&mut self, ui: &mut Ui, rect: egui::Rect) {
        const BUTTON: f32 = 44.0;
        let area = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + 10.0,
                rect.center().y - TOOL_STRIP_ABOVE_CENTRE,
            ),
            egui::vec2(BUTTON + 12.0, 3.0 * BUTTON + 24.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            egui::Frame::new()
                .fill(theme::PANEL)
                .corner_radius(8)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        for t in gizmo::Tool::ALL {
                            let selected = self.tool == t;
                            let b = egui::Button::selectable(selected, "")
                                .min_size(egui::vec2(BUTTON, BUTTON));
                            let hint = match t {
                                gizmo::Tool::Move => {
                                    "Drag the red or green arrow to slide the selected model"
                                }
                                gizmo::Tool::Scale => {
                                    "Drag an orange corner to resize the selected model evenly"
                                }
                                gizmo::Tool::Rotate => {
                                    "Drag a red, green or blue ring to turn the selected model"
                                }
                            };
                            let response = ui.add(b);
                            let painter = ui.painter();
                            let r = response.rect;
                            let color = if selected || response.hovered() {
                                Color32::WHITE
                            } else {
                                theme::TEXT
                            };
                            crate::tool_icons::draw(
                                painter,
                                r.shrink(5.0).translate(egui::vec2(-1.0, -1.0)),
                                t,
                                color,
                            );
                            // The keyboard shortcut, small in the corner.
                            painter.text(
                                r.right_bottom() + egui::vec2(-4.0, -2.0),
                                egui::Align2::RIGHT_BOTTOM,
                                t.key(),
                                egui::FontId::proportional(10.0),
                                theme::TEXT_DIM,
                            );
                            let tip = format!("{} ({})\n{hint}", t.label(), t.key());
                            if response.on_hover_text(tip).clicked() {
                                self.tool = t;
                            }
                        }
                    });
                });
        });
    }

    /// Mouse input on the 3D view: gizmo hover, starting and continuing a drag (gizmo
    /// handle, moving a model, orbit or pan), click to select, and scroll to zoom.
    fn handle_view_input(
        &mut self,
        ui: &Ui,
        rect: egui::Rect,
        response: &egui::Response,
        preview: bool,
    ) {
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        // Which handle of the selected object is under the mouse.
        let selection = if preview {
            None
        } else {
            self.selected_bounds()
        };
        self.gizmo_hover = match (&selection, response.hover_pos()) {
            (Some((_, b)), Some(at)) if matches!(self.drag, Drag::None) => {
                gizmo::pick(&self.camera, rect, b, self.tool, at)
            }
            _ => None,
        };
        if self.gizmo_hover.is_some() || matches!(self.drag, Drag::Gizmo(_)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if response.drag_started() {
            self.begin_view_drag(ui, rect, pointer, selection, preview);
        }
        if response.dragged() {
            self.continue_view_drag(rect, response.drag_delta(), pointer);
        }
        if response.drag_stopped() {
            self.drag = Drag::None;
        }
        if response.clicked() && !preview {
            self.select_clicked_object(rect, pointer);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
            }
        }
    }

    /// The object drawn under `at` on the view, if any.
    fn object_under(&self, rect: egui::Rect, at: egui::Pos2) -> Option<usize> {
        let tris: Vec<&[Triangle]> = self.plate.iter().map(|t| t.as_slice()).collect();
        view3d::pick_object(&self.camera, rect, &tris, at)
    }

    /// A drag starts: a primary drag on a gizmo handle grabs it, on a model selects and
    /// moves it (Prepare only), on empty space orbits; any other button pans.
    fn begin_view_drag(
        &mut self,
        ui: &Ui,
        rect: egui::Rect,
        pointer: Option<egui::Pos2>,
        selection: Option<(usize, crate::geometry::Bounds)>,
        preview: bool,
    ) {
        let primary = ui.input(|i| i.pointer.primary_down());
        let grabbed = match (primary, pointer, &selection) {
            (true, Some(at), Some((index, b))) => self.grab_gizmo(rect, at, *index, b),
            _ => None,
        };
        self.drag = match (primary, pointer, grabbed) {
            (_, _, Some(g)) => Drag::Gizmo(g),
            (true, Some(at), None) if !preview => match self.object_under(rect, at) {
                Some(index) => {
                    self.selected = Some(index);
                    let o = &self.objects[index];
                    let hit = self.camera.plate_point(at, rect).unwrap_or_default();
                    Drag::Move {
                        index,
                        grab: hit - Vec3::new(o.x, o.y, 0.0),
                    }
                }
                None => Drag::Orbit,
            },
            (true, _, _) => Drag::Orbit,
            (false, _, _) => Drag::Pan,
        };
    }

    /// The gizmo handle of object `index` (with bounds `b`) under `at`, recorded with the
    /// object's starting placement so the drag can be applied relative to it.
    fn grab_gizmo(
        &self,
        rect: egui::Rect,
        at: egui::Pos2,
        index: usize,
        b: &crate::geometry::Bounds,
    ) -> Option<GizmoDrag> {
        gizmo::pick(&self.camera, rect, b, self.tool, at).map(|handle| {
            let o = &self.objects[index];
            let center = self
                .camera
                .project(b.center(), rect)
                .map(|p| p.0)
                .unwrap_or(at);
            GizmoDrag {
                index,
                handle,
                start_pointer: at,
                start_plate: self.camera.plate_point(at, rect).unwrap_or_default(),
                start_x: o.x,
                start_y: o.y,
                start_scale: o.scale_percent,
                start_rotation: o.rotation,
                center,
                start_angle: gizmo::screen_angle(center, at),
            }
        })
    }

    /// While dragging: turn or pan the view, slide the grabbed model, or apply the
    /// gizmo handle's move, scale or rotation.
    fn continue_view_drag(
        &mut self,
        rect: egui::Rect,
        delta: egui::Vec2,
        pointer: Option<egui::Pos2>,
    ) {
        match self.drag {
            Drag::Orbit => self.camera.orbit(delta),
            Drag::Pan => self.camera.pan(delta, rect),
            Drag::Move { index, grab } => {
                let at = pointer.and_then(|at| self.camera.plate_point(at, rect));
                if let (Some(p), Some(o)) = (at, self.objects.get_mut(index)) {
                    o.x = p.x - grab.x;
                    o.y = p.y - grab.y;
                    self.refresh_object(index);
                }
            }
            Drag::Gizmo(g) => {
                if let Some(at) = pointer {
                    self.apply_gizmo_drag(&g, at, rect);
                }
            }
            Drag::None => {}
        }
    }

    /// A click selects the model under the pointer (or clears the selection on empty
    /// space).
    fn select_clicked_object(&mut self, rect: egui::Rect, pointer: Option<egui::Pos2>) {
        self.selected = pointer.and_then(|at| self.object_under(rect, at));
        if self.selected.is_some() {
            // Show the picked model's position, rotation and size on the right.
            self.process_tab = ProcessTab::Objects;
        }
    }

    /// Everything drawn over the 3D view: the Printer box and model tools (Prepare), the
    /// view buttons, Slice plate / Send print, and the hints or the layer slider.
    fn view_overlays(&mut self, ui: &mut Ui, rect: egui::Rect, preview: bool) {
        if self.mode == Mode::Prepare {
            self.prepare_overlays(ui, rect);
        }
        let slider_room = if preview { 70.0 } else { 0.0 };
        self.view_buttons(ui, rect, slider_room);
        // Slice plate / Send print, bottom right.
        let buttons = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 240.0 - slider_room, rect.bottom() - 96.0),
            egui::vec2(222.0, 90.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(buttons), |ui| {
            self.slice_buttons(ui)
        });

        if self.objects.is_empty() && !preview {
            empty_plate_hint(ui, rect);
        }

        if !preview {
            prepare_view_hint(ui, rect);
            return;
        }
        self.layer_slider_overlay(ui, rect);
    }

    /// Printer box (top left) and model tools (along the top), as in the original.
    fn prepare_overlays(&mut self, ui: &mut Ui, rect: egui::Rect) {
        // The box ends above the Move / Scale / Rotate strip (see `tool_bar`), so a long
        // printer editor scrolls instead of running under it.
        let strip_top = rect.center().y - TOOL_STRIP_ABOVE_CENTRE;
        let boxr = egui::Rect::from_min_max(
            rect.min + egui::vec2(10.0, 10.0),
            egui::pos2(
                rect.left() + 360.0,
                (strip_top - 10.0).max(rect.top() + 200.0),
            ),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(boxr), |ui| {
            self.printer_box(ui)
        });
        let tools = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 372.0, rect.top() + 10.0),
            egui::pos2(rect.right() - 90.0, rect.top() + 60.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(tools), |ui| {
            self.model_toolbar(ui)
        });
    }

    /// View buttons, top right: like the original's TOP / FRONT cube and home button.
    /// `slider_room` keeps them clear of the Preview layer slider.
    fn view_buttons(&mut self, ui: &mut Ui, rect: egui::Rect, slider_room: f32) {
        let views = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 80.0 - slider_room, rect.top() + 12.0),
            egui::vec2(70.0, 120.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(views), |ui| {
            ui.vertical_centered_justified(|ui| {
                if ui
                    .button("Top")
                    .on_hover_text("Look straight down")
                    .clicked()
                {
                    self.camera.pitch = 89.0;
                    self.camera.yaw = 0.0;
                }
                if ui
                    .button("Front")
                    .on_hover_text("Look from the front")
                    .clicked()
                {
                    self.camera.pitch = 12.0;
                    self.camera.yaw = 0.0;
                }
                if ui.button("Home").on_hover_text("Reset the view").clicked() {
                    self.camera = Camera::for_plate(self.printer.bed_x, self.printer.bed_y);
                }
            });
        });
    }

    /// Preview: the vertical layer slider on the right, the current layer's number and
    /// height at the top, and the arrow-key hint. Nothing until there are sliced layers.
    fn layer_slider_overlay(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let Some(sliced) = &self.sliced else { return };
        let n = sliced.result.layers.len();
        if n == 0 {
            return;
        }
        let slider_rect = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 60.0, rect.top() + 50.0),
            egui::pos2(rect.right() - 16.0, rect.bottom() - 50.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(slider_rect), |ui| {
            ui.spacing_mut().slider_width = slider_rect.height() - 10.0;
            ui.add(
                egui::Slider::new(&mut self.layer, 0..=n - 1)
                    .vertical()
                    .show_value(false),
            );
        });
        let layer = &sliced.result.layers[self.layer.min(n - 1)];
        let label = format!("Layer {} of {}  ·  {:.2} mm", self.layer + 1, n, layer.z);
        ui.painter().text(
            egui::pos2(rect.center().x, rect.top() + 24.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(14.0),
            theme::TEXT,
        );
        ui.painter().text(
            rect.left_bottom() + egui::vec2(12.0, -12.0),
            egui::Align2::LEFT_BOTTOM,
            "Up / Down arrows step through layers",
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
    }

    // ----- input and the screenshot option ---------------------------------------------------

    fn handle_keys_and_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.load_paths(&dropped);
        }
        let typing = ctx.memory(|m| m.focused().is_some());
        if typing {
            return;
        }
        let (del, open, up, down) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Delete),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        let (save, save_as) = ctx.input(|i| {
            let s = i.modifiers.command && i.key_pressed(egui::Key::S);
            (s && !i.modifiers.shift, s && i.modifiers.shift)
        });
        if del && self.mode == Mode::Prepare {
            self.delete_selected();
        }
        // M / S / R pick the selection tool, as in other slicers (not with Ctrl, so Ctrl+S saves).
        let tool_key = ctx.input(|i| {
            if i.modifiers.command || i.modifiers.alt {
                return None;
            }
            [
                (egui::Key::M, gizmo::Tool::Move),
                (egui::Key::S, gizmo::Tool::Scale),
                (egui::Key::R, gizmo::Tool::Rotate),
            ]
            .into_iter()
            .find(|(k, _)| i.key_pressed(*k))
            .map(|(_, t)| t)
        });
        if let (Some(t), Mode::Prepare) = (tool_key, self.mode) {
            self.tool = t;
        }
        if open {
            self.open_dialog();
        }
        if save {
            self.save_project();
        }
        if save_as {
            self.save_project_as();
        }
        if let (Mode::Preview, Some(s)) = (self.mode, &self.sliced) {
            let last = s.result.layers.len().saturating_sub(1);
            if up {
                self.layer = (self.layer + 1).min(last);
            }
            if down {
                self.layer = self.layer.saturating_sub(1);
            }
        }
    }

    fn run_screenshot(&mut self, ctx: &egui::Context) {
        let Some(task) = &mut self.screenshot else {
            return;
        };
        ctx.request_repaint();
        task.frames += 1;
        if task.frames >= SCREENSHOT_FRAMES && !task.requested {
            task.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::RgbaImage::from_raw(w as u32, h as u32, bytes)
                .map(|img| img.save(&task.path))
            {
                Some(Ok(())) => println!("Saved screenshot to {}", task.path.display()),
                Some(Err(e)) => eprintln!("Could not save the screenshot: {e}"),
                None => eprintln!("Could not save the screenshot: bad image size"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for PrintCraftApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_slice(&ctx);
        self.handle_keys_and_drops(&ctx);
        // Printer dialogs ("Print again?", confirmations) show on every page.
        self.device.overlay(&ctx);
        self.discard_dialog(&ctx);

        let bar_frame = egui::Frame::new()
            .fill(theme::BG)
            .inner_margin(egui::Margin::symmetric(12, 6));
        egui::Panel::top("toolbar")
            .frame(bar_frame)
            .exact_size(46.0)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(bar_frame)
            .exact_size(30.0)
            .show(ui, |ui| self.status_bar(ui));
        let side = egui::Frame::new()
            .fill(theme::PANEL)
            .inner_margin(egui::Margin::same(10));
        if self.mode == Mode::Printer {
            let job = self.print_job();
            let page = egui::Frame::new()
                .fill(theme::BG)
                .inner_margin(egui::Margin::same(14));
            egui::CentralPanel::default()
                .frame(page)
                .show(ui, |ui| self.device.ui(ui, job));
            self.run_screenshot(&ctx);
            return;
        }
        egui::Panel::right("process")
            .frame(side)
            .resizable(true)
            .default_size(430.0)
            .size_range(340.0..=640.0)
            .show(ui, |ui| {
                if self.mode == Mode::Prepare {
                    self.right_panel(ui);
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| self.results_panel(ui));
                }
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.viewport(ui));

        self.run_screenshot(&ctx);
    }
}

// ----- small helpers ---------------------------------------------------------------------------

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn paint_background(painter: &egui::Painter, rect: egui::Rect) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), theme::VIEW_TOP);
    mesh.colored_vertex(rect.right_top(), theme::VIEW_TOP);
    mesh.colored_vertex(rect.right_bottom(), theme::VIEW_BOTTOM);
    mesh.colored_vertex(rect.left_bottom(), theme::VIEW_BOTTOM);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// The two-line "Open a model or add a shape" message in the middle of an empty plate.
fn empty_plate_hint(ui: &Ui, rect: egui::Rect) {
    ui.painter().text(
        rect.center() - egui::vec2(0.0, 40.0),
        egui::Align2::CENTER_CENTER,
        "Open a model or add a shape to begin",
        egui::FontId::proportional(20.0),
        theme::TEXT_DIM,
    );
    ui.painter().text(
        rect.center() - egui::vec2(0.0, 14.0),
        egui::Align2::CENTER_CENTER,
        "You can also drop STL, OBJ or 3MF files onto this window",
        egui::FontId::proportional(14.0),
        theme::TEXT_DIM,
    );
}

/// The mouse hint along the bottom left of the Prepare view.
fn prepare_view_hint(ui: &Ui, rect: egui::Rect) {
    let hint = "Click a model to select it · drag its handles to move (M), scale (S) or rotate (R) · drag empty space to turn the view · right-drag to pan · scroll to zoom";
    ui.painter().text(
        rect.left_bottom() + egui::vec2(12.0, -12.0),
        egui::Align2::LEFT_BOTTOM,
        hint,
        egui::FontId::proportional(12.0),
        theme::TEXT_DIM,
    );
}

/// Position (shown from the plate's corner; `ox`/`oy` is the plate's centre), rotation
/// and scale of one object. Returns whether anything changed.
fn object_placement_grid(ui: &mut Ui, o: &mut PlacedObject, ox: f32, oy: f32) -> bool {
    let mut changed = false;
    settings_grid(ui, "object_grid", |ui| {
        let (mut px, mut py) = (o.x + ox, o.y + oy);
        ui.label("Position X");
        changed |= ui
            .add(egui::DragValue::new(&mut px).speed(0.5).suffix(" mm"))
            .changed();
        ui.end_row();
        ui.label("Position Y");
        changed |= ui
            .add(egui::DragValue::new(&mut py).speed(0.5).suffix(" mm"))
            .changed();
        ui.end_row();
        (o.x, o.y) = (px - ox, py - oy);
        for (label, v) in [
            ("Rotate X", &mut o.rotation.x),
            ("Rotate Y", &mut o.rotation.y),
            ("Rotate Z", &mut o.rotation.z),
        ] {
            ui.label(label);
            changed |= ui
                .add(
                    egui::DragValue::new(v)
                        .speed(1.0)
                        .range(-360.0..=360.0)
                        .suffix("°"),
                )
                .changed();
            ui.end_row();
        }
        ui.label("Scale");
        changed |= ui
            .add(
                egui::Slider::new(&mut o.scale_percent, 10.0..=500.0)
                    .suffix(" %")
                    .logarithmic(true),
            )
            .changed();
        ui.end_row();
    });
    changed
}

/// Turn 90°, Center, Lay flat and Reset, then Duplicate and Delete. Center and Reset
/// edit the object here; the others are returned as an action. Returns whether the
/// object was edited, and the action clicked.
fn object_buttons(ui: &mut Ui, o: &mut PlacedObject) -> (bool, Option<ObjectAction>) {
    let mut changed = false;
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if ui.button("Turn 90°").clicked() {
            action = Some(ObjectAction::Turn90);
        }
        if ui.button("Center").clicked() {
            (o.x, o.y) = (0.0, 0.0);
            changed = true;
        }
        if ui
            .button("Lay flat")
            .on_hover_text("Turn it so its biggest flat face sits on the plate")
            .clicked()
        {
            action = Some(ObjectAction::LayFlat);
        }
        if ui
            .button("Reset")
            .on_hover_text("Undo rotation and scale")
            .clicked()
        {
            o.rotation = Vec3::default();
            o.scale_percent = 100.0;
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Duplicate").clicked() {
            action = Some(ObjectAction::Duplicate);
        }
        if ui.button("Delete").clicked() {
            action = Some(ObjectAction::Delete);
        }
    });
    (changed, action)
}

/// A nozzle of a two-nozzle printer, as the window names it (tool 0 is the right nozzle on
/// the Creator Pro / Replicator Dual family), marked when it is the printer's primary.
pub(crate) fn nozzle_name(tool: u8, primary: u8) -> String {
    let side = if tool == 0 { "Right (T0)" } else { "Left (T1)" };
    if tool == primary {
        format!("{side} · primary")
    } else {
        side.to_string()
    }
}

/// Whether a Ditto job works: its extent (min x, min y, max x, max y, plate coordinates;
/// the parts, or after slicing every line printed, skirt and brim included) must be at
/// least [`DITTO_GAP_MM`] narrower than the nozzle spacing `|dx|`, or the two copies
/// print into each other, and the copy, `dx` along X, must land on a plate `bed_x` wide.
/// Err says what to change.
fn ditto_check(parts: &[[f32; 4]], bed_x: f32, dx: f32) -> Result<(), String> {
    if parts.is_empty() {
        return Ok(());
    }
    let (lo, hi) = parts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), b| {
        (lo.min(b[0]), hi.max(b[2]))
    });
    let spacing = dx.abs();
    let widest = spacing - DITTO_GAP_MM;
    if hi - lo > widest {
        return Err(format!(
            "Ditto prints a copy {spacing:.0} mm to the side (the nozzle spacing), so the \
             job must fit within {widest:.0} mm across or the copies print into each other; \
             this one is {:.0} mm.",
            hi - lo
        ));
    }
    let half = bed_x / 2.0;
    if lo + dx < -half || hi + dx > half {
        return Err(format!(
            "The Ditto copy would print off the plate: move the parts {} by about {:.0} mm.",
            if dx < 0.0 { "right" } else { "left" },
            if dx < 0.0 {
                -half - (lo + dx)
            } else {
                hi + dx - half
            }
        ));
    }
    Ok(())
}

/// Where the prime tower's centre stands, in plate coordinates (0, 0 at the centre): the
/// position set under Two nozzles, or near the back-right corner.
fn tower_centre(
    printer: &crate::profiles::Printer,
    dual: &crate::profiles::DualNozzle,
) -> crate::geometry::Point2 {
    let (hx, hy) = (printer.bed_x / 2.0, printer.bed_y / 2.0);
    match dual.tower_at {
        Some([x, y]) => crate::geometry::Point2::new(x - hx, y - hy),
        None => {
            let inset = PRIME_TOWER_MARGIN_MM + crate::gcode::PRIME_TOWER_MM / 2.0;
            crate::geometry::Point2::new(hx - inset, hy - inset)
        }
    }
}

/// Whether the prime tower, centred at `c` (plate coordinates), stands wholly on a plate
/// `bed_x` x `bed_y` and at least [`PRIME_TOWER_CLEARANCE_MM`] from every part. Err says
/// what to change.
fn tower_check(
    c: crate::geometry::Point2,
    parts: &[[f32; 4]],
    bed_x: f32,
    bed_y: f32,
) -> Result<(), String> {
    let half = crate::gcode::PRIME_TOWER_MM / 2.0;
    if c.x - half < -bed_x / 2.0
        || c.x + half > bed_x / 2.0
        || c.y - half < -bed_y / 2.0
        || c.y + half > bed_y / 2.0
    {
        return Err(
            "The prime tower would stand partly off the plate: change its position under Two \
             nozzles."
                .into(),
        );
    }
    let h = half + PRIME_TOWER_CLEARANCE_MM;
    if parts
        .iter()
        .any(|b| b[0] < c.x + h && b[2] > c.x - h && b[1] < c.y + h && b[3] > c.y - h)
    {
        return Err(
            "The prime tower stands where a part is: move the part, set the tower's position, \
             or turn the prime tower off under Two nozzles."
                .into(),
        );
    }
    Ok(())
}

/// Whether a sliced job's ooze shield stands on the plate and clear of the prime tower
/// (centred at `tower`, when there is one). Err says what to change.
fn shield_check_fn(
    result: &SliceResult,
    tower: Option<crate::geometry::Point2>,
    bed_x: f32,
    bed_y: f32,
) -> Result<(), String> {
    let pts: Vec<_> = result
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .filter(|p| p.kind == PathKind::OozeShield)
        .flat_map(|p| p.points.iter().copied())
        .collect();
    if pts.is_empty() {
        return Ok(());
    }
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in &pts {
        lo = crate::geometry::Point2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = crate::geometry::Point2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    if lo.x < -bed_x / 2.0 || hi.x > bed_x / 2.0 || lo.y < -bed_y / 2.0 || hi.y > bed_y / 2.0 {
        return Err(
            "The ooze shield would run off the plate: move the parts in from the edge, lower \
             the shield distance, or turn the shield off under Two nozzles."
                .into(),
        );
    }
    if let Some(c) = tower {
        let h = crate::gcode::PRIME_TOWER_MM / 2.0 + PRIME_TOWER_CLEARANCE_MM;
        if lo.x < c.x + h && hi.x > c.x - h && lo.y < c.y + h && hi.y > c.y - h {
            return Err(
                "The prime tower stands inside or against the ooze shield: set the tower's \
                 position further from the parts under Two nozzles."
                    .into(),
            );
        }
    }
    Ok(())
}

/// The X/Y extent of everything a sliced job prints (min x, min y, max x, max y).
fn job_extent(result: &SliceResult) -> Option<[f32; 4]> {
    let mut b: Option<[f32; 4]> = None;
    for p in result
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .flat_map(|p| &p.points)
    {
        let e = b.get_or_insert([p.x, p.y, p.x, p.y]);
        *e = [e[0].min(p.x), e[1].min(p.y), e[2].max(p.x), e[3].max(p.y)];
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ditto_needs_a_narrow_plate_whose_copy_lands_on_the_plate() {
        // A 20 mm part at the centre; the copy 33 mm to the left: fine.
        assert_eq!(
            ditto_check(&[[-10.0, -5.0, 10.0, 5.0]], 227.0, -33.0),
            Ok(())
        );
        // Too wide for the spacing.
        let e = ditto_check(&[[-20.0, -5.0, 20.0, 5.0]], 227.0, -33.0).unwrap_err();
        assert!(
            e.contains("within 30 mm") && e.contains("into each other"),
            "{e}"
        );
        // At the left edge the copy, further left, would fall off.
        let e = ditto_check(&[[-110.0, -5.0, -95.0, 5.0]], 227.0, -33.0).unwrap_err();
        assert!(e.contains("move the parts right"), "{e}");
        // With T1 printing the copy goes right instead, so the left edge is fine.
        assert_eq!(
            ditto_check(&[[-110.0, -5.0, -95.0, 5.0]], 227.0, 33.0),
            Ok(())
        );
        assert_eq!(ditto_check(&[], 227.0, -33.0), Ok(()));
    }

    #[test]
    fn a_brim_can_make_a_ditto_job_too_wide_for_its_copy() {
        use crate::geometry::Vec3;
        use crate::model::{Mesh, PlacedObject};
        use crate::profiles::{Adhesion, Filament, PrintSettings, Printer};
        let printer = Printer::presets()
            .into_iter()
            .find(|p| p.extruders == 2)
            .unwrap();
        let part = PlacedObject::new("part", Mesh::cube(Vec3::new(26.0, 10.0, 1.0)));
        let b = part.world_bounds();
        let dx = -printer.nozzle_spacing;
        // The part alone fits within 30 mm (33 mm spacing less the gap).
        assert_eq!(
            ditto_check(&[[b.min.x, b.min.y, b.max.x, b.max.y]], printer.bed_x, dx),
            Ok(())
        );
        // Sliced with a brim, the job is wider than that, and is refused.
        let settings = PrintSettings {
            adhesion: Adhesion::Brim,
            ..PrintSettings::default()
        };
        let sliced = crate::pipeline::slice_plate(
            &part.world_triangles(),
            &printer,
            &Filament::presets()[0],
            &settings,
            &|_| {},
        );
        let extent = job_extent(&sliced.result).unwrap();
        assert!(extent[2] - extent[0] > 30.0, "{extent:?}");
        let e = ditto_check(&[extent], printer.bed_x, dx).unwrap_err();
        assert!(e.contains("into each other"), "{e}");
        // A printer with its nozzles further apart takes the same job.
        assert_eq!(ditto_check(&[extent], printer.bed_x, -60.0), Ok(()));
    }

    #[test]
    fn the_prime_tower_goes_where_it_is_set_and_stays_clear_and_on_the_plate() {
        use crate::geometry::Point2;
        use crate::profiles::{DualNozzle, Printer};
        let printer = Printer::presets()
            .into_iter()
            .find(|p| p.extruders == 2)
            .unwrap();
        // Automatic: 10 mm in from the back-right corner.
        let auto = tower_centre(&printer, &DualNozzle::default());
        assert_eq!(auto, Point2::new(227.0 / 2.0 - 16.0, 148.0 / 2.0 - 16.0));
        // Set: mm from the front-left corner.
        let set = DualNozzle {
            tower_at: Some([20.0, 30.0]),
            ..DualNozzle::default()
        };
        assert_eq!(
            tower_centre(&printer, &set),
            Point2::new(20.0 - 113.5, 30.0 - 74.0)
        );
        let part = [[-10.0, -10.0, 10.0, 10.0]];
        assert_eq!(tower_check(auto, &part, 227.0, 148.0), Ok(()));
        let e = tower_check(Point2::new(12.0, 0.0), &part, 227.0, 148.0).unwrap_err();
        assert!(e.contains("where a part is"), "{e}");
        let e = tower_check(Point2::new(110.0, 0.0), &part, 227.0, 148.0).unwrap_err();
        assert!(e.contains("off the plate"), "{e}");
    }

    #[test]
    fn the_ooze_shield_must_stay_on_the_plate_and_clear_of_the_tower() {
        use crate::geometry::Point2;
        use crate::slicer::{Layer, PathKind, ToolPath};
        let ring = |h: f32| SliceResult {
            layers: vec![Layer {
                z: 0.2,
                height: 0.2,
                paths: vec![ToolPath {
                    kind: PathKind::OozeShield,
                    points: vec![
                        Point2::new(-h, -h),
                        Point2::new(h, -h),
                        Point2::new(h, h),
                        Point2::new(-h, h),
                    ],
                    closed: true,
                    width: None,
                }],
            }],
            line_width: 0.44,
        };
        assert_eq!(shield_check_fn(&ring(20.0), None, 227.0, 148.0), Ok(()));
        let e = shield_check_fn(&ring(80.0), None, 227.0, 148.0).unwrap_err();
        assert!(e.contains("off the plate"), "{e}");
        let e =
            shield_check_fn(&ring(20.0), Some(Point2::new(15.0, 0.0)), 227.0, 148.0).unwrap_err();
        assert!(e.contains("inside or against the ooze shield"), "{e}");
        assert_eq!(
            shield_check_fn(&ring(20.0), Some(Point2::new(80.0, 50.0)), 227.0, 148.0),
            Ok(())
        );
    }
}
