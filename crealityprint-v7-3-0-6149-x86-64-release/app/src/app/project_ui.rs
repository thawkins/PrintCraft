//! Projects in the window: the File menu (New, Open, Save, Save As), the question about
//! unsaved changes, and the window title showing the project's name.

use super::*;
use crate::project::{self, Project};

/// Something that would replace the current plate, waiting for "discard changes?".
#[derive(Clone, Debug)]
pub(super) enum ProjectAction {
    New,
    /// Open this file, or ask which file when None.
    Open(Option<PathBuf>),
}

impl PrintCraftApp {
    /// The File menu at the left of the top bar, as in the original's window.
    pub(super) fn file_menu(&mut self, ui: &mut Ui) {
        let menu = ui.menu_button(RichText::new("File").size(15.0), |ui| {
            ui.set_min_width(220.0);
            if ui.button("New project").clicked() {
                self.request(ProjectAction::New);
                ui.close();
            }
            if ui.button("Open project…").clicked() {
                self.request(ProjectAction::Open(None));
                ui.close();
            }
            if ui.button("Save project        Ctrl+S").clicked() {
                self.save_project();
                ui.close();
            }
            if ui.button("Save project as…   Ctrl+Shift+S").clicked() {
                self.save_project_as();
                ui.close();
            }
            ui.separator();
            self.recent_projects_menu(ui);
            ui.separator();
            if ui.button("Add model (STL, OBJ, 3MF)…   Ctrl+O").clicked() {
                self.open_dialog();
                ui.close();
            }
            let can_export = self.slice_is_current();
            if ui
                .add_enabled(
                    can_export,
                    egui::Button::new(if self.printer.x3g {
                        "Export X3G…"
                    } else {
                        "Export G-code…"
                    }),
                )
                .clicked()
            {
                self.export();
                ui.close();
            }
        });
        if std::mem::take(&mut self.open_file_menu) {
            egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&menu.response));
        }
    }

    /// The Edit menu beside File: undo and redo of plate and settings changes.
    pub(super) fn edit_menu(&mut self, ui: &mut Ui) {
        ui.menu_button(RichText::new("Edit").size(15.0), |ui| {
            ui.set_min_width(200.0);
            if ui
                .add_enabled(
                    self.can_undo(),
                    egui::Button::new("Undo              Ctrl+Z"),
                )
                .clicked()
            {
                self.undo();
                ui.close();
            }
            if ui
                .add_enabled(
                    self.can_redo(),
                    egui::Button::new("Redo              Ctrl+Y"),
                )
                .clicked()
            {
                self.redo();
                ui.close();
            }
        });
    }

    /// The newest projects, numbered, each opening with one click. A file that has been
    /// moved or deleted is shown greyed out.
    fn recent_projects_menu(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("Recent projects")
                .small()
                .color(theme::TEXT_DIM),
        );
        if self.recent.list.is_empty() {
            ui.add_enabled(false, egui::Button::new("No recent projects"));
            return;
        }
        let mut open = None;
        for (i, path) in self.recent.list.iter().enumerate() {
            let exists = path.is_file();
            let text = format!("{}   {}", i + 1, display_name(path));
            let hint = if exists {
                path.display().to_string()
            } else {
                format!("Not found: {}", path.display())
            };
            let b = ui
                .add_enabled(exists, egui::Button::new(text))
                .on_hover_text(&hint)
                .on_disabled_hover_text(&hint);
            if b.clicked() {
                open = Some(path.clone());
                ui.close();
            }
        }
        if ui
            .add(egui::Button::new(
                RichText::new("Clear recent projects")
                    .small()
                    .color(theme::TEXT_DIM),
            ))
            .clicked()
        {
            self.recent.clear();
        }
        if let Some(path) = open {
            self.request(ProjectAction::Open(Some(path)));
        }
    }

    /// True when the plate or settings changed since the project was last saved or opened.
    pub(super) fn has_unsaved_changes(&self) -> bool {
        self.project_fingerprint() != self.saved_fingerprint
    }

    /// Runs `action`, first asking about unsaved changes if there are any.
    pub(super) fn request(&mut self, action: ProjectAction) {
        if self.has_unsaved_changes() && self.any_plate_has_objects() {
            self.pending_project = Some(action);
        } else {
            self.run(action);
        }
    }

    pub(super) fn run(&mut self, action: ProjectAction) {
        match action {
            ProjectAction::New => self.new_project(),
            ProjectAction::Open(Some(path)) => self.open_project(&path),
            ProjectAction::Open(None) => {
                if self.screenshot.is_some() {
                    return;
                }
                self.ask_file(|| {
                    rfd::FileDialog::new()
                        .add_filter("PrintCraft project", &[project::EXTENSION])
                        .pick_file()
                        .map(super::dialogs::Picked::OpenProject)
                });
            }
        }
    }

    fn new_project(&mut self) {
        self.clear_plate();
        self.set_more_plates(Vec::new());
        self.layer_events.clear();
        self.sliced = None;
        self.reset_history();
        self.project_path = None;
        self.mode = Mode::Prepare;
        self.saved_fingerprint = self.project_fingerprint();
        self.status = "New project.".into();
        self.update_title();
    }

    /// What would be saved right now.
    pub(super) fn current_project(&self) -> Project {
        let mut plates = self.all_plates().into_iter();
        Project {
            printer: self.printer.clone(),
            filament: self.filament.clone(),
            settings: self.settings.clone(),
            quality: self.quality,
            objects: plates.next().unwrap_or_default(),
            more_plates: plates.collect(),
            device: self.device.current_device(),
            dual: self.dual.clone(),
            layer_events: self.layer_events.clone(),
        }
    }

    pub(super) fn open_project(&mut self, path: &Path) {
        let opened = match project::open(path) {
            Ok(o) => o,
            Err(e) => {
                // A project that is gone no longer belongs in the recent list.
                if !path.exists() {
                    self.recent.remove(path);
                }
                self.device.show_alert(
                    "Could not open the project",
                    format!("{}\n\n{e}", display_name(path)),
                );
                return;
            }
        };
        self.recent.add(path);
        let newer = opened
            .is_from_newer_version()
            .then_some(opened.file_version);
        self.apply_project(opened.project);
        self.reset_history();
        self.project_path = Some(path.to_path_buf());
        self.saved_fingerprint = self.project_fingerprint();
        self.status = format!("Opened {}.", display_name(path));
        if let Some(version) = newer {
            self.status = format!(
                "Opened {}: saved by a newer PrintCraft, some settings may be missing.",
                display_name(path)
            );
            self.device.show_alert(
                "Project from a newer PrintCraft",
                format!(
                    "{} was saved by a newer version of PrintCraft (project format {version}; \
                     this version reads format {}).\n\nIt may use features this version \
                     doesn't support. Those settings are left out, and saving the project \
                     here will drop them.",
                    display_name(path),
                    project::VERSION
                ),
            );
        }
        self.update_title();
    }

    /// Puts a loaded project in place: plate, settings, printer profile and printer device.
    pub(super) fn apply_project(&mut self, p: Project) {
        // The printer profile: the saved values, under its saved name (a preset or not).
        self.camera = Camera::for_plate(p.printer.bed_x, p.printer.bed_y);
        self.printer = p.printer;
        self.filament = p.filament;
        self.dual = p.dual;
        self.layer_events = p.layer_events;
        self.settings = p.settings;
        self.quality = p
            .quality
            .or_else(|| Quality::ALL.into_iter().find(|q| q.matches(&self.settings)));
        self.cancel_slice();
        self.objects = p.objects;
        self.set_more_plates(p.more_plates);
        self.refresh_plate();
        self.selected = None;
        self.drag = Drag::None;
        self.sliced = None;
        self.cancel_slice();
        self.mode = Mode::Prepare;
        // The printer device: looked for, then selected only if it is there.
        match p.device {
            Some(d) => self.device.select_saved_device(d, &self.egui_ctx),
            None => self.device.deselect(),
        }
    }

    pub(super) fn save_project(&mut self) {
        match self.project_path.clone() {
            Some(path) => self.save_to(&path),
            None => self.save_project_as(),
        }
    }

    pub(super) fn save_project_as(&mut self) {
        self.save_project_as_then(None);
    }

    /// Asks where to save the project, saves it there, then does `then` (when saved).
    fn save_project_as_then(&mut self, then: Option<ProjectAction>) {
        if self.screenshot.is_some() {
            return;
        }
        let name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .or_else(|| self.objects.first().map(|o| o.name.clone()))
            .unwrap_or_else(|| "plate".into());
        self.ask_file(move || {
            rfd::FileDialog::new()
                .add_filter("PrintCraft project", &[project::EXTENSION])
                .set_file_name(format!("{name}.{}", project::EXTENSION))
                .save_file()
                .map(|path| super::dialogs::Picked::SaveProject { path, then })
        });
    }

    pub(super) fn save_to(&mut self, path: &Path) {
        match project::save(path, &self.current_project()) {
            Ok(()) => {
                self.recent.add(path);
                self.project_path = Some(path.to_path_buf());
                self.saved_fingerprint = self.project_fingerprint();
                self.status = format!("Saved {}.", display_name(path));
                self.update_title();
            }
            Err(e) => self.device.show_alert("Could not save the project", e),
        }
    }

    /// "PrintCraft — name" in the window's title bar.
    pub(super) fn update_title(&self) {
        let title = match &self.project_path {
            Some(p) => format!("PrintCraft — {}", display_name(p)),
            None => "PrintCraft".to_string(),
        };
        self.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// "Save changes?" before New or Open replaces a plate that hasn't been saved.
    pub(super) fn discard_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending_project.clone() else {
            return;
        };
        let mut answer: Option<u8> = None;
        egui::Modal::new(egui::Id::new("unsaved_changes"))
            .frame(crate::widgets::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Save changes?");
                ui.add_space(6.0);
                ui.label("The plate or settings have changed since the project was last saved.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Save").color(Color32::WHITE))
                                .fill(theme::ACCENT_DARK),
                        )
                        .clicked()
                    {
                        answer = Some(0);
                    }
                    if ui.button("Don't save").clicked() {
                        answer = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(2);
                    }
                });
            });
        match answer {
            Some(0) => {
                self.pending_project = None;
                match self.project_path.clone() {
                    Some(path) => {
                        self.save_to(&path);
                        if !self.has_unsaved_changes() {
                            self.run(action);
                        }
                    }
                    // Never saved: the action waits for the Save As dialog.
                    None => self.save_project_as_then(Some(action)),
                }
            }
            Some(1) => {
                self.pending_project = None;
                self.run(action);
            }
            Some(_) => self.pending_project = None,
            None => {}
        }
    }
}
