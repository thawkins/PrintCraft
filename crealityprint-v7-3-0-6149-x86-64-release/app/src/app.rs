//! The main window: page tabs on top, the 3D plate with the Printer box and model tools,
//! Filament / Process settings (or slice results) on the right, and a status bar.
//! The Prepare screen's parts live in prepare_ui.rs.

mod dialogs;
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
    Cut,
    Split,
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
    start_scale: Vec3,
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
    /// What the preview colours paths by.
    preview_colour: PreviewColour,
    /// The selected part's scale: X, Y and Z together (true) or each on its own.
    scale_lock: bool,
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
    /// Colour changes and pauses, by height, added to the print file (set in the preview).
    layer_events: Vec<crate::layer_events::LayerEvent>,
    /// Undo and redo of plate and settings changes.
    history: crate::history::History<PlateState>,
    /// The plate and settings as of the last recorded step, and their fingerprint: a
    /// change since then is recorded as one step once the edit is finished.
    settled: PlateState,
    settled_fp: String,
    /// New / Open waiting for an answer about unsaved changes.
    pending_project: Option<project_ui::ProjectAction>,
    egui_ctx: egui::Context,
    /// The file dialog that is open, if any: its answer arrives here (`dialogs`).
    file_dialog: Option<Receiver<Option<dialogs::Picked>>>,
    /// The selection tool: move arrows, scale corners or rotate rings.
    tool: gizmo::Tool,
    gizmo_hover: Option<gizmo::Handle>,
    /// With the Face tool: the part and the triangles of the flat face under the mouse.
    face_hover: Option<(usize, Vec<usize>)>,
    /// The cut controls: height over the plate (mm), the parts to keep, and whether the
    /// mouse is over them (the 3D view then shows the cut plane).
    cut_z: f32,
    cut_keep: crate::cut::CutKeep,
    cut_preview: bool,
    /// The last few projects opened or saved, listed in the File menu.
    recent: crate::recent::RecentProjects,
    /// Open the File menu on the first frame (`--file-menu`, for checking pictures).
    open_file_menu: bool,
}

impl PrintCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: StartOptions) -> Self {
        theme::apply(&cc.egui_ctx);
        let capturing = start.screenshot.is_some();
        // Taking a check picture never changes the person's saved printers or recent
        // projects.
        let device = DevicePage::with_saved_printers(capturing);
        let recent = {
            let path = crate::recent::RecentProjects::default_path();
            if capturing {
                crate::recent::RecentProjects::load_read_only(path)
            } else {
                crate::recent::RecentProjects::load(path)
            }
        };
        let mut app = Self::build(&cc.egui_ctx, &start, device, recent);
        app.load_paths(&start.models);
        // The models opened at start are where the undo history begins.
        app.reset_history();
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

    /// An app on an empty plate with no saved printers or recent projects, for tests:
    /// it reads and writes nothing in the person's settings folder.
    #[cfg(test)]
    fn for_tests() -> Self {
        Self::build(
            &egui::Context::default(),
            &StartOptions::default(),
            DevicePage::default(),
            crate::recent::RecentProjects::default(),
        )
    }

