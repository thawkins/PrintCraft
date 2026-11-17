//! The Prepare screen's layout, following the original's main window the person shared
//! (reference/picture-2.png):
//! * page tabs centred in the top bar, the active one filled
//! * a compact Printer box at the top left, over the 3D view, with a row of model tools
//! * Slice plate / Send print at the bottom right of the 3D view
//! * Filament and Process settings on the right, with a Global / Objects switch and
//!   grouped settings (Layer height, Seam, Walls and surfaces, …)
//!
//! The look is PrintCraft's own (no logos or brand artwork).

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ProcessTab {
    #[default]
    Global,
    Objects,
}

/// Nozzle sizes offered in the Printer box.
const NOZZLES: [f32; 4] = [0.2, 0.4, 0.6, 0.8];

impl PrintCraftApp {
    pub(super) fn top_bar(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        ui.horizontal_centered(|ui| {
            ui.label(
                RichText::new("PrintCraft")
                    .size(18.0)
                    .strong()
                    .color(theme::ACCENT),
            );
            ui.add_space(14.0);
            // File menu (New / Open / Save project), as in the original's top-left corner.
            self.file_menu(ui);
            if let Some(name) = self.device.locating_name() {
                ui.add_space(10.0);
                ui.spinner();
                ui.label(RichText::new(format!("Looking for {name}…")).color(theme::TEXT_DIM));
            }
        });
        // Page tabs in the middle of the bar.
        let tabs = egui::Rect::from_center_size(full.center(), egui::vec2(360.0, full.height()));
        ui.scope_builder(egui::UiBuilder::new().max_rect(tabs), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (mode, label) in [
                    (Mode::Prepare, "Prepare"),
                    (Mode::Preview, "Preview"),
                    (Mode::Printer, "Device"),
                ] {
                    let on = self.mode == mode;
                    let text = RichText::new(label).size(16.0).strong();
                    let button = if on {
                        egui::Button::new(text.color(Color32::WHITE)).fill(theme::ACCENT_DARK)
                    } else {
                        egui::Button::new(text).frame(false)
                    };
                    if ui.add(button.min_size(egui::vec2(104.0, 32.0))).clicked() {
                        self.mode = mode;
                    }
                }
            });
        });
        let right = egui::Rect::from_min_max(
            egui::pos2(tabs.right() + 10.0, full.top()),
            full.right_bottom(),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_export = self.slice_is_current();
                let label = if self.printer.x3g {
                    "Export X3G"
                } else {
                    "Export G-code"
                };
                let r = ui.add_enabled(can_export, egui::Button::new(label));
                let r = if can_export {
                    r
                } else {
                    r.on_disabled_hover_text("Slice the plate first")
                };
                if r.clicked() {
                    self.export();
                }
                if ui
                    .button("Open model")
                    .on_hover_text("Add STL, OBJ or 3MF files to the plate (Ctrl+O)")
                    .clicked()
                {
                    self.open_dialog();
                }
            });
        });
    }

    /// The Printer box at the top left of the 3D view.
    pub(super) fn printer_box(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(330.0);
                ui.label(RichText::new("Printer").size(16.0).strong());
                ui.add_space(4.0);
                let mut chosen = None;
                egui::ComboBox::from_id_salt("printer")
                    .width(ui.available_width())
                    .selected_text(&self.printer.name)
                    .show_ui(ui, |ui| {
                        for (i, p) in self.printers.iter().enumerate() {
                            if ui
                                .selectable_label(p.name == self.printer.name, &p.name)
                                .clicked()
                            {
                                chosen = Some(i);
                            }
                        }
                    });
                if let Some(i) = chosen {
                    self.printer = self.printers[i].clone();
                    self.camera = Camera::for_plate(self.printer.bed_x, self.printer.bed_y);
                }
                settings_grid(ui, "printer_box_grid", |ui| {
                    ui.label("Plate");
                    ui.label(format!(
                        "{:.0} × {:.0} × {:.0} mm",
                        self.printer.bed_x, self.printer.bed_y, self.printer.max_z
                    ));
                    ui.end_row();
                    ui.label("Nozzle diameter");
                    egui::ComboBox::from_id_salt("nozzle")
                        .selected_text(format!("{:.1} mm", self.printer.nozzle_diameter))
                        .show_ui(ui, |ui| {
                            for n in NOZZLES {
                                let label = if (n - 0.4).abs() < 1e-3 {
                                    format!("{n:.1} mm (standard)")
                                } else {
                                    format!("{n:.1} mm")
                                };
                                ui.selectable_value(&mut self.printer.nozzle_diameter, n, label);
                            }
                        });
                    ui.end_row();
                });
                egui::CollapsingHeader::new("Edit plate, retraction and purge")
                    .id_salt("printer_more")
                    .show(ui, |ui| {
                        // Scrolls within the room left above the tool strip.
                        let room = (ui.available_height() - 24.0).max(80.0);
                        egui::ScrollArea::vertical()
                            .id_salt("printer_more_scroll")
                            .max_height(room)
                            .show(ui, |ui| {
                        settings_grid(ui, "printer_grid", |ui| {
                            row(
                                ui,
                                "Plate width",
                                egui::DragValue::new(&mut self.printer.bed_x)
                                    .range(limits::PLATE_MM)
                                    .suffix(" mm"),
                            );
                            row(
                                ui,
                                "Plate depth",
                                egui::DragValue::new(&mut self.printer.bed_y)
                                    .range(limits::PLATE_MM)
                                    .suffix(" mm"),
                            );
                            row(
                                ui,
                                "Max height",
                                egui::DragValue::new(&mut self.printer.max_z)
                                    .range(limits::PLATE_MM)
                                    .suffix(" mm"),
                            );
                            row(
                                ui,
                                "Retraction",
                                egui::DragValue::new(&mut self.printer.retract_length)
                                    .range(limits::RETRACT_MM)
                                    .speed(0.05)
                                    .suffix(" mm"),
                            );
                            ui.label("Runs Klipper firmware").on_hover_text(
                                "Klipper printers (like the Ender-3 V3 KE) get heat-up-only waits written Klipper's way",
                            );
                            ui.checkbox(&mut self.printer.klipper, "");
                            ui.end_row();
                            ui.label("MakerBot / Sailfish").on_hover_text(
                                "FlashForge Creator Pro, early Creator Pro 2, Replicator and clones: Export saves an X3G file for the printer's SD card",
                            );
                            ui.checkbox(&mut self.printer.x3g, "");
                            ui.end_row();
                            ui.label("Nozzles").on_hover_text(
                                "2 for printers with two nozzles on one carriage (Creator Pro, Replicator Dual): each part can have its own nozzle and filament",
                            );
                            ui.add(egui::DragValue::new(&mut self.printer.extruders).range(1..=2));
                            ui.end_row();
                            ui.label("Retract on travels over").on_hover_text(
                                "Travels longer than this pull the filament back first, so it doesn't ooze or string on the way",
                            );
                            ui.add(mm(&mut self.printer.retract_min_travel, limits::RETRACT_MIN_TRAVEL, 0.1));
                            ui.end_row();
                            ui.label("Retract on layer change").on_hover_text(
                                "Also pull the filament back before every move up to the next layer",
                            );
                            ui.checkbox(&mut self.printer.retract_layer_change, "");
                            ui.end_row();
                            ui.label("Wipe").on_hover_text(
                                "While retracting, move back this far along the line just printed, wiping the ooze off onto the part (0 = off)",
                            );
                            ui.add(mm(&mut self.printer.wipe_mm, limits::WIPE_MM, 0.1));
                            ui.end_row();
                            if self.printer.extruders >= 2 {
                                ui.label("Primary nozzle").on_hover_text(
                                    "The default nozzle: new parts, supports, the skirt and one-nozzle prints use it, and it has the main filament (01)",
                                );
                                let before = self.printer.primary_nozzle;
                                egui::ComboBox::from_id_salt("primary_nozzle")
                                    .selected_text(if self.printer.primary_nozzle == 0 { "Right (T0)" } else { "Left (T1)" })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut self.printer.primary_nozzle, 0, "Right (T0)");
                                        ui.selectable_value(&mut self.printer.primary_nozzle, 1, "Left (T1)");
                                    });
                                let now = self.printer.primary_nozzle;
                                if now != before {
                                    // Parts and supports on the old primary follow it to the
                                    // new one; those set to the other nozzle stay there.
                                    if self.dual.support_tool == before {
                                        self.dual.support_tool = now;
                                    }
                                    for o in self.objects.iter_mut().filter(|o| o.extruder == before) {
                                        o.extruder = now;
                                    }
                                }
                                ui.end_row();
                                ui.label("Nozzle spacing").on_hover_text(
                                    "The distance between the two nozzles along X (the extruder offset). A Ditto copy prints this far to the side, so a Ditto job must be narrower",
                                );
                                ui.add(mm(&mut self.printer.nozzle_spacing, limits::NOZZLE_SPACING, 0.1));
                                ui.end_row();
                                ui.label("Tool-change retraction").on_hover_text(
                                    "How far the nozzle being set aside is pulled back so it doesn't ooze while the other one prints; it is pushed forward by the same amount before it prints again",
                                );
                                ui.add(mm(&mut self.printer.tool_change_retract, limits::TOOL_CHANGE_RETRACT, 0.1));
                                ui.end_row();
                                ui.label("Standby drop").on_hover_text(
                                    "The idle nozzle waits this much below its printing temperature, which stops it oozing; it is heated back (a short wait) before it prints again. 0 keeps it hot",
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.printer.standby_drop)
                                        .range(limits::STANDBY_DROP)
                                        .suffix(" °C"),
                                );
                                ui.end_row();
                            }
                            ui.label("Purge amount")
                                .on_hover_text("Filament pushed through the two purge lines at the left edge before printing, to clear the nozzle");
                            ui.add(
                                egui::DragValue::new(&mut self.printer.purge_mm)
                                    .range(limits::PURGE_MM)
                                    .speed(0.5)
                                    .fixed_decimals(0)
                                    .suffix(" mm"),
                            );
                            ui.end_row();
                        });
                            });
                    });
            });
    }

    /// The row of model tools along the top of the 3D view.
    pub(super) fn model_toolbar(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let has = !self.objects.is_empty();
                    let sel = self.selected.is_some();
                    if ui
                        .button("Add model")
                        .on_hover_text("Open STL, OBJ or 3MF files")
                        .clicked()
                    {
                        self.open_dialog();
                    }
                    ui.menu_button("Add shape", |ui| {
                        for shape in SampleShape::ALL {
                            if ui.button(shape.label()).clicked() {
                                self.add_object(PlacedObject::new(shape.label(), shape.mesh()));
                                self.status = format!("Added a {}.", shape.label().to_lowercase());
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui
                        .add_enabled(has, egui::Button::new("Arrange"))
                        .on_hover_text("Lay the models out neatly")
                        .clicked()
                    {
                        self.arrange();
                        self.status = "Arranged the plate.".into();
                    }
                    if ui
                        .add_enabled(sel, egui::Button::new("Lay flat"))
                        .on_hover_text("Put the selected model's biggest flat face down")
                        .clicked()
                    {
                        self.apply_to_selected(ObjectAction::LayFlat);
                    }
                    for (label, action) in [
                        ("Turn 90°", ObjectAction::Turn90),
                        ("Duplicate", ObjectAction::Duplicate),
                        ("Delete", ObjectAction::Delete),
                    ] {
                        if ui.add_enabled(sel, egui::Button::new(label)).clicked() {
                            self.apply_to_selected(action);
                        }
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has, egui::Button::new("Clear plate"))
                        .clicked()
                    {
                        self.clear_plate();
                        self.status = "Cleared the plate.".into();
                    }
                });
            });
    }

    /// "Slice plate" and "Send print", stacked at the bottom right of the 3D view.
    pub(super) fn slice_buttons(&mut self, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let size = egui::vec2(220.0, 36.0);
            let slicing = self.job.is_some();
            let label = if slicing { "Slicing…" } else { "Slice plate" };
            let slice = egui::Button::new(
                RichText::new(label)
                    .size(15.0)
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(theme::ACCENT_DARK);
            let can_slice = !slicing && !self.objects.is_empty();
            if ui
                .add_enabled_ui(can_slice, |ui| ui.add_sized(size, slice))
                .inner
                .clicked()
            {
                self.start_slice();
            }
            let send = egui::Button::new(RichText::new("Send print").size(15.0).strong());
            let r = ui
                .add_enabled_ui(!self.objects.is_empty() && !slicing, |ui| {
                    ui.add_sized(size, send)
                })
                .inner;
            let r = if self.device.is_connected() {
                r.on_hover_text("Slice if needed, then send to the connected printer")
            } else {
                r.on_hover_text("Connect a printer on the Device page first")
            };
            if r.clicked() {
                self.send_print();
            }
        });
    }

    /// Slices first when needed, then asks to send to the connected printer.
    pub(super) fn send_print(&mut self) {
        if !self.device.is_connected() {
            self.mode = Mode::Printer;
            self.status = "Connect a printer first, then press Send print again.".into();
            return;
        }
        if let Some(why) = self
            .sliced
            .as_ref()
            .filter(|_| self.slice_is_current())
            .and_then(|s| s.problem.clone())
        {
            self.status = why;
            return;
        }
        match self.print_job() {
            Some(job) => {
                self.device.ask_to_print(job);
                self.mode = Mode::Printer;
            }
            None => {
                self.start_slice();
                // Send when the slice finishes; a refused slice must not leave the
                // request behind to surprise the next ordinary "Slice plate".
                self.send_after_slice = self.job.is_some();
            }
        }
    }

    /// The right-hand panel on the Prepare page: Filament, then Process.
    pub(super) fn right_panel(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.filament_section(ui);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Process").size(17.0).strong());
                ui.add_space(6.0);
                for (tab, label) in [
                    (ProcessTab::Global, "Global"),
                    (ProcessTab::Objects, "Objects"),
                ] {
                    if ui
                        .add(egui::Button::selectable(self.process_tab == tab, label))
                        .clicked()
                    {
                        self.process_tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.advanced, "Advanced")
                        .on_hover_text("Show every setting");
                });
            });
            ui.add_space(6.0);
            match self.process_tab {
                ProcessTab::Global => self.process_settings(ui),
                ProcessTab::Objects => self.objects_panel(ui),
            }
        });
    }

    fn filament_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Filament").size(17.0).strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // A colour chip like a filament slot: "01" and the material.
            egui::Frame::new()
                .fill(theme::ACCENT_DARK)
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("01").color(Color32::WHITE).small());
                        let mut chosen = None;
                        egui::ComboBox::from_id_salt("filament")
                            .selected_text(RichText::new(&self.filament.name).color(Color32::WHITE))
                            .show_ui(ui, |ui| {
                                for (i, f) in self.filaments.iter().enumerate() {
                                    if ui
                                        .selectable_label(f.name == self.filament.name, &f.name)
                                        .clicked()
                                    {
                                        chosen = Some(i);
                                    }
                                }
                            });
                        if let Some(i) = chosen {
                            self.filament = self.filaments[i].clone();
                        }
                    });
                });
            ui.vertical(|ui| {
                // On a two-nozzle printer, say which nozzle this filament is in.
                let which = if self.printer.extruders < 2 {
                    String::new()
                } else if self.dual.ditto {
                    "Both nozzles (Ditto) · ".to_string()
                } else {
                    let primary = self.printer.primary_tool();
                    format!("{} · ", crate::app::nozzle_name(primary, primary))
                };
                ui.label(
                    RichText::new(format!(
                        "{which}Nozzle {} °C · plate {} °C",
                        self.filament.nozzle_temp, self.filament.bed_temp
                    ))
                    .color(theme::TEXT_DIM),
                );
                ui.label(
                    RichText::new(format!(
                        "{:.2} mm · {:.2} g/cm³",
                        self.filament.diameter, self.filament.density
                    ))
                    .color(theme::TEXT_DIM),
                );
            });
        });
        if self.printer.extruders >= 2 && !self.dual.ditto {
            self.second_filament_row(ui);
        }
        egui::CollapsingHeader::new("Temperatures and cooling")
            .id_salt("filament_more")
            .show(ui, |ui| {
                settings_grid(ui, "filament_grid", |ui| {
                    row(
                        ui,
                        "Nozzle temp",
                        egui::DragValue::new(&mut self.filament.nozzle_temp)
                            .range(limits::NOZZLE_TEMP)
                            .suffix(" °C"),
                    );
                    row(
                        ui,
                        "Plate temp",
                        egui::DragValue::new(&mut self.filament.bed_temp)
                            .range(limits::BED_TEMP)
                            .suffix(" °C"),
                    );
                    row(
                        ui,
                        "Cooling fan",
                        egui::Slider::new(&mut self.filament.fan_percent, limits::FAN_PERCENT)
                            .suffix(" %"),
                    );
                });
            });
    }

    /// The second nozzle's filament: "Same as 01" or one of the filament presets.
    fn second_filament_row(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(theme::ACCENT_DARK)
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("02").color(Color32::WHITE).small());
                        let shown = self
                            .dual
                            .second_filament
                            .as_ref()
                            .map(|f| f.name.clone())
                            .unwrap_or_else(|| "Same as 01".into());
                        let mut chosen: Option<Option<usize>> = None;
                        egui::ComboBox::from_id_salt("filament2")
                            .selected_text(RichText::new(shown).color(Color32::WHITE))
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        self.dual.second_filament.is_none(),
                                        "Same as 01",
                                    )
                                    .clicked()
                                {
                                    chosen = Some(None);
                                }
                                for (i, f) in self.filaments.iter().enumerate() {
                                    let on = self
                                        .dual
                                        .second_filament
                                        .as_ref()
                                        .is_some_and(|s| s.name == f.name);
                                    if ui.selectable_label(on, &f.name).clicked() {
                                        chosen = Some(Some(i));
                                    }
                                }
                            });
                        if let Some(c) = chosen {
                            self.dual.second_filament = c.map(|i| self.filaments[i].clone());
                        }
                    });
                });
            let f = self.dual.second_filament.as_ref().unwrap_or(&self.filament);
            let primary = self.printer.primary_tool();
            ui.label(
                RichText::new(format!(
                    "{} · nozzle {} °C",
                    crate::app::nozzle_name(1 - primary, primary),
                    f.nozzle_temp
                ))
                .color(theme::TEXT_DIM),
            );
        });
    }

    /// Global process settings, grouped like the original's Process panel.
    fn process_settings(&mut self, ui: &mut Ui) {
        self.process_profile_picker(ui);
        ui.add_space(6.0);
        let before = self.settings.clone();
        let advanced = self.advanced;
        let s = &mut self.settings;

        layer_height_group(ui, s, advanced);
        seam_group(ui, s);
        walls_and_surfaces_group(ui, s, advanced);
        infill_group(ui, s);
        support_group(ui, s);
        if self.printer.extruders >= 2 {
            let primary = self.printer.primary_tool();
            two_nozzle_group(ui, &mut self.dual, self.settings.supports, primary);
        }
        let s = &mut self.settings;
        adhesion_group(ui, s);
        speed_group(ui, s, advanced);
        if advanced {
            cooling_and_travel_group(ui, s);
            special_mode_group(ui, s);
        } else {
            ui.label(
                RichText::new(
                    "Turn on Advanced for speeds, cooling, z-hop, fuzzy skin and spiral vase.",
                )
                .color(theme::TEXT_DIM)
                .small(),
            );
        }
        if self.settings != before {
            self.quality = Quality::ALL.into_iter().find(|q| q.matches(&self.settings));
        }
    }

    /// The profile: one of the quality presets, or "Custom" once edited.
    fn process_profile_picker(&mut self, ui: &mut Ui) {
        let shown = match self.quality {
            Some(q) => q.label().to_string(),
            None => "Custom".to_string(),
        };
        egui::ComboBox::from_id_salt("process_profile")
            .width(ui.available_width())
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for q in Quality::ALL {
                    if ui
                        .selectable_label(self.quality == Some(q), q.label())
                        .clicked()
                    {
                        q.apply(&mut self.settings);
                        self.quality = Some(q);
                    }
                }
            });
    }
}

