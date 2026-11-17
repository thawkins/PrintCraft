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
        ui.menu_button(RichText::new("File").size(15.0), |ui| {
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
            if ui.button("Add model (STL, OBJ, 3MF)…   Ctrl+O").clicked() {
                self.open_dialog();
                ui.close();
            }
            let can_export = self.slice_is_current();
            if ui
                .add_enabled(can_export, egui::Button::new("Export G-code…"))
                .clicked()
            {
                self.export();
                ui.close();
            }
        });
    }

    /// True when the plate or settings changed since the project was last saved or opened.
    pub(super) fn has_unsaved_changes(&self) -> bool {
        self.fingerprint() != self.saved_fingerprint
    }

    /// Runs `action`, first asking about unsaved changes if there are any.
    pub(super) fn request(&mut self, action: ProjectAction) {
        if self.has_unsaved_changes() && !self.objects.is_empty() {
            self.pending_project = Some(action);
        } else {
            self.run(action);
        }
    }

    fn run(&mut self, action: ProjectAction) {
        match action {
            ProjectAction::New => self.new_project(),
            ProjectAction::Open(Some(path)) => self.open_project(&path),
            ProjectAction::Open(None) => {
                if self.screenshot.is_some() {
                    return;
                }
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PrintCraft project", &[project::EXTENSION])
                    .pick_file()
                {
                    self.open_project(&path);
                }
            }
        }
    }

    fn new_project(&mut self) {
        self.objects.clear();
        self.plate.clear();
        self.selected = None;
        self.drag = Drag::None;
        self.sliced = None;
        self.project_path = None;
        self.mode = Mode::Prepare;
        self.saved_fingerprint = self.fingerprint();
        self.status = "New project.".into();
        self.update_title();
    }

    /// What would be saved right now.
    pub(super) fn current_project(&self) -> Project {
        Project {
            printer: self.printer.clone(),
            filament: self.filament.clone(),
            settings: self.settings.clone(),
            quality: self.quality,
            objects: self.objects.clone(),
            device: self.device.current_device(),
        }
    }

    pub(super) fn open_project(&mut self, path: &Path) {
        let p = match project::load(path) {
            Ok(p) => p,
            Err(e) => {
                self.device.show_alert(
                    "Could not open the project",
                    format!("{}\n\n{e}", display_name(path)),
                );
                return;
            }
        };
        self.apply_project(p);
        self.project_path = Some(path.to_path_buf());
        self.saved_fingerprint = self.fingerprint();
        self.status = format!("Opened {}.", display_name(path));
        self.update_title();
    }

    /// Puts a loaded project in place: plate, settings, printer profile and printer device.
    pub(super) fn apply_project(&mut self, p: Project) {
        // The printer profile: the saved values, under its saved name (a preset or not).
        self.camera = Camera::for_plate(p.printer.bed_x, p.printer.bed_y);
        self.printer = p.printer;
        self.filament = p.filament;
        self.settings = p.settings;
        self.quality = p
            .quality
            .or_else(|| Quality::ALL.into_iter().find(|q| q.matches(&self.settings)));
        self.objects = p.objects;
        self.refresh_plate();
        self.selected = None;
        self.drag = Drag::None;
        self.sliced = None;
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
        if let Some(mut path) = rfd::FileDialog::new()
            .add_filter("PrintCraft project", &[project::EXTENSION])
            .set_file_name(format!("{name}.{}", project::EXTENSION))
            .save_file()
        {
            if !project::is_project(&path) {
                path.set_extension(project::EXTENSION);
            }
            self.save_to(&path);
        }
    }

    pub(super) fn save_to(&mut self, path: &Path) {
        match project::save(path, &self.current_project()) {
            Ok(()) => {
                self.project_path = Some(path.to_path_buf());
                self.saved_fingerprint = self.fingerprint();
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
        egui::Modal::new(egui::Id::new("unsaved_changes")).show(ctx, |ui| {
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
                self.save_project();
                if !self.has_unsaved_changes() {
                    self.run(action);
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