    /// The app's starting state, with the printer page and recent projects given.
    fn build(
        ctx: &egui::Context,
        start: &StartOptions,
        device: DevicePage,
        recent: crate::recent::RecentProjects,
    ) -> Self {
        let printers = Printer::presets();
        let filaments = Filament::presets();
        // The person's own printer (shown selected in reference/picture-2.png) comes first.
        let printer = printers
            .iter()
            .find(|p| p.name.contains("Ender-3 V3 KE"))
            .unwrap_or(&printers[0])
            .clone();
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
            preview_colour: PreviewColour::Type,
            scale_lock: true,
            status: "Open a model (STL, OBJ or 3MF) or add a shape to start.".into(),
            screenshot: start.screenshot.clone().map(|path| ScreenshotTask {
                path,
                frames: 0,
                requested: false,
            }),
            device,
            process_tab: ProcessTab::Global,
            advanced: false,
            send_after_slice: false,
            project_path: None,
            saved_fingerprint: String::new(),
            layer_events: Vec::new(),
            history: crate::history::History::new(UNDO_STEPS),
            settled: PlateState::default(),
            settled_fp: String::new(),
            pending_project: None,
            egui_ctx: ctx.clone(),
            file_dialog: None,
            tool: gizmo::Tool::Move,
            gizmo_hover: None,
            face_hover: None,
            cut_z: 0.0,
            cut_keep: crate::cut::CutKeep::Both,
            cut_preview: false,
            recent,
            open_file_menu: start.file_menu,
        };
        app.saved_fingerprint = app.project_fingerprint();
        app.reset_history();
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
        let mut repaired = Vec::new();
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
                    let (mesh, fixed) = crate::repair::repair(mesh);
                    if !fixed.is_empty() {
                        repaired.push(format!("{name}: {fixed}"));
                    }
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
            if !repaired.is_empty() {
                self.status += &format!(" Repaired {}.", repaired.join("; "));
            }
        }
    }

    fn open_dialog(&mut self) {
        if self.screenshot.is_some() {
            return;
        }
        self.ask_file(|| {
            rfd::FileDialog::new()
                .add_filter(
                    "3D models (STL, OBJ, 3MF)",
                    &["stl", "STL", "obj", "OBJ", "3mf", "3MF"],
                )
                .pick_files()
                .map(dialogs::Picked::Models)
        });
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
            ObjectAction::Cut => self.cut_selected(),
            ObjectAction::Split => self.split_selected(),
        }
    }

    /// Replaces the selected object with one object per separate piece of it, each left
    /// where it stood (the first selected).
    fn split_selected(&mut self) {
        let Some(i) = self.selected.filter(|&i| i < self.objects.len()) else {
            return;
        };
        let parts = self.objects[i].split();
        let name = self.objects[i].name.clone();
        if parts.len() < 2 {
            self.status = format!("{name} is one piece: there is nothing to split.");
            return;
        }
        let count = parts.len();
        self.drag = Drag::None;
        self.objects.splice(i..=i, parts);
        self.selected = Some(i);
        self.refresh_plate();
        self.status = format!("Split {name} into {count} objects.");
    }

    /// Cuts the selected object at `cut_z`, replacing it with the parts `cut_keep` asks
    /// for (the first selected), and arranges the plate.
    fn cut_selected(&mut self) {
        let Some(i) = self.selected.filter(|&i| i < self.objects.len()) else {
            return;
        };
        let height = self.object_bounds(i).size().z;
        let (z, keep) = (self.cut_z, self.cut_keep);
        if !(z > 0.0 && z < height) {
            self.status = format!("Choose a cut height between 0 and {height:.1} mm.");
            return;
        }
        let o = &self.objects[i];
        let parts = o.cut(z, keep);
        let name = o.name.clone();
        if parts.is_empty() {
            self.status = format!("Nothing of {name} is on that side of {z:.1} mm.");
            return;
        }
        let count = parts.len();
        self.drag = Drag::None;
        self.objects.splice(i..=i, parts);
        self.selected = Some(i);
        self.arrange();
        self.status = if count == 2 {
            format!("Cut {name} at {z:.1} mm into two parts.")
        } else {
            format!("Cut {name} at {z:.1} mm and kept one part.")
        };
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
                "|{}:{}:{}:{:?}:{:?}:{}:{}",
                o.name,
                o.x,
                o.y,
                o.rotation,
                o.scale,
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

    /// The plate and settings as they are now, for the undo history.
    fn capture(&self) -> PlateState {
        PlateState {
            objects: self.objects.clone(),
            settings: self.settings.clone(),
            quality: self.quality,
            printer: Some(self.printer.clone()),
            filament: Some(self.filament.clone()),
            dual: self.dual.clone(),
            layer_events: self.layer_events.clone(),
        }
    }

    /// Puts back a state from the undo history. The selection and any drag are dropped
    /// (they may point at a part that is gone); the slice stays, marked out of date if the
    /// plate differs from it.
    fn restore(&mut self, s: PlateState) {
        self.objects = s.objects;
        self.settings = s.settings;
        self.quality = s.quality;
        if let Some(p) = s.printer {
            self.printer = p;
        }
        if let Some(f) = s.filament {
            self.filament = f;
        }
        self.dual = s.dual;
        self.layer_events = s.layer_events;
        self.selected = None;
        self.drag = Drag::None;
        self.refresh_plate();
        self.refresh_print_file();
    }

    /// Records a finished change as one undo step. `idle` is false while an edit is
    /// still going on (a mouse button held, a field being typed in): the step is
    /// recorded once it ends, so a drag or a typed value is one step.
    fn note_changes(&mut self, idle: bool) {
        if !idle {
            return;
        }
        let fp = self.project_fingerprint();
        if fp != self.settled_fp {
            let now = self.capture();
            let before = std::mem::replace(&mut self.settled, now);
            self.history.record(before);
            self.settled_fp = fp;
        }
    }

    /// Starts a new undo history from the plate as it is (a new or opened project).
    fn reset_history(&mut self) {
        self.history.clear();
        self.settled = self.capture();
        self.settled_fp = self.project_fingerprint();
    }

    fn can_undo(&self) -> bool {
        self.history.can_undo() || self.project_fingerprint() != self.settled_fp
    }

    fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Goes back one step.
    fn undo(&mut self) {
        // A change not yet recorded (still being typed) becomes its own step first.
        self.note_changes(true);
        if let Some(back) = self.history.undo(self.capture()) {
            self.restore(back);
            self.settled = self.capture();
            self.settled_fp = self.project_fingerprint();
            self.status = "Undone.".into();
        }
    }

    /// Goes forward again one undone step.
    fn redo(&mut self) {
        if let Some(forward) = self.history.redo(self.capture()) {
            self.restore(forward);
            self.settled = self.capture();
            self.settled_fp = self.project_fingerprint();
            self.status = "Redone.".into();
        }
    }

    /// What a saved project holds: the plate and settings (`fingerprint`) plus the
    /// colour changes and pauses, which change the print file but need no new slice.
    pub(super) fn project_fingerprint(&self) -> String {
        format!("{}|{:?}", self.fingerprint(), self.layer_events)
    }

    /// Rebuilds the print file that is sent or exported: the sliced G-code with the
    /// colour changes and pauses added after their layers' markers.
    fn refresh_print_file(&mut self) {
        let Some(sliced) = &mut self.sliced else {
            return;
        };
        let zs: Vec<f32> = sliced.result.layers.iter().map(|l| l.z).collect();
        sliced.shared_text = Arc::new(crate::layer_events::add_layer_events(
            &sliced.gcode.text,
            &zs,
            &self.layer_events,
            &self.printer,
        ));
    }

    /// The colour mode in use: nozzle colouring only on a two-nozzle printer, not in
    /// Ditto (falls back to line type).
    fn effective_colour(&self) -> PreviewColour {
        let nozzle_ok = self.printer.extruders >= 2 && !self.dual.ditto;
        match self.preview_colour {
            PreviewColour::Nozzle if !nozzle_ok => PreviewColour::Type,
            other => other,
        }
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
            // A custom G-code mistake (an unknown placeholder, an unclosed {if}) comes first:
            // the file would not be what the person wrote. Then Ditto: the job as sliced
            // (skirt, brim and raft included) must leave room for its copy.
            let problem = crate::gcode::check_custom_gcode(&printer)
                .err()
                .or_else(|| {
                    let dx = ditto_dx?;
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
                self.refresh_print_file();
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
                &sliced.shared_text,
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
                sliced.shared_text.as_bytes().to_vec(),
                "G-code",
                "gcode",
                Vec::new(),
            )
        };
        self.ask_file(move || {
            let path = rfd::FileDialog::new()
                .add_filter(kind, &[ext])
                .set_file_name(format!("{name}.{ext}"))
                .save_file()?;
            Some(dialogs::Picked::Export { path, bytes, notes })
        });
    }

    /// Writes the print file chosen in the export dialog.
    fn write_export(&mut self, path: &Path, bytes: &[u8], notes: &[String]) {
        self.status = match std::fs::write(path, bytes) {
            Ok(()) if notes.is_empty() => format!("Saved {}.", display_name(path)),
            Ok(()) => format!("Saved {}. Note: {}", display_name(path), notes.join(" ")),
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
            changed |= object_placement_grid(ui, o, ox, oy, &mut self.scale_lock);
            let (edited, clicked) = object_buttons(ui, o);
            changed |= edited;
            action = clicked;
            let (cut, over) = cut_row(ui, &mut self.cut_z, &mut self.cut_keep, size.z);
            if cut {
                action = Some(ObjectAction::Cut);
            }
            self.cut_preview = over;
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
        let mode = self.effective_colour();
        let range = colour_range(mode, sliced);
        let times = sliced.gcode.times.clone();
        card(ui, "Show", |ui| {
            let nozzle_mode = self.printer.extruders >= 2 && !self.dual.ditto;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Colour by").color(theme::TEXT_DIM));
                egui::ComboBox::from_id_salt("preview_colour")
                    .selected_text(mode.label())
                    .show_ui(ui, |ui| {
                        for c in PreviewColour::ALL {
                            if c == PreviewColour::Nozzle && !nozzle_mode {
                                continue;
                            }
                            ui.selectable_value(&mut self.preview_colour, c, c.label());
                        }
                    });
            });
            match (mode, range) {
                (PreviewColour::Nozzle, _) => {
                    ui.label(
                        RichText::new("Right nozzle (T0) blue, left nozzle (T1) orange")
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                }
                (PreviewColour::Type, _) => {}
                (_, Some((lo, hi))) => gradient_legend(ui, mode, lo, hi),
                (_, None) => {}
            }
            ui.add_space(4.0);
            for kind in PathKind::ALL {
                let mut on = !self.hidden_kinds.contains(&kind);
                let time = times
                    .by_kind
                    .iter()
                    .find(|(k, _)| *k == kind)
                    .map(|(_, s)| *s);
                ui.horizontal(|ui| {
                    let swatch = if mode == PreviewColour::Type {
                        theme::path_color(kind)
                    } else {
                        theme::TEXT_DIM.gamma_multiply(0.4)
                    };
                    let (r, painter) = ui.allocate_painter(egui::vec2(14.0, 14.0), Sense::hover());
                    painter.rect_filled(r.rect.shrink(1.0), 3.0, swatch);
                    if ui.checkbox(&mut on, kind.label()).changed() {
                        if on {
                            self.hidden_kinds.retain(|k| *k != kind);
                        } else {
                            self.hidden_kinds.push(kind);
                        }
                    }
                    if let Some(s) = time {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(duration_text(s)).color(theme::TEXT_DIM));
                        });
                    }
                });
            }
            for (name, s) in [
                ("Travel", times.travel),
                ("Heating, purge, changes", times.other),
            ] {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.label(RichText::new(name).color(theme::TEXT_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(duration_text(s)).color(theme::TEXT_DIM));
                    });
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
            let mode = self.effective_colour();
            let stats = &sliced.gcode.path_stats;
            let range = colour_range(mode, sliced);
            let color = |li: usize, pi: usize, p: &crate::slicer::ToolPath| match mode {
                PreviewColour::Type => theme::path_color(p.kind),
                PreviewColour::Nozzle => theme::nozzle_color(tools.tool_for(p)),
                PreviewColour::Speed
                | PreviewColour::Width
                | PreviewColour::LayerHeight
                | PreviewColour::Fan
                | PreviewColour::Temperature => {
                    let height = sliced
                        .result
                        .layers
                        .get(li)
                        .map(|l| l.height)
                        .unwrap_or(0.0);
                    let value = stats
                        .get(li)
                        .and_then(|l| l.get(pi))
                        .and_then(|st| mode.value(st, height));
                    match (value, range) {
                        (Some(v), Some((lo, hi))) if hi > lo => {
                            theme::gradient((v - lo) / (hi - lo))
                        }
                        (Some(_), Some(_)) => theme::gradient(0.5),
                        (Some(_), None) | (None, Some(_)) | (None, None) => theme::TEXT_DIM,
                    }
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
            // The face the Face tool would lay down, drawn over its part.
            if let Some((oi, tris)) = &self.face_hover {
                if let Some(o) = self.objects.get(*oi) {
                    let world = o.world_triangles();
                    let face: Vec<Triangle> =
                        tris.iter().filter_map(|&i| world.get(i).copied()).collect();
                    view3d::draw_objects(
                        &painter,
                        &self.camera,
                        rect,
                        &[DrawObject {
                            triangles: &face,
                            // Orange, so it stands out on a selected (teal) part.
                            color: theme::WARNING,
                        }],
                    );
                }
            }
        }

        self.handle_view_input(ui, rect, &response, preview);
        // Where the cut controls would cut the selected part.
        if std::mem::take(&mut self.cut_preview) {
            if let Some((_, b)) = self.selected_bounds() {
                let (lo, hi) = (b.min, b.max);
                let z = lo.z + self.cut_z;
                let pad = 3.0;
                let corners: Vec<egui::Pos2> = [
                    (lo.x - pad, lo.y - pad),
                    (hi.x + pad, lo.y - pad),
                    (hi.x + pad, hi.y + pad),
                    (lo.x - pad, hi.y + pad),
                ]
                .into_iter()
                .filter_map(|(x, y)| self.camera.project(Vec3::new(x, y, z), rect))
                .map(|(p, _)| p)
                .collect();
                if corners.len() == 4 {
                    painter.add(egui::Shape::convex_polygon(
                        corners,
                        theme::WARNING.gamma_multiply(0.25),
                        egui::Stroke::new(1.5, theme::WARNING),
                    ));
                }
            }
        }
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
                // The corner handle scales evenly: every axis by the same ratio, so the
                // part keeps its proportions.
                let axis = |s: f32| (s * ratio).clamp(1.0, 2000.0).round();
                o.scale = Vec3::new(
                    axis(g.start_scale.x),
                    axis(g.start_scale.y),
                    axis(g.start_scale.z),
                );
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
                                gizmo::Tool::Face => {
                                    "Click a flat face of a model to lay it on the plate"
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
        // The Face tool: the flat face under the mouse, to lay down on a click.
        self.face_hover = match (self.tool, response.hover_pos(), &self.drag) {
            (gizmo::Tool::Face, Some(at), Drag::None) if !preview => self.face_under(rect, at),
            _ => None,
        };
        if self.face_hover.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
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
            match self.face_hover.take() {
                Some((oi, tris)) => self.lay_flat_on_face(oi, &tris),
                None => self.select_clicked_object(rect, pointer),
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
            }
        }
    }

    /// The nearest part under `at` and the triangles of the flat face hit there.
    fn face_under(&self, rect: egui::Rect, at: egui::Pos2) -> Option<(usize, Vec<usize>)> {
        let (eye, dir) = self.camera.ray(at, rect);
        let (oi, tri, _) = self
            .objects
            .iter()
            .enumerate()
            .filter_map(|(oi, o)| o.face_hit(eye, dir).map(|(tri, along)| (oi, tri, along)))
            .min_by(|a, b| a.2.total_cmp(&b.2))?;
        Some((oi, self.objects[oi].coplanar_with(tri)))
    }

    /// Lays part `oi` flat on the face made of triangles `tris`, and selects it.
    fn lay_flat_on_face(&mut self, oi: usize, tris: &[usize]) {
        let Some(o) = self.objects.get_mut(oi) else {
            return;
        };
        let Some(n) = tris.first().and_then(|&i| o.face_normal(i)) else {
            return;
        };
        o.lay_flat_on(n);
        self.selected = Some(oi);
        self.refresh_plate();
        self.status = format!("{} laid flat on the face picked.", self.objects[oi].name);
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
                start_scale: o.scale,
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
        let z_here = layer.z;
        let label = format!("Layer {} of {}  ·  {:.2} mm", self.layer + 1, n, layer.z);
        ui.painter().text(
            egui::pos2(rect.center().x, rect.top() + 24.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(14.0),
            theme::TEXT,
        );
        // Colour changes and pauses: ticks beside the slider at their layers.
        let zs: Vec<f32> = sliced.result.layers.iter().map(|l| l.z).collect();
        let (track_top, track_bottom) = (slider_rect.top() + 10.0, slider_rect.bottom() - 10.0);
        for e in &self.layer_events {
            let Some(li) = crate::layer_events::layer_for(e.z, &zs) else {
                continue;
            };
            let f = li as f32 / (n - 1).max(1) as f32;
            let y = track_bottom - f * (track_bottom - track_top);
            ui.painter().line_segment(
                [
                    egui::pos2(slider_rect.left() - 10.0, y),
                    egui::pos2(slider_rect.left() + 6.0, y),
                ],
                egui::Stroke::new(3.0, event_color(e.kind)),
            );
        }
        self.layer_event_controls(ui, rect, z_here, &zs);
        ui.painter().text(
            rect.left_bottom() + egui::vec2(12.0, -12.0),
            egui::Align2::LEFT_BOTTOM,
            "Up / Down arrows step through layers",
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
    }

    /// Under the layer label: buttons adding a colour change or a pause before the layer
    /// shown, and the list of those already added, each with a button removing it.
    fn layer_event_controls(&mut self, ui: &mut Ui, rect: egui::Rect, z_here: f32, zs: &[f32]) {
        use crate::layer_events::{layer_for, LayerEvent, LayerEventKind};
        let area = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + 48.0),
            egui::vec2(520.0, 26.0),
        );
        let first_layer = self.layer == 0;
        let why_not = if self.printer.x3g {
            Some("MakerBot / Sailfish printers have no pause or colour-change command")
        } else if first_layer {
            Some("Pick a layer above the first with the slider")
        } else {
            None
        };
        let mut changed = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ui.horizontal(|ui| {
                ui.add_space((area.width() - 330.0).max(0.0) / 2.0);
                for kind in [LayerEventKind::ColourChange, LayerEventKind::Pause] {
                    let button = egui::Button::new(
                        RichText::new(format!("+ {} here", kind.label())).color(event_color(kind)),
                    );
                    let hint = match kind {
                        LayerEventKind::ColourChange => {
                            "Stop before this layer for a filament swap (M600), then carry on"
                        }
                        LayerEventKind::Pause => {
                            "Stop before this layer until you resume (PAUSE on Klipper, M601 otherwise)"
                        }
                    };
                    let response = ui
                        .add_enabled(why_not.is_none(), button)
                        .on_hover_text(hint)
                        .on_disabled_hover_text(why_not.unwrap_or(hint));
                    let event = LayerEvent { z: z_here, kind };
                    if response.clicked() && !self.layer_events.contains(&event) {
                        self.layer_events.push(event);
                        self.layer_events.sort_by(|a, b| a.z.total_cmp(&b.z));
                        changed = true;
                    }
                }
            });
        });
        let mut remove = None;
        for (i, e) in self.layer_events.iter().enumerate() {
            let row = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, area.bottom() + 14.0 + 24.0 * i as f32),
                egui::vec2(320.0, 22.0),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
                ui.horizontal(|ui| {
                    let at = match layer_for(e.z, zs) {
                        Some(li) => format!("layer {}", li + 1),
                        None => "above the print".to_string(),
                    };
                    ui.label(
                        RichText::new(format!("{} before {at}  ({:.2} mm)", e.kind.label(), e.z))
                            .color(event_color(e.kind)),
                    );
                    if ui.small_button("x").on_hover_text("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            self.layer_events.remove(i);
            changed = true;
        }
        if changed {
            self.refresh_print_file();
        }
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
        // Ctrl+Z undoes; Ctrl+Y or Ctrl+Shift+Z redoes (fields being typed in keep their
        // own undo: this is skipped above while typing).
        let (undo, redo) = ctx.input(|i| {
            let z = i.modifiers.command && i.key_pressed(egui::Key::Z);
            let y = i.modifiers.command && i.key_pressed(egui::Key::Y);
            (z && !i.modifiers.shift, y || (z && i.modifiers.shift))
        });
        if undo {
            self.undo();
        }
        if redo {
            self.redo();
        }
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
                (egui::Key::F, gizmo::Tool::Face),
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
        self.poll_file_dialog();
        // Last frame's change becomes an undo step once the edit is finished: no mouse
        // button held and no field being typed in.
        let idle = !ctx.input(|i| i.pointer.any_down()) && ctx.memory(|m| m.focused().is_none());
        self.note_changes(idle);
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
fn object_placement_grid(
    ui: &mut Ui,
    o: &mut PlacedObject,
    ox: f32,
    oy: f32,
    scale_lock: &mut bool,
) -> bool {
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
        // Locked: one slider scaling all three axes together (in proportion, if they
        // differ). Unlocked: a field per axis, along the model's own axes.
        ui.label("Scale");
        ui.horizontal(|ui| {
            if *scale_lock {
                let mut s = o.scale.x;
                let slider = egui::Slider::new(&mut s, 10.0..=500.0)
                    .suffix(" %")
                    .logarithmic(true);
                if ui.add(slider).changed() {
                    let ratio = s / o.scale.x.max(0.01);
                    let axis = |v: f32| (v * ratio).clamp(1.0, 2000.0);
                    o.scale = Vec3::new(s, axis(o.scale.y), axis(o.scale.z));
                    changed = true;
                }
            }
            ui.checkbox(scale_lock, "Lock")
                .on_hover_text("Scale X, Y and Z together. Untick to set each axis on its own");
        });
        ui.end_row();
        if !*scale_lock {
            for (label, v) in [
                ("Scale X", &mut o.scale.x),
                ("Scale Y", &mut o.scale.y),
                ("Scale Z", &mut o.scale.z),
            ] {
                ui.label(label)
                    .on_hover_text("Along the model's own axis, before it is turned");
                changed |= ui
                    .add(
                        egui::DragValue::new(v)
                            .speed(1.0)
                            .range(1.0..=2000.0)
                            .suffix(" %"),
                    )
                    .changed();
                ui.end_row();
            }
        }
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
            o.scale = Vec3::new(100.0, 100.0, 100.0);
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
        if ui
            .button("Split")
            .on_hover_text("Make each separate piece of this part an object of its own")
            .clicked()
        {
            action = Some(ObjectAction::Split);
        }
    });
    (changed, action)
}