/// Layer height, plus the first layer's height in Advanced.
fn layer_height_group(ui: &mut Ui, s: &mut PrintSettings, advanced: bool) {
    group(ui, "Layer height", |ui| {
        row(
            ui,
            "Layer height",
            mm(&mut s.layer_height, limits::LAYER_HEIGHT, 0.01),
        );
        if advanced {
            row(
                ui,
                "First layer height",
                mm(&mut s.first_layer_height, limits::FIRST_LAYER_HEIGHT, 0.01),
            );
        }
    });
}

/// Where each layer's outer wall starts and ends.
fn seam_group(ui: &mut Ui, s: &mut PrintSettings) {
    group(ui, "Seam", |ui| {
        ui.label("Seam position");
        egui::ComboBox::from_id_salt("seam")
            .selected_text(s.seam.label())
            .show_ui(ui, |ui| {
                for p in SeamPosition::ALL {
                    ui.selectable_value(&mut s.seam, p, p.label());
                }
            });
        ui.end_row();
    });
}

/// Wall and top/bottom layer counts, ironing, fuzzy top and (Advanced) fuzzy skin.
fn walls_and_surfaces_group(ui: &mut Ui, s: &mut PrintSettings, advanced: bool) {
    group(ui, "Walls and surfaces", |ui| {
        row(
            ui,
            "Walls",
            egui::DragValue::new(&mut s.walls).range(limits::WALLS),
        );
        row(
            ui,
            "Top layers",
            egui::DragValue::new(&mut s.top_layers).range(limits::SOLID_LAYERS),
        );
        row(
            ui,
            "Bottom layers",
            egui::DragValue::new(&mut s.bottom_layers).range(limits::SOLID_LAYERS),
        );
        row(
            ui,
            "Ironing (smooth tops)",
            egui::Checkbox::without_text(&mut s.ironing),
        );
        ui.label("Fuzzy top (textured top)").on_hover_text(
            "While laying the visible top surface, the nozzle randomly lifts by up to the amount below, for a textured, matte top. Ironing is skipped while this is on.",
        );
        ui.checkbox(&mut s.fuzzy_top, "");
        ui.end_row();
        if s.fuzzy_top {
            row(
                ui,
                "Fuzzy top amount",
                mm(&mut s.fuzzy_top_mm, limits::FUZZY_TOP_MM, 0.01),
            );
        }
        if advanced {
            row(
                ui,
                "Fuzzy skin",
                egui::Checkbox::without_text(&mut s.fuzzy_skin),
            );
        }
    });
}

