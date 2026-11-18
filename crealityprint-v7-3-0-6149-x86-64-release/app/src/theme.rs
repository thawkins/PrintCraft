//! Colours and the overall look of the app (a calm dark theme with a teal accent).

use crate::slicer::PathKind;
use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(38, 186, 150);
pub const ACCENT_DARK: Color32 = Color32::from_rgb(24, 128, 104);
pub const BG: Color32 = Color32::from_rgb(30, 32, 36);
pub const PANEL: Color32 = Color32::from_rgb(40, 43, 48);
pub const CARD: Color32 = Color32::from_rgb(50, 54, 60);
pub const VIEW_TOP: Color32 = Color32::from_rgb(58, 63, 72);
pub const VIEW_BOTTOM: Color32 = Color32::from_rgb(26, 28, 32);
pub const PLATE: Color32 = Color32::from_rgb(66, 70, 78);
pub const PLATE_GRID: Color32 = Color32::from_rgb(84, 89, 98);
pub const PLATE_GRID_MAJOR: Color32 = Color32::from_rgb(104, 110, 120);
pub const PLATE_EDGE: Color32 = Color32::from_rgb(140, 148, 160);
pub const VOLUME_EDGE: Color32 = Color32::from_rgba_premultiplied(70, 80, 90, 120);
pub const TEXT: Color32 = Color32::from_rgb(226, 230, 236);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 156, 166);
pub const OBJECT: Color32 = Color32::from_rgb(200, 206, 214);
pub const OBJECT_SELECTED: Color32 = Color32::from_rgb(90, 220, 186);
pub const OBJECT_OUTSIDE: Color32 = Color32::from_rgb(232, 120, 80);
pub const WARNING: Color32 = Color32::from_rgb(240, 170, 70);

pub fn path_color(kind: PathKind) -> Color32 {
    match kind {
        PathKind::OuterWall => Color32::from_rgb(255, 146, 52),
        PathKind::InnerWall => Color32::from_rgb(244, 214, 70),
        PathKind::SolidInfill => Color32::from_rgb(178, 102, 230),
        PathKind::SparseInfill => Color32::from_rgb(226, 78, 84),
        PathKind::Support => Color32::from_rgb(70, 196, 220),
        PathKind::Ironing => Color32::from_rgb(120, 150, 255),
        PathKind::TopSurface => Color32::from_rgb(236, 120, 196),
        PathKind::Bridge => Color32::from_rgb(40, 110, 220),
        PathKind::Skirt | PathKind::Brim => Color32::from_rgb(140, 214, 140),
        PathKind::Raft => Color32::from_rgb(160, 160, 170),
        PathKind::PrimeTower => Color32::from_rgb(120, 200, 170),
        PathKind::OozeShield => Color32::from_rgb(150, 170, 200),
    }
}

/// The preview colour of a nozzle when colouring by nozzle: T0 blue, T1 orange.
pub fn nozzle_color(tool: u8) -> Color32 {
    if tool == 0 {
        Color32::from_rgb(70, 150, 245)
    } else {
        Color32::from_rgb(255, 120, 60)
    }
}

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT_DARK;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    // Gently rounded, so tick boxes still read as boxes.
    let r = CornerRadius::same(4);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(64, 69, 78);
    v.widgets.inactive.bg_fill = Color32::from_rgb(70, 76, 86);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(82, 88, 98));
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(82, 88, 98);
    v.widgets.hovered.bg_fill = Color32::from_rgb(86, 93, 104);
    v.widgets.active.weak_bg_fill = ACCENT_DARK;
    v.slider_trailing_fill = true;
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.slider_width = 104.0;
    });
}
