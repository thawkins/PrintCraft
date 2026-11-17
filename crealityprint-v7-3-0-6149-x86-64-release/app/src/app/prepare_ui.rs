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
                let r = ui.add_enabled(can_export, egui::Button::new("Export G-code"));
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
                        settings_grid(ui, "printer_grid", |ui| {
                            row(
                                ui,
                                "Plate width",
                                egui::DragValue::new(&mut self.printer.bed_x)
                                    .range(50.0..=1000.0)
                                    .suffix(" mm"),
                            );
                            row(
                                ui,
                                "Plate depth",
                                egui::DragValue::new(&mut self.printer.bed_y)
                                    .range(50.0..=1000.0)
                                    .suffix(" mm"),
                            );
                            row(
                                ui,
                                "Max height",
                                egui::DragValue::new(&mut self.printer.max_z)
                                    .range(50.0..=1000.0)
                                    .suffix(" mm"),
                            );
                            row(
                                ui,
                                "Retraction",
                                egui::DragValue::new(&mut self.printer.retract_length)
                                    .range(0.0..=10.0)
                                    .speed(0.05)
                                    .suffix(" mm"),
                            );
                            ui.label("Runs Klipper firmware").on_hover_text(
                                "Klipper printers (like the Ender-3 V3 KE) get heat-up-only waits written Klipper's way",
                            );
                            ui.checkbox(&mut self.printer.klipper, "");
                            ui.end_row();
                            ui.label("Purge amount")
                                .on_hover_text("Filament pushed through the two purge lines at the left edge before printing, to clear the nozzle");
                            ui.add(
                                egui::DragValue::new(&mut self.printer.purge_mm)
                                    .range(0.0..=100.0)
                                    .speed(0.5)
                                    .fixed_decimals(0)
                                    .suffix(" mm"),
                            );
                            ui.end_row();
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
                        if let Some(i) = self.selected {
                            self.objects[i].lay_flat();
                            self.refresh_object(i);
                        }
                    }
                    if ui.add_enabled(sel, egui::Button::new("Turn 90°")).clicked() {
                        if let Some(i) = self.selected {
                            self.objects[i].rotation.z =
                                (self.objects[i].rotation.z + 90.0) % 360.0;
                            self.refresh_object(i);
                        }
                    }
                    if ui
                        .add_enabled(sel, egui::Button::new("Duplicate"))
                        .clicked()
                    {
                        self.duplicate_selected();
                    }
                    if ui.add_enabled(sel, egui::Button::new("Delete")).clicked() {
                        self.delete_selected();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has, egui::Button::new("Clear plate"))
                        .clicked()
                    {
                        self.objects.clear();
                        self.plate.clear();
                        self.selected = None;
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
        match self.print_job() {
            Some(job) => {
                self.device.ask_to_print(job);
                self.mode = Mode::Printer;
            }
            None => {
                self.send_after_slice = true;
                self.start_slice();
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
                ui.label(
                    RichText::new(format!(
                        "Nozzle {} °C · plate {} °C",
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
        egui::CollapsingHeader::new("Temperatures and cooling")
            .id_salt("filament_more")
            .show(ui, |ui| {
                settings_grid(ui, "filament_grid", |ui| {
                    row(
                        ui,
                        "Nozzle temp",
                        egui::DragValue::new(&mut self.filament.nozzle_temp)
                            .range(150..=300)
                            .suffix(" °C"),
                    );
                    row(
                        ui,
                        "Plate temp",
                        egui::DragValue::new(&mut self.filament.bed_temp)
                            .range(0..=120)
                            .suffix(" °C"),
                    );
                    row(
                        ui,
                        "Cooling fan",
                        egui::Slider::new(&mut self.filament.fan_percent, 0..=100).suffix(" %"),
                    );
                });
            });
    }

    /// Global process settings, grouped like the original's Process panel.
    fn process_settings(&mut self, ui: &mut Ui) {
        // The profile: one of the quality presets, or "Custom" once edited.
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
        ui.add_space(6.0);
        let before = self.settings.clone();
        let advanced = self.advanced;
        let s = &mut self.settings;

        group(ui, "Layer height", |ui| {
            row(
                ui,
                "Layer height",
                mm(&mut s.layer_height, 0.04..=0.4, 0.01),
            );
            if advanced {
                row(
                    ui,
                    "First layer height",
                    mm(&mut s.first_layer_height, 0.08..=0.4, 0.01),
                );
            }
        });
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
        group(ui, "Walls and surfaces", |ui| {
            row(
                ui,
                "Walls",
                egui::DragValue::new(&mut s.walls).range(1..=10),
            );
            row(
                ui,
                "Top layers",
                egui::DragValue::new(&mut s.top_layers).range(0..=20),
            );
            row(
                ui,
                "Bottom layers",
                egui::DragValue::new(&mut s.bottom_layers).range(0..=20),
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
                    mm(&mut s.fuzzy_top_mm, 0.02..=0.3, 0.01),
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
        group(ui, "Infill", |ui| {
            row(
                ui,
                "Density",
                egui::DragValue::new(&mut s.infill_percent)
                    .range(0.0..=100.0)
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
                        .range(30.0..=80.0)
                        .speed(1.0)
                        .suffix("°"),
                );
                row(
                    ui,
                    "On build plate only",
                    egui::Checkbox::without_text(&mut s.supports_on_plate_only),
                );
            }
        });
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
        group(ui, "Speed", |ui| {
            row(
                ui,
                "Print speed",
                egui::DragValue::new(&mut s.print_speed)
                    .range(10.0..=500.0)
                    .speed(1.0)
                    .suffix(" mm/s"),
            );
            if advanced {
                row(
                    ui,
                    "First layer speed",
                    egui::DragValue::new(&mut s.first_layer_speed)
                        .range(5.0..=100.0)
                        .speed(1.0)
                        .suffix(" mm/s"),
                );
                row(
                    ui,
                    "Travel speed",
                    egui::DragValue::new(&mut s.travel_speed)
                        .range(20.0..=600.0)
                        .speed(1.0)
                        .suffix(" mm/s"),
                );
            }
        });
        if advanced {
            group(ui, "Cooling and travel", |ui| {
                row(
                    ui,
                    "Minimum layer time",
                    egui::DragValue::new(&mut s.min_layer_time)
                        .range(0.0..=120.0)
                        .speed(0.5)
                        .suffix(" s"),
                );
                row(ui, "Z-hop", mm(&mut s.z_hop, 0.0..=2.0, 0.05));
            });
            group(ui, "Special mode", |ui| {
                row(
                    ui,
                    "Spiral vase",
                    egui::Checkbox::without_text(&mut s.vase_mode),
                );
            });
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