/// Infill density and pattern.
fn infill_group(ui: &mut Ui, s: &mut PrintSettings) {
    group(ui, "Infill", |ui| {
        row(
            ui,
            "Density",
            egui::DragValue::new(&mut s.infill_percent)
                .range(limits::INFILL_PERCENT)
                .speed(1.0)
                .fixed_decimals(0)
                .suffix(" %"),
        );
        ui.label("Pattern");
        egui::ComboBox::from_id_salt("infill_pattern")
            .selected_text(s.infill_pattern.label())
            .show_ui(ui, |ui| {
                for p in InfillPattern::ALL {
                    ui.selectable_value(&mut s.infill_pattern, p, p.label());
                }
            });
        ui.end_row();
    });
}

/// The support switch, and its overhang angle and placement once it is on.
fn support_group(ui: &mut Ui, s: &mut PrintSettings) {
    group(ui, "Support", |ui| {
        row(
            ui,
            "Enable support",
            egui::Checkbox::without_text(&mut s.supports),
        );
        if s.supports {
            row(
                ui,
                "Overhang angle",
                egui::DragValue::new(&mut s.support_angle)
                    .range(limits::SUPPORT_ANGLE)
                    .speed(1.0)
                    .suffix("°"),
            );
            row(
                ui,
                "On build plate only",
                egui::Checkbox::without_text(&mut s.supports_on_plate_only),
            );
            // With a full raft the supports already stand on it.
            let full_raft = s.adhesion == Adhesion::Raft;
            let hint = if full_raft {
                "Adhesion is set to Raft, which already covers the supports"
            } else {
                "Supports that reach the bed stand on a small raft of their own, so they stick"
            };
            ui.label("Raft under support").on_hover_text(hint);
            ui.add_enabled(
                !full_raft,
                egui::Checkbox::without_text(&mut s.support_raft),
            )
            .on_hover_text(hint)
            .on_disabled_hover_text(hint);
            ui.end_row();
        }
    });
}