/// "Cut at [height] [parts to keep] [Cut]" for an object `height` mm tall; a height
/// outside the object starts again at half its height. Returns whether Cut was clicked,
/// and whether the mouse is over the row or its height is being edited.
fn cut_row(ui: &mut Ui, z: &mut f32, keep: &mut crate::cut::CutKeep, height: f32) -> (bool, bool) {
    if !(*z > 0.0 && *z < height) {
        *z = (height / 2.0 * 10.0).round() / 10.0;
    }
    let mut clicked = false;
    let mut editing = false;
    let row = ui.horizontal(|ui| {
        ui.label("Cut at");
        let field = ui.add(
            egui::DragValue::new(z)
                .speed(0.1)
                .range(0.1..=(height - 0.1).max(0.1))
                .suffix(" mm"),
        );
        editing = field.dragged() || field.has_focus();
        egui::ComboBox::from_id_salt("cut_keep")
            .selected_text(keep.label())
            .show_ui(ui, |ui| {
                for k in crate::cut::CutKeep::ALL {
                    ui.selectable_value(keep, k, k.label());
                }
            });
        clicked = ui
            .button("Cut")
            .on_hover_text(
                "Cut the part across at this height. The upper part is turned over so its cut \
                 face sits on the plate",
            )
            .clicked();
    });
    let over = ui.rect_contains_pointer(row.response.rect);
    (clicked, over || editing)
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

/// How many undo steps are kept.
const UNDO_STEPS: usize = 100;

/// What undo and redo bring back: the plate and every setting a project saves. Meshes
/// are shared, so a copy is small.
#[derive(Clone, Debug, Default)]
struct PlateState {
    objects: Vec<PlacedObject>,
    settings: PrintSettings,
    quality: Option<Quality>,
    printer: Option<Printer>,
    filament: Option<Filament>,
    dual: crate::profiles::DualNozzle,
    layer_events: Vec<crate::layer_events::LayerEvent>,
}

/// What the preview colours paths by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewColour {
    /// The kind of line (outer wall, infill, ...).
    Type,
    /// The nozzle that prints it (two-nozzle printers).
    Nozzle,
    Speed,
    Width,
    LayerHeight,
    Fan,
    Temperature,
}

