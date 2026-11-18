//! The print settings panel, drawn from the settings registry: one section per
//! `settings_registry::Group`, one row per registry entry, each with the widget its
//! `Kind` asks for. Adding a print setting to the registry adds its row here.

use super::*;
use crate::overrides;
use crate::settings_registry::{self, AutoContext, Group, Kind, Level, Setting, Value};

/// One section of the settings panel: a heading with an accent bar and a two-column
/// grid of rows.
pub(super) fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
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

/// The section for `g`: its rows up to `level` (Advanced shows Simple rows too) that
/// show with these settings. A section with no such rows (Cooling and travel in the
/// simple panel) is left out.
pub(super) fn settings_group(
    ui: &mut Ui,
    s: &mut PrintSettings,
    g: Group,
    level: Level,
    ctx: &AutoContext,
) {
    let visible = |r: &Setting, s: &PrintSettings| r.group == g && r.level <= level && (r.shown)(s);
    if !settings_registry::all().iter().any(|r| visible(r, s)) {
        return;
    }
    group(ui, g.title(), |ui| {
        // Checked row by row, so a row a checkbox above it reveals appears at once.
        for r in settings_registry::all() {
            if visible(r, s) {
                setting_row(ui, s, r, ctx);
            }
        }
    });
}

/// One label / widget row. The label's hint also shows on the widget (so a disabled
/// widget can say why). A change is stored through `Setting::write`, so it is in range.
fn setting_row(ui: &mut Ui, s: &mut PrintSettings, r: &Setting, ctx: &AutoContext) {
    let hint = (r.hint)(s);
    let label = ui.label(r.label);
    if let Some(h) = hint {
        label.on_hover_text(h);
    }
    let enabled = (r.enabled)(s);
    let changed = match (&r.kind, (r.get)(s)) {
        (Kind::Auto { .. }, Value::Auto(set)) => {
            ui.add_enabled_ui(enabled, |ui| auto_widget(ui, s, r, set, ctx))
                .inner
        }
        (kind, value) => {
            let mut value = value;
            let response = ui.add_enabled(enabled, value_widget(kind, &mut value, r.key));
            let response = match hint {
                Some(h) => response.on_hover_text(h).on_disabled_hover_text(h),
                None => response,
            };
            response.changed().then_some(value)
        }
    };
    if let Some(v) = changed {
        r.write(s, v);
    }
    ui.end_row();
}

/// The widget editing `value` in place, for every kind but Auto (`auto_widget`).
fn value_widget<'a>(kind: &'a Kind, value: &'a mut Value, key: &'a str) -> impl egui::Widget + 'a {
    move |ui: &mut Ui| match (kind, value) {
        (
            Kind::Number {
                range,
                step,
                suffix,
                decimals,
            },
            Value::Num(x),
        ) => {
            let drag = egui::DragValue::new(x)
                .range(range.clone())
                .speed(*step)
                .suffix(*suffix);
            match decimals {
                Some(d) => ui.add(drag.fixed_decimals(*d)),
                None => ui.add(drag),
            }
        }
        (Kind::Count { range, suffix }, Value::Count(n)) => {
            ui.add(egui::DragValue::new(n).range(range.clone()).suffix(*suffix))
        }
        (Kind::Flag, Value::Flag(b)) => ui.add(egui::Checkbox::without_text(b)),
        (Kind::Choice { labels }, Value::Choice(c)) => choice_widget(ui, key, labels(), c),
        // The registry gives each kind its own value; a mismatch shows nothing.
        (
            Kind::Number { .. }
            | Kind::Count { .. }
            | Kind::Flag
            | Kind::Choice { .. }
            | Kind::Auto { .. },
            Value::Num(_) | Value::Count(_) | Value::Flag(_) | Value::Choice(_) | Value::Auto(_),
        ) => ui.label(""),
    }
}

/// A drop-down of `labels` with `current` selected; a pick marks the response changed.
fn choice_widget(
    ui: &mut Ui,
    id: &str,
    labels: Vec<&'static str>,
    current: &mut &'static str,
) -> egui::Response {
    let mut picked = false;
    let mut response = egui::ComboBox::from_id_salt(id)
        .selected_text(*current)
        .show_ui(ui, |ui| {
            for l in labels {
                if ui.selectable_label(*current == l, l).clicked() && *current != l {
                    *current = l;
                    picked = true;
                }
            }
        })
        .response;
    if picked {
        response.mark_changed();
    }
    response
}