/// Skirt, brim or nothing around the first layer.
fn adhesion_group(ui: &mut Ui, s: &mut PrintSettings) {
    group(ui, "Adhesion", |ui| {
        ui.label("Type");
        egui::ComboBox::from_id_salt("adhesion")
            .selected_text(s.adhesion.label())
            .show_ui(ui, |ui| {
                for a in Adhesion::ALL {
                    ui.selectable_value(&mut s.adhesion, a, a.label());
                }
            });
        ui.end_row();
    });
}

/// Print speed, plus first layer and travel speeds in Advanced.
fn speed_group(ui: &mut Ui, s: &mut PrintSettings, advanced: bool) {
    group(ui, "Speed", |ui| {
        row(
            ui,
            "Print speed",
            egui::DragValue::new(&mut s.print_speed)
                .range(limits::PRINT_SPEED)
                .speed(1.0)
                .suffix(" mm/s"),
        );
        if advanced {
            row(
                ui,
                "First layer speed",
                egui::DragValue::new(&mut s.first_layer_speed)
                    .range(limits::FIRST_LAYER_SPEED)
                    .speed(1.0)
                    .suffix(" mm/s"),
            );
            row(
                ui,
                "Travel speed",
                egui::DragValue::new(&mut s.travel_speed)
                    .range(limits::TRAVEL_SPEED)
                    .speed(1.0)
                    .suffix(" mm/s"),
            );
        }
    });
}