impl PreviewColour {
    const ALL: [Self; 7] = [
        Self::Type,
        Self::Nozzle,
        Self::Speed,
        Self::Width,
        Self::LayerHeight,
        Self::Fan,
        Self::Temperature,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Type => "Line type",
            Self::Nozzle => "Nozzle",
            Self::Speed => "Speed",
            Self::Width => "Line width",
            Self::LayerHeight => "Layer height",
            Self::Fan => "Fan",
            Self::Temperature => "Temperature",
        }
    }

    /// How the scale's values are written.
    fn format(self, v: f32) -> String {
        match self {
            Self::Speed => format!("{v:.0} mm/s"),
            Self::Width | Self::LayerHeight => format!("{v:.2} mm"),
            Self::Fan => format!("{v:.0} %"),
            Self::Temperature => format!("{v:.0} °C"),
            Self::Type | Self::Nozzle => String::new(),
        }
    }

    /// A printed path's value on this scale (`layer_height` is its layer's); `None` for
    /// the type and nozzle modes, and for a path that wasn't printed.
    fn value(self, st: &crate::gcode::PathStats, layer_height: f32) -> Option<f32> {
        if st.seconds <= 0.0 {
            return None;
        }
        match self {
            Self::Speed => Some(st.speed),
            Self::Width => Some(st.width),
            Self::LayerHeight => Some(layer_height),
            Self::Fan => Some(st.fan as f32),
            Self::Temperature => Some(st.temp as f32),
            Self::Type | Self::Nozzle => None,
        }
    }
}

