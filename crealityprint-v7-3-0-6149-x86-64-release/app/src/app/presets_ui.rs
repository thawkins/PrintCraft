//! The person's own presets in the Prepare panel: save the current filament or process
//! settings under a name, pick them again from the same lists as the built-in ones,
//! delete them, and import or export them as a JSON file (`crate::presets`).

use super::dialogs::Picked;
use super::*;
use crate::presets::ProcessPreset;

/// Which kind of preset is being named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PresetKind {
    Filament,
    Process,
}

/// The file extension for exported presets.
const EXTENSION: &str = "json";

impl PrintCraftApp {
    /// The filament list: the built-in filaments, then the person's own.
    pub(super) fn refresh_filaments(&mut self) {
        let mut list = Filament::presets();
        list.extend(self.presets.filaments.iter().cloned());
        self.filaments = list;
    }

    /// The person's own process preset whose settings are the current ones, if any.
    pub(super) fn current_process_preset(&self) -> Option<&ProcessPreset> {
        self.presets
            .processes
            .iter()
            .find(|p| p.settings == self.settings)
    }

    /// Puts the process preset `name`'s settings in place.
    pub(super) fn apply_process_preset(&mut self, name: &str) {
        if let Some(p) = self.presets.processes.iter().find(|p| p.name == name) {
            self.settings = p.settings.clone();
            self.quality = Quality::ALL.into_iter().find(|q| q.matches(&self.settings));
        }
    }

    /// Whether the selected filament is one of the person's own.
    fn filament_is_own(&self) -> bool {
        self.presets
            .filaments
            .iter()
            .any(|f| f.name == self.filament.name)
    }

    /// Saves the named preset; problems go to the status line.
    pub(super) fn save_preset(&mut self, kind: PresetKind, name: &str) {
        let result = match kind {
            PresetKind::Filament => {
                let f = Filament {
                    name: name.to_string(),
                    ..self.filament.clone()
                };
                self.presets.save_filament(&f).map(|()| {
                    self.filament.name = name.trim().to_string();
                })
            }
            PresetKind::Process => self.presets.save_process(name, &self.settings),
        };
        self.status = match result {
            Ok(()) => format!("Saved the preset \"{}\".", name.trim()),
            Err(e) => e,
        };
        self.refresh_filaments();
    }

    /// "Save as…" and "Delete" under a preset list, or the name box while naming one.
    pub(super) fn preset_buttons(&mut self, ui: &mut Ui, kind: PresetKind) {
        if let Some((k, name)) = &mut self.naming {
            if *k == kind {
                let mut done = None;
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(name)
                            .hint_text("Preset name")
                            .desired_width(140.0),
                    );
                    edit.request_focus();
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Save").clicked() || enter {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(false);
                    }
                });
                if let Some(save) = done {
                    let name = name.clone();
                    self.naming = None;
                    if save {
                        self.save_preset(kind, &name);
                    }
                }
                return;
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button("Save as preset…")
                .on_hover_text("Keep these settings under a name of your own")
                .clicked()
            {
                self.naming = Some((kind, String::new()));
            }
            let own = match kind {
                PresetKind::Filament => self.filament_is_own().then(|| self.filament.name.clone()),
                PresetKind::Process => self.current_process_preset().map(|p| p.name.clone()),
            };
            if let Some(name) = own {
                if ui
                    .small_button("Delete preset")
                    .on_hover_text(format!("Remove your preset \"{name}\""))
                    .clicked()
                {
                    let result = match kind {
                        PresetKind::Filament => self.presets.delete_filament(&name),
                        PresetKind::Process => self.presets.delete_process(&name),
                    };
                    self.status = match result {
                        Ok(()) => format!("Deleted the preset \"{name}\"."),
                        Err(e) => e,
                    };
                    self.refresh_filaments();
                }
            }
            if kind == PresetKind::Process {
                if ui
                    .small_button("Import…")
                    .on_hover_text("Add presets from a file exported by PrintCraft")
                    .clicked()
                    && self.screenshot.is_none()
                {
                    self.ask_file(|| {
                        rfd::FileDialog::new()
                            .add_filter("PrintCraft presets", &[EXTENSION])
                            .pick_file()
                            .map(Picked::ImportPresets)
                    });
                }
                let any = !self.presets.filaments.is_empty() || !self.presets.processes.is_empty();
                if ui
                    .add_enabled(any, egui::Button::new("Export…").small())
                    .on_hover_text("Save all your filament and process presets to a file")
                    .clicked()
                    && self.screenshot.is_none()
                {
                    let text = self.presets.export_json();
                    self.ask_file(move || {
                        let path = rfd::FileDialog::new()
                            .add_filter("PrintCraft presets", &[EXTENSION])
                            .set_file_name("printcraft-presets.json")
                            .save_file()?;
                        Some(Picked::ExportPresets { path, text })
                    });
                }
            }
        });
    }

    /// Adds the presets in the file at `path`.
    pub(super) fn import_presets(&mut self, path: &Path) {
        let result = std::fs::read_to_string(path)
            .map_err(|e| format!("Could not read the presets file: {e}"))
            .and_then(|text| self.presets.import_json(&text))
            .and_then(|taken| self.presets.save().map(|()| taken));
        self.status = match result {
            Ok((f, p)) => format!("Imported {f} filament and {p} process preset(s)."),
            Err(e) => e,
        };
        self.refresh_filaments();
    }

    /// Writes exported presets to `path`.
    pub(super) fn write_presets(&mut self, path: &Path, text: &str) {
        self.status = match crate::fsutil::write_atomically(path, text.as_bytes()) {
            Ok(()) => format!("Exported the presets to {}.", path.display()),
            Err(e) => format!("Could not export the presets: {e}"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_presets_ui_saved_presets_join_the_lists_and_apply() {
        let mut app = PrintCraftApp::for_tests();
        let built_in = app.filaments.len();
        app.filament.nozzle_temp = 218;
        app.save_preset(PresetKind::Filament, " Silk PLA ");
        assert_eq!(app.filaments.len(), built_in + 1);
        assert_eq!(app.filaments[built_in].name, "Silk PLA");
        assert_eq!(app.filaments[built_in].nozzle_temp, 218);
        assert_eq!(
            app.filament.name, "Silk PLA",
            "the saved preset is selected"
        );
        assert!(app.filament_is_own());

        app.settings.walls = 5;
        app.save_preset(PresetKind::Process, "Strong");
        assert_eq!(
            app.current_process_preset().map(|p| p.name.as_str()),
            Some("Strong")
        );
        app.settings = PrintSettings::default();
        assert!(app.current_process_preset().is_none());
        app.apply_process_preset("Strong");
        assert_eq!(app.settings.walls, 5);
        assert_eq!(app.quality, None, "a preset of its own, not a quality");

        // A built-in name is refused with a message.
        app.save_preset(PresetKind::Filament, "PETG");
        assert!(app.status.contains("built-in"), "{}", app.status);
        assert_eq!(app.filaments.len(), built_in + 1);
    }
}