/// Minimum layer time and z-hop (Advanced only).
fn cooling_and_travel_group(ui: &mut Ui, s: &mut PrintSettings) {
    group(ui, "Cooling and travel", |ui| {
        row(
            ui,
            "Minimum layer time",
            egui::DragValue::new(&mut s.min_layer_time)
                .range(limits::MIN_LAYER_TIME)
                .speed(0.5)
                .suffix(" s"),
        );
        row(ui, "Z-hop", mm(&mut s.z_hop, limits::Z_HOP, 0.05));
    });
}

/// Spiral vase mode (Advanced only).
fn special_mode_group(ui: &mut Ui, s: &mut PrintSettings) {
    group(ui, "Special mode", |ui| {
        row(
            ui,
            "Spiral vase",
            egui::Checkbox::without_text(&mut s.vase_mode),
        );
    });
}

/// The two-nozzle choices: which nozzle prints supports, and Ditto printing.
fn two_nozzle_group(ui: &mut Ui, d: &mut crate::profiles::DualNozzle, supports: bool, primary: u8) {
    group(ui, "Two nozzles", |ui| {
        if supports && !d.ditto {
            ui.label("Supports with");
            egui::ComboBox::from_id_salt("support_nozzle")
                .selected_text(crate::app::nozzle_name(d.support_tool, primary))
                .show_ui(ui, |ui| {
                    for t in [0u8, 1] {
                        ui.selectable_value(
                            &mut d.support_tool,
                            t,
                            crate::app::nozzle_name(t, primary),
                        );
                    }
                });
            ui.end_row();
        }
        if !d.ditto {
            ui.label("Prime tower").on_hover_text(
                "A small tower in the back-right corner: after each tool change the incoming nozzle primes on it, so ooze ends up there and not on the part",
            );
            ui.add(egui::Checkbox::without_text(&mut d.prime_tower));
            ui.end_row();
            if d.prime_tower {
                ui.label("Tower position").on_hover_text(
                    "Where the tower's centre stands, in mm from the plate's front-left corner. Automatic puts it near the back-right corner",
                );
                ui.horizontal(|ui| {
                    let mut custom = d.tower_at.is_some();
                    if ui.checkbox(&mut custom, "Set").changed() {
                        d.tower_at = custom.then_some([30.0, 30.0]);
                    }
                    match &mut d.tower_at {
                        Some([x, y]) => {
                            ui.add(
                                egui::DragValue::new(x)
                                    .prefix("X ")
                                    .suffix(" mm")
                                    .speed(0.5),
                            );
                            ui.add(
                                egui::DragValue::new(y)
                                    .prefix("Y ")
                                    .suffix(" mm")
                                    .speed(0.5),
                            );
                        }
                        None => {
                            ui.label("Back right (automatic)");
                        }
                    }
                });
                ui.end_row();
            }
        }
        if !d.ditto {
            ui.label("Ooze shield").on_hover_text(
                "A one-line wall around the print: the idle nozzle wipes its ooze on it when it crosses, instead of on the parts",
            );
            ui.horizontal(|ui| {
                ui.add(egui::Checkbox::without_text(&mut d.ooze_shield));
                if d.ooze_shield {
                    ui.add(
                        egui::DragValue::new(&mut d.shield_distance)
                            .range(crate::profiles::limits::SHIELD_DISTANCE)
                            .speed(0.1)
                            .suffix(" mm away"),
                    );
                }
            });
            ui.end_row();
        }
        ui.label("Ditto printing").on_hover_text(
            "The printer prints a copy of the plate with its second nozzle, one nozzle spacing to the side. Switch Ditto on in the printer's own menu too; the app heats both nozzles and keeps the plate narrow enough for the copy.",
        );
        ui.add(egui::Checkbox::without_text(&mut d.ditto));
        ui.end_row();
    });
}

/// A titled group with an accent bar, then label / value rows.
fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let (r, p) = ui.allocate_painter(egui::vec2(4.0, 18.0), Sense::hover());
        p.rect_filled(r.rect, 2.0, theme::ACCENT);
        ui.label(RichText::new(title).size(15.0).strong());
    });
    ui.indent(title, |ui| {
        egui::Grid::new(("group", title))
            .num_columns(2)
            .min_col_width(170.0)
            .spacing([16.0, 6.0])
            .show(ui, add);
    });
    ui.add_space(6.0);
}

fn mm(v: &mut f32, range: std::ops::RangeInclusive<f32>, speed: f64) -> egui::DragValue<'_> {
    egui::DragValue::new(v)
        .range(range)
        .speed(speed)
        .fixed_decimals(2)
        .suffix(" mm")
}