/// The lowest and highest value of `mode` over every printed path of the slice, for
/// the colour scale; `None` for the type and nozzle modes or an empty slice.
fn colour_range(mode: PreviewColour, sliced: &Sliced) -> Option<(f32, f32)> {
    let mut range: Option<(f32, f32)> = None;
    for (li, layer) in sliced.gcode.path_stats.iter().enumerate() {
        let height = sliced
            .result
            .layers
            .get(li)
            .map(|l| l.height)
            .unwrap_or(0.0);
        for st in layer {
            if let Some(v) = mode.value(st, height) {
                range = Some(match range {
                    Some((lo, hi)) => (lo.min(v), hi.max(v)),
                    None => (v, v),
                });
            }
        }
    }
    range
}

/// The colour scale for a value mode: a bar from blue to red with its lowest, middle
/// and highest values.
fn gradient_legend(ui: &mut Ui, mode: PreviewColour, lo: f32, hi: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 12.0), Sense::hover());
    let steps = 24;
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let x0 = rect.left() + rect.width() * t0;
        let x1 = rect.left() + rect.width() * (i + 1) as f32 / steps as f32;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            theme::gradient((t0 + 0.5 / steps as f32).min(1.0)),
        );
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(mode.format(lo))
                .small()
                .color(theme::TEXT_DIM),
        );
        if hi > lo {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(mode.format(hi))
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(mode.format((lo + hi) / 2.0))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
            });
        }
    });
}

