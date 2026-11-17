//! Small layout pieces shared by the Prepare and Printer screens.

use crate::theme;
use eframe::egui::{self, RichText, Ui};

/// A titled panel with the app's card background.
pub fn card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
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

/// A two-column label / control grid.
pub fn settings_grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, add);
}

/// One label / control row of a `settings_grid`.
pub fn row(ui: &mut Ui, label: &str, widget: impl egui::Widget) {
    ui.label(label);
    ui.add(widget);
    ui.end_row();
}
