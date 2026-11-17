//! The main window: page tabs on top, the 3D plate with the Printer box and model tools,
//! Filament / Process settings (or slice results) on the right, and a status bar.
//! The Prepare screen's parts live in prepare_ui.rs.

mod prepare_ui;
mod project_ui;

use crate::device_ui::{DevicePage, PrintJob};
use crate::gcode::{self, GcodeOutput};
use crate::geometry::Vec3;
use crate::gizmo;
use crate::model::{Mesh, PlacedObject, SampleShape, Triangle};
use crate::profiles::{
    Adhesion, Filament, InfillPattern, PrintSettings, Printer, Quality, SeamPosition,
};
use crate::slicer::{self, PathKind, SliceResult};
use crate::theme;
use crate::view3d::{self, Camera, DrawObject};
use eframe::egui::{self, Color32, RichText, Sense, Ui};
use prepare_ui::ProcessTab;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Prepare,
    Preview,
    Printer,
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
        app
    }

    // ----- plate and objects -------------------------------------------------------------

    fn refresh_plate(&mut self) {
        self.plate = self.objects.iter().map(|o| o.world_triangles()).collect();
    }

    fn refresh_object(&mut self, i: usize) {
        if let Some(o) = self.objects.get(i) {
            self.plate[i] = o.world_triangles();
        }
    }

    fn add_object(&mut self, obj: PlacedObject) {
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
        let mut b = crate::geometry::Bounds::empty();
        self.plate[i].iter().flatten().for_each(|v| b.include(*v));
        let (hx, hy) = (
            self.printer.bed_x / 2.0 + 0.01,
            self.printer.bed_y / 2.0 + 0.01,
        );
        b.min.x >= -hx
            && b.max.x <= hx
            && b.min.y >= -hy
            && b.max.y <= hy
            && b.max.z <= self.printer.max_z
    }

    // ----- slicing --------------------------------------------------------------------------

    fn fingerprint(&self) -> String {
        let mut s = format!("{:?}|{:?}|{:?}", self.settings, self.printer, self.filament);
        for o in &self.objects {
            s += &format!(
                "|{}:{}:{}:{:?}:{}:{}",
                o.name,
                o.x,
                o.y,
                o.rotation,
                o.scale_percent,
                o.mesh.triangles.len()
            );
        }
        s
    }

    fn slice_is_current(&self) -> bool {
        self.sliced
            .as_ref()
            .is_some_and(|s| s.fingerprint == self.fingerprint())
    }

    fn line_width(&self) -> f32 {
        self.printer.nozzle_diameter * 1.05
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
        let (settings, printer, filament) = (
            self.settings.clone(),
            self.printer.clone(),
            self.filament.clone(),
        );
        let lw = self.line_width();
        let fingerprint = self.fingerprint();
        let progress = Arc::new(AtomicU32::new(0f32.to_bits()));
        let (tx, rx) = mpsc::channel();
        let p = progress.clone();
        std::thread::spawn(move || {
            let result = slicer::slice(&tris, &settings, lw, &|v| {
                p.store(v.to_bits(), Ordering::Relaxed)
            });
            let mut gcode = gcode::generate(&result, &printer, &filament, &settings);
            // Pictures of the part for the printer's screen.
            let (text, picture) = crate::thumbnail::add_to_gcode(&gcode.text, &tris);
            gcode.text = text;
            let shared_text = Arc::new(gcode.text.clone());
            let _ = tx.send(Sliced {
                result,
                gcode,
                fingerprint,
                shared_text,
                picture: picture.map(Arc::new),
            });
        });
        self.job = Some(SliceJob { progress, rx });
        self.status = "Slicing…".into();
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
                self.sliced = Some(done);
                self.mode = Mode::Preview;
                self.job = None;
                if std::mem::take(&mut self.send_after_slice) {
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
        if self.screenshot.is_some() {
            return;
        }
        let name = self
            .objects
            .first()
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "print".into());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("G-code", &["gcode"])
            .set_file_name(format!("{name}.gcode"))
            .save_file()
        else {
            return;
        };
        self.status = match std::fs::write(&path, &sliced.gcode.text) {
            Ok(()) => format!("Saved {}.", display_name(&path)),
            Err(e) => format!("Could not save the file: {e}"),
        };
    }

    // ----- panels ---------------------------------------------------------------------------

    fn objects_panel(&mut self, ui: &mut Ui) {
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

        let Some(i) = self.selected.filter(|&i| i < self.objects.len()) else {
            return;
        };
        let (ox, oy) = (self.printer.bed_x / 2.0, self.printer.bed_y / 2.0);
        let mut changed = false;
        let mut action = None;
        card(ui, "Selected object", |ui| {
            let o = &mut self.objects[i];
            let size = {
                let mut b = crate::geometry::Bounds::empty();
                self.plate[i].iter().flatten().for_each(|v| b.include(*v));
                b.size()
            };
            ui.label(
                RichText::new(format!(
                    "Size {:.1} × {:.1} × {:.1} mm",
                    size.x, size.y, size.z
                ))
                .color(theme::TEXT_DIM),
            );
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
            ui.horizontal_wrapped(|ui| {
                if ui.button("Turn 90°").clicked() {
                    o.rotation.z = (o.rotation.z + 90.0) % 360.0;
                    changed = true;
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
                    o.lay_flat();
                    changed = true;
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
                    action = Some("dup");
                }
                if ui.button("Delete").clicked() {
                    action = Some("del");
                }
            });
        });
        if changed {
            self.refresh_object(i);
        }
        match action {
            Some("dup") => self.duplicate_selected(),
            Some("del") => self.delete_selected(),
            _ => {}
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
            });
        });
        card(ui, "Show", |ui| {
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
            view3d::draw_toolpaths(
                &painter,
                &self.camera,
                rect,
                &sliced.result,
                self.layer,
                &|k| !hidden.contains(&k),
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
        let mut b = crate::geometry::Bounds::empty();
        self.plate[i].iter().flatten().for_each(|v| b.include(*v));
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
                let mut r = g.start_rotation;
                match g.handle {
                    Handle::RotateX => r.x = wrap(r.x + turn),
                    Handle::RotateY => r.y = wrap(r.y + turn),
                    _ => r.z = wrap(r.z + turn),
                }
                o.rotation = r;
            }
        }
        self.refresh_object(g.index);
    }

    /// The Move / Scale / Rotate buttons at the left of the 3D view.
    fn tool_bar(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let area = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.center().y - 70.0),
            egui::vec2(96.0, 140.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            egui::Frame::new()
                .fill(theme::PANEL)
                .corner_radius(8)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        for t in gizmo::Tool::ALL {
                            let text = RichText::new(format!("{}  {}", t.label(), t.key()));
                            let b = egui::Button::selectable(self.tool == t, text)
                                .min_size(egui::vec2(84.0, 30.0));
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
                            if ui.add(b).on_hover_text(hint).clicked() {
                                self.tool = t;
                            }
                        }
                    });
                });
        });
    }

    fn handle_view_input(
        &mut self,
        ui: &Ui,
        rect: egui::Rect,
        response: &egui::Response,
        preview: bool,
    ) {
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let pick = |app: &Self, at: egui::Pos2| {
            let tris: Vec<&[Triangle]> = app.plate.iter().map(|t| t.as_slice()).collect();
            view3d::pick_object(&app.camera, rect, &tris, at)
        };
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
            let primary = ui.input(|i| i.pointer.primary_down());
            let grabbed = match (primary, pointer, &selection) {
                (true, Some(at), Some((index, b))) => {
                    gizmo::pick(&self.camera, rect, b, self.tool, at).map(|handle| {
                        let o = &self.objects[*index];
                        let center = self
                            .camera
                            .project(b.center(), rect)
                            .map(|p| p.0)
                            .unwrap_or(at);
                        GizmoDrag {
                            index: *index,
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
                _ => None,
            };
            self.drag = match (primary, pointer, grabbed) {
                (_, _, Some(g)) => Drag::Gizmo(g),
                (true, Some(at), None) if !preview => match pick(self, at) {
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
        if response.dragged() {
            let delta = response.drag_delta();
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
        if response.drag_stopped() {
            self.drag = Drag::None;
        }
        if response.clicked() && !preview {
            self.selected = pointer.and_then(|at| pick(self, at));
            if self.selected.is_some() {
                // Show the picked model's position, rotation and size on the right.
                self.process_tab = ProcessTab::Objects;
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
            }
        }
    }

    fn view_overlays(&mut self, ui: &mut Ui, rect: egui::Rect, preview: bool) {
        // Printer box (top left) and model tools (along the top), as in the original.
        if self.mode == Mode::Prepare {
            let boxr = egui::Rect::from_min_size(
                rect.min + egui::vec2(10.0, 10.0),
                egui::vec2(350.0, 260.0),
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
        // View buttons, top right: like the original's TOP / FRONT cube and home button.
        let slider_room = if preview { 70.0 } else { 0.0 };
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
        // Slice plate / Send print, bottom right.
        let buttons = egui::Rect::from_min_size(
            egui::pos2(rect.right() - 240.0 - slider_room, rect.bottom() - 96.0),
            egui::vec2(222.0, 90.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(buttons), |ui| {
            self.slice_buttons(ui)
        });

        if self.objects.is_empty() && !preview {
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

        if !preview {
            let hint = "Click a model to select it · drag its handles to move (M), scale (S) or rotate (R) · drag empty space to turn the view · right-drag to pan · scroll to zoom";
            ui.painter().text(
                rect.left_bottom() + egui::vec2(12.0, -12.0),
                egui::Align2::LEFT_BOTTOM,
                hint,
                egui::FontId::proportional(12.0),
                theme::TEXT_DIM,
            );
            return;
        }
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

pub(crate) fn card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(15.0).strong());
            ui.add_space(4.0);
            add(ui);
        });
    ui.add_space(8.0);
}

fn settings_grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, add);
}

fn row(ui: &mut Ui, label: &str, widget: impl egui::Widget) {
    ui.label(label);
    ui.add(widget);
    ui.end_row();
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