/// An Auto setting: an "Auto" checkbox, then the value it follows (greyed) or a field
/// for a value of its own. Unticking Auto starts from the Auto value. Returns the new
/// value when it changed.
fn auto_widget(
    ui: &mut Ui,
    s: &PrintSettings,
    r: &Setting,
    set: Option<f32>,
    ctx: &AutoContext,
) -> Option<Value> {
    let Kind::Auto {
        range,
        step,
        suffix,
        auto,
        auto_hint,
    } = &r.kind
    else {
        return None;
    };
    let auto_value = auto(s, ctx);
    let mut changed = None;
    ui.horizontal(|ui| {
        let mut is_auto = set.is_none();
        if ui
            .checkbox(&mut is_auto, "Auto")
            .on_hover_text(*auto_hint)
            .changed()
        {
            changed = Some(Value::Auto((!is_auto).then_some(auto_value)));
        }
        match set {
            Some(mut v) if !is_auto => {
                let drag = egui::DragValue::new(&mut v)
                    .range(range.clone())
                    .speed(*step)
                    .suffix(*suffix);
                if ui.add(drag).changed() {
                    changed = Some(Value::Auto(Some(v)));
                }
            }
            Some(_) | None => {
                ui.label(RichText::new(format!("{auto_value:.0}{suffix}")).color(theme::TEXT_DIM));
            }
        }
    });
    changed
}

/// The object list's badge for a part with `n` settings of its own.
pub(super) fn override_badge(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("1 setting".into()),
        n => Some(format!("{n} settings")),
    }
}

/// A value as the panel words it, for "the plate's value" hints.
fn value_text(kind: &Kind, v: Value) -> String {
    let suffix = match kind {
        Kind::Number { suffix, .. } | Kind::Count { suffix, .. } | Kind::Auto { suffix, .. } => {
            *suffix
        }
        Kind::Flag | Kind::Choice { .. } => "",
    };
    match v {
        Value::Num(x) | Value::Auto(Some(x)) => {
            format!(
                "{}{suffix}",
                crate::gcode_template::format_number(f64::from(x))
            )
        }
        Value::Count(n) => format!("{n}{suffix}"),
        Value::Flag(b) => if b { "on" } else { "off" }.into(),
        Value::Choice(c) => c.into(),
        Value::Auto(None) => "Auto".into(),
    }
}

impl PrintCraftApp {
    /// Gives object `i` setting `key` of its own, starting at the plate's value (so
    /// nothing prints differently until it is changed). A setting it already has keeps
    /// its value; one a part can't have is not added.
    pub(super) fn add_override(&mut self, i: usize, key: &str) {
        let Some(o) = self.objects.get_mut(i) else {
            return;
        };
        if o.overrides.get(key).is_some() {
            return;
        }
        if let Some(r) = overrides::per_object_settings().find(|r| r.key == key) {
            o.overrides.set(key, (r.get)(&self.settings));
        }
    }

    /// Object `i` goes back to the plate's value of `key`.
    pub(super) fn remove_override(&mut self, i: usize, key: &str) {
        if let Some(o) = self.objects.get_mut(i) {
            o.overrides.remove(key);
        }
    }

    /// "Settings for this object": each setting the selected part has of its own, with
    /// a button back to the plate's value, and a list to add another.
    pub(super) fn object_settings_card(&mut self, ui: &mut Ui, i: usize) {
        let mut edit: Option<(&'static str, Value)> = None;
        let mut remove: Option<&'static str> = None;
        let mut add: Option<&'static str> = None;
        let plate = &self.settings;
        let overrides = &self.objects[i].overrides;
        card(ui, "Settings for this object", |ui| {
            let effective = overrides.apply(plate);
            let (own, others): (Vec<&Setting>, Vec<&Setting>) =
                overrides::per_object_settings().partition(|r| overrides.get(r.key).is_some());
            if own.is_empty() {
                ui.label(
                    RichText::new(
                        "Prints with the plate's settings. Add a setting to print this part \
                         differently.",
                    )
                    .color(theme::TEXT_DIM),
                );
            } else {
                egui::Grid::new("object_settings")
                    .num_columns(3)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        let mut section = None;
                        for r in own {
                            // The section's name over its rows, as on the Global tab.
                            if section != Some(r.group) {
                                section = Some(r.group);
                                ui.label(RichText::new(r.group.title()).color(theme::TEXT_DIM));
                                ui.end_row();
                            }
                            ui.label(r.label);
                            let mut v = (r.get)(&effective);
                            let id = format!("object_{}", r.key);
                            if ui.add(value_widget(&r.kind, &mut v, &id)).changed() {
                                edit = Some((r.key, v));
                            }
                            let plate_value = value_text(&r.kind, (r.get)(plate));
                            if ui
                                .small_button("x")
                                .on_hover_text(format!("Use the plate's value ({plate_value})"))
                                .clicked()
                            {
                                remove = Some(r.key);
                            }
                            ui.end_row();
                        }
                    });
            }
            if !others.is_empty() {
                egui::ComboBox::from_id_salt("add_object_setting")
                    .selected_text("Add a setting...")
                    .show_ui(ui, |ui| {
                        let mut section = None;
                        for r in others {
                            if section != Some(r.group) {
                                section = Some(r.group);
                                ui.label(RichText::new(r.group.title()).color(theme::TEXT_DIM));
                            }
                            if ui
                                .selectable_label(false, format!("  {}", r.label))
                                .clicked()
                            {
                                add = Some(r.key);
                            }
                        }
                    });
            }
        });
        if let Some((key, v)) = edit {
            self.objects[i].overrides.set(key, v);
        }
        if let Some(key) = remove {
            self.remove_override(i, key);
        }
        if let Some(key) = add {
            self.add_override(i, key);
        }
    }
}