/// A short duration: `12 s`, `2 min`, `1 h 2 min`.
fn duration_text(seconds: f32) -> String {
    let s = seconds.max(0.0).round() as u64;
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// The colour that marks a colour change or a pause in the preview.
fn event_color(kind: crate::layer_events::LayerEventKind) -> Color32 {
    match kind {
        crate::layer_events::LayerEventKind::ColourChange => Color32::from_rgb(255, 160, 60),
        crate::layer_events::LayerEventKind::Pause => Color32::from_rgb(90, 180, 255),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(name: &str) -> PlacedObject {
        PlacedObject::new(name, Mesh::cube(Vec3::new(10.0, 10.0, 10.0)))
    }

    /// Waits (up to 5 s) for the open file dialog's answer and acts on it.
    fn finish_dialog(app: &mut PrintCraftApp) {
        for _ in 0..500 {
            app.poll_file_dialog();
            if app.file_dialog.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the dialog never answered");
    }

    #[test]
    fn test_file_dialog_answers_from_its_own_thread() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/temp/dialogs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cube.stl");
        std::fs::write(
            &path,
            Mesh::cube(Vec3::new(10.0, 10.0, 10.0)).to_binary_stl(),
        )
        .unwrap();
        let mut app = PrintCraftApp::for_tests();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let picked = path.clone();
        app.ask_file(move || {
            // The "person" picks only when the test says so.
            gate_rx.recv().ok()?;
            Some(dialogs::Picked::Models(vec![picked]))
        });
        // The window carries on while the dialog is open, and a second one waits.
        app.poll_file_dialog();
        assert!(app.file_dialog.is_some() && app.objects.is_empty());
        app.ask_file(|| panic!("a second dialog must not open"));
        gate_tx.send(()).unwrap();
        finish_dialog(&mut app);
        assert_eq!(app.objects.len(), 1);
        // A cancelled dialog does nothing.
        app.ask_file(|| None);
        finish_dialog(&mut app);
        assert_eq!(app.objects.len(), 1);
    }

    #[test]
    fn test_save_as_from_save_changes_then_does_the_new_project() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/temp/dialogs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("saved-then-new");
        let mut app = PrintCraftApp::for_tests();
        app.add_object(PlacedObject::new(
            "box",
            Mesh::cube(Vec3::new(10.0, 10.0, 10.0)),
        ));
        let target = path.clone();
        app.ask_file(move || {
            Some(dialogs::Picked::SaveProject {
                path: target,
                then: Some(project_ui::ProjectAction::New),
            })
        });
        finish_dialog(&mut app);
        let saved = crate::project::with_project_extension(path);
        assert!(saved.exists(), "saved first");
        assert!(app.objects.is_empty(), "then the new project");
        std::fs::remove_file(saved).unwrap();
    }

    #[test]
    fn test_split_selected_object_into_its_pieces() {
        let mut app = PrintCraftApp::for_tests();
        let mut tris = Mesh::cube(Vec3::new(10.0, 10.0, 10.0)).triangles;
        tris.extend(
            Mesh::cube(Vec3::new(4.0, 4.0, 4.0))
                .triangles
                .iter()
                .map(|t| t.map(|v| v + Vec3::new(20.0, 0.0, 0.0))),
        );
        app.add_object(PlacedObject::new("pair", Mesh { triangles: tris }));
        app.note_changes(true);
        app.apply_to_selected(ObjectAction::Split);
        assert_eq!(app.status, "Split pair into 2 objects.");
        assert_eq!(app.objects.len(), 2);
        assert_eq!(app.plate.len(), 2);
        // The pieces stay 20 mm apart, as they were.
        let gap = app.object_bounds(1).center().x - app.object_bounds(0).center().x;
        assert!((gap - 20.0).abs() < 1e-3, "{gap}");
        app.note_changes(true);
        app.undo();
        assert_eq!(app.objects.len(), 1);
        // A part in one piece is left alone.
        app.selected = Some(0);
        app.objects[0] = PlacedObject::new("box", Mesh::cube(Vec3::new(5.0, 5.0, 5.0)));
        app.refresh_plate();
        app.apply_to_selected(ObjectAction::Split);
        assert_eq!(app.objects.len(), 1);
        assert_eq!(app.status, "box is one piece: there is nothing to split.");
    }

    #[test]
    fn test_cut_selected_object_replaces_it_with_its_parts() {
        let mut app = PrintCraftApp::for_tests();
        app.add_object(PlacedObject::new(
            "box",
            Mesh::cube(Vec3::new(20.0, 20.0, 20.0)),
        ));
        app.note_changes(true);
        // A height outside the part (here the plate itself) cuts nothing.
        app.cut_z = 0.0;
        app.apply_to_selected(ObjectAction::Cut);
        assert_eq!(app.objects.len(), 1);
        assert_eq!(app.status, "Choose a cut height between 0 and 20.0 mm.");
        app.cut_z = 5.0;
        app.cut_keep = crate::cut::CutKeep::Both;
        app.apply_to_selected(ObjectAction::Cut);
        assert_eq!(app.status, "Cut box at 5.0 mm into two parts.");
        let names: Vec<&str> = app.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["box (lower)", "box (upper)"]);
        assert_eq!(app.plate.len(), 2);
        let heights: Vec<f32> = (0..2).map(|i| app.object_bounds(i).size().z).collect();
        assert!((heights[0] - 5.0).abs() < 1e-2 && (heights[1] - 15.0).abs() < 1e-2);
        assert_eq!(app.selected, Some(0));
        // One undo step brings the box back.
        app.note_changes(true);
        app.undo();
        assert_eq!(app.objects.len(), 1);
        assert_eq!(app.objects[0].name, "box");
        // Keeping only the upper part.
        app.selected = Some(0);
        app.cut_keep = crate::cut::CutKeep::Upper;
        app.apply_to_selected(ObjectAction::Cut);
        assert_eq!(app.objects.len(), 1);
        assert_eq!(app.objects[0].name, "box (upper)");
        assert_eq!(app.status, "Cut box at 5.0 mm and kept one part.");
    }

    #[test]
    fn test_import_repairs_a_broken_model_and_says_what_was_fixed() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/temp/repair-import");
        std::fs::create_dir_all(&dir).unwrap();
        // A cube with one side missing and another side turned inside out.
        let mut broken = Mesh::cube(Vec3::new(20.0, 20.0, 20.0));
        broken.triangles.drain(6..8);
        for t in &mut broken.triangles[4..6] {
            t.swap(1, 2);
        }
        let path = dir.join("broken.stl");
        std::fs::write(&path, broken.to_binary_stl()).unwrap();
        let clean = dir.join("clean.stl");
        std::fs::write(
            &clean,
            Mesh::cube(Vec3::new(20.0, 20.0, 20.0)).to_binary_stl(),
        )
        .unwrap();

        let mut app = PrintCraftApp::for_tests();
        app.load_paths(&[path]);
        assert_eq!(
            app.status,
            "Added 1 model. Repaired broken: flipped 2 faces, closed 1 hole."
        );
        assert_eq!(app.objects[0].mesh.triangles.len(), 12);
        // A clean model says nothing about repairs.
        app.load_paths(&[clean]);
        assert_eq!(app.status, "Added 1 model.");
    }

    #[test]
    fn test_face_tool_finds_the_face_under_the_mouse_and_lays_it_down() {
        let mut app = PrintCraftApp::for_tests();
        app.add_object(PlacedObject::new(
            "box",
            Mesh::cube(Vec3::new(30.0, 20.0, 10.0)),
        ));
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let o = &app.objects[0];
        let b = o.world_bounds();
        // A point on the top face, as the camera shows it.
        let top = Vec3::new(b.center().x + 3.0, b.center().y - 2.0, b.max.z);
        let (at, _) = app.camera.project(top, rect).expect("on screen");
        let (oi, tris) = app
            .face_under(rect, at)
            .expect("the top is under the mouse");
        assert_eq!(oi, 0);
        assert_eq!(tris.len(), 2, "the top's two triangles");
        let world = app.objects[0].world_triangles();
        for &i in &tris {
            let t = world[i];
            assert!((t[0].z - b.max.z).abs() < 1e-3, "a top triangle");
        }
        // Laid on a side (the 30 x 10 one): it then stands 20 mm tall.
        let side = Vec3::new(b.center().x, b.min.y, b.center().z);
        let side_tris: Vec<usize> = (0..world.len())
            .filter(|&i| world[i].iter().all(|v| (v.y - side.y).abs() < 1e-3))
            .collect();
        app.lay_flat_on_face(0, &side_tris);
        let h = app.objects[0].world_bounds().size().z;
        assert!((h - 20.0).abs() < 1e-3, "height {h}");
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn test_undo_and_redo_plate_and_settings_edits() {
        let mut app = PrintCraftApp::for_tests();
        let empty = app.project_fingerprint();
        app.add_object(cube("a"));
        app.note_changes(true);
        let added = app.project_fingerprint();
        app.objects[0].x = 25.0;
        app.note_changes(true);
        let moved = app.project_fingerprint();
        app.settings.walls = 4;
        app.note_changes(true);
        let walls = app.project_fingerprint();
        assert!(app.can_undo() && !app.can_redo());
        for want in [&moved, &added, &empty] {
            app.undo();
            assert_eq!(&app.project_fingerprint(), want);
        }
        assert!(!app.can_undo());
        for want in [&added, &moved, &walls] {
            app.redo();
            assert_eq!(&app.project_fingerprint(), want);
        }
        assert!(!app.can_redo());
    }

    #[test]
    fn test_undo_a_drag_is_one_step() {
        let mut app = PrintCraftApp::for_tests();
        app.add_object(cube("a"));
        app.note_changes(true);
        let before = app.project_fingerprint();
        // While the mouse is held, moves aren't steps of their own.
        for x in [5.0, 10.0, 15.0] {
            app.objects[0].x = x;
            app.note_changes(false);
        }
        app.note_changes(true);
        app.undo();
        assert_eq!(app.project_fingerprint(), before);
    }

    #[test]
    fn test_undo_keeps_meshes_shared_and_a_new_edit_drops_redo() {
        let mut app = PrintCraftApp::for_tests();
        app.add_object(cube("a"));
        app.note_changes(true);
        let mesh = app.objects[0].mesh.clone();
        app.objects[0].x = 30.0;
        app.note_changes(true);
        app.undo();
        assert!(
            std::sync::Arc::ptr_eq(&app.objects[0].mesh, &mesh),
            "no copy of the triangles"
        );
        assert!(app.can_redo());
        app.objects[0].y = 12.0;
        app.note_changes(true);
        assert!(
            !app.can_redo(),
            "a new edit after undo can't be redone over"
        );
    }

    #[test]
    fn test_undo_history_starts_again_with_a_new_project() {
        let mut app = PrintCraftApp::for_tests();
        app.add_object(cube("a"));
        app.note_changes(true);
        assert!(app.can_undo());
        app.reset_history();
        assert!(!app.can_undo() && !app.can_redo());
    }

    #[test]
    fn test_preview_gradient_runs_blue_to_red() {
        let (lo, hi) = (theme::gradient(0.0), theme::gradient(1.0));
        assert!(lo.b() > lo.r(), "cool end is blue: {lo:?}");
        assert!(hi.r() > hi.b(), "hot end is red: {hi:?}");
        assert_eq!(theme::gradient(-1.0), lo);
        assert_eq!(theme::gradient(2.0), hi);
        assert_ne!(theme::gradient(0.5), lo);
    }

    #[test]
    fn test_preview_colour_values() {
        let st = crate::gcode::PathStats {
            speed: 42.0,
            width: 0.42,
            fan: 80,
            temp: 210,
            seconds: 1.0,
        };
        assert_eq!(PreviewColour::Speed.value(&st, 0.2), Some(42.0));
        assert_eq!(PreviewColour::Width.value(&st, 0.2), Some(0.42));
        assert_eq!(PreviewColour::LayerHeight.value(&st, 0.2), Some(0.2));
        assert_eq!(PreviewColour::Fan.value(&st, 0.2), Some(80.0));
        assert_eq!(PreviewColour::Temperature.value(&st, 0.2), Some(210.0));
        assert_eq!(PreviewColour::Type.value(&st, 0.2), None);
        // A path that wasn't printed has no value.
        let unprinted = crate::gcode::PathStats::default();
        assert_eq!(PreviewColour::Speed.value(&unprinted, 0.2), None);
    }

    #[test]
    fn test_duration_text() {
        assert_eq!(duration_text(12.4), "12 s");
        assert_eq!(duration_text(125.0), "2 min");
        assert_eq!(duration_text(3725.0), "1 h 2 min");
    }

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
