//! The print settings panel, drawn from the settings registry: one section per
//! `settings_registry::Group`, one row per registry entry, each with the widget its
//! `Kind` asks for. Adding a print setting to the registry adds its row here.

use super::*;
use crate::modifier::{Modifier, ModifierKind, ModifierShape, PER_MODIFIER};
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

/// The object list's badge for a part with `settings` settings of its own, `ranges`
/// height ranges and `modifiers` modifiers.
pub(super) fn override_badge(settings: usize, ranges: usize, modifiers: usize) -> Option<String> {
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    let parts: Vec<String> = [
        count(settings, "setting", "settings"),
        count(ranges, "range", "ranges"),
        count(modifiers, "modifier", "modifiers"),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
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

/// A change made in an overrides editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum OverrideEdit {
    /// A new value for a setting already overridden.
    Set(&'static str, Value),
    /// Back to the value underneath.
    Remove(&'static str),
    /// Override one more setting, starting at the value underneath.
    Add(&'static str),
}

/// Applies `edit` to `overrides`; `under` is what they sit on (the plate's settings for
/// a part, the part's own for a height range), where an added setting starts. A setting
/// already overridden keeps its value when added again.
pub(super) fn apply_override_edit(
    overrides: &mut overrides::Overrides,
    edit: OverrideEdit,
    under: &PrintSettings,
) {
    match edit {
        OverrideEdit::Set(key, v) => {
            overrides.set(key, v);
        }
        OverrideEdit::Remove(key) => overrides.remove(key),
        OverrideEdit::Add(key) => {
            if overrides.get(key).is_none() {
                if let Some(r) = overrides::per_object_settings().find(|r| r.key == key) {
                    overrides.set(key, (r.get)(under));
                }
            }
        }
    }
}

/// The rows of `overrides`, by section, each with a button back to the value in
/// `under`, then a list to add another of the `allowed` settings. `id` keeps two
/// editors' widgets apart.
fn overrides_editor(
    ui: &mut Ui,
    id: &str,
    overrides: &overrides::Overrides,
    allowed: &[&str],
    under: &PrintSettings,
    under_name: &str,
) -> Option<OverrideEdit> {
    let mut edit = None;
    let effective = overrides.apply(under);
    let (own, others): (Vec<&Setting>, Vec<&Setting>) = overrides::per_object_settings()
        .filter(|r| allowed.contains(&r.key))
        .partition(|r| overrides.get(r.key).is_some());
    if !own.is_empty() {
        egui::Grid::new(("overrides", id))
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
                    let widget_id = format!("{id}_{}", r.key);
                    if ui.add(value_widget(&r.kind, &mut v, &widget_id)).changed() {
                        edit = Some(OverrideEdit::Set(r.key, v));
                    }
                    let back = value_text(&r.kind, (r.get)(under));
                    if ui
                        .small_button("x")
                        .on_hover_text(format!("Use {under_name} value ({back})"))
                        .clicked()
                    {
                        edit = Some(OverrideEdit::Remove(r.key));
                    }
                    ui.end_row();
                }
            });
    }
    if !others.is_empty() {
        egui::ComboBox::from_id_salt(("add_setting", id))
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
                        edit = Some(OverrideEdit::Add(r.key));
                    }
                }
            });
    }
    edit
}

/// A change to a part's height ranges.
enum RangeEdit {
    Add,
    Remove(usize),
    Heights(usize, f32, f32),
    Settings(usize, OverrideEdit),
}

impl PrintCraftApp {
    /// Adds a height range to object `i` over the top quarter of its height (`height`
    /// mm tall), with no settings yet.
    pub(super) fn add_height_range(&mut self, i: usize, height: f32) {
        if let Some(o) = self.objects.get_mut(i) {
            let from = (height * 0.75 * 10.0).round() / 10.0;
            o.ranges.push(overrides::HeightRange {
                from,
                to: height.max(from + 0.1),
                overrides: Default::default(),
            });
        }
    }

    /// "Settings for this object": the settings the selected part has of its own, then
    /// its height ranges, each with settings of their own.
    pub(super) fn object_settings_card(&mut self, ui: &mut Ui, i: usize) {
        let height = self.object_bounds(i).size().z;
        let plate = &self.settings;
        let o = &self.objects[i];
        let mut own_edit = None;
        let mut range_edit = None;
        let mut modifier_edit = None;
        let size = self.object_bounds(i).size();
        card(ui, "Settings for this object", |ui| {
            if o.overrides.is_empty() {
                ui.label(
                    RichText::new(
                        "Prints with the plate's settings. Add a setting to print this part \
                         differently.",
                    )
                    .color(theme::TEXT_DIM),
                );
            }
            own_edit = overrides_editor(
                ui,
                "object",
                &o.overrides,
                &overrides::PER_OBJECT,
                plate,
                "the plate's",
            );
            ui.add_space(8.0);
            ui.label(RichText::new("Height ranges").strong());
            let part = o.overrides.apply(plate);
            for (j, r) in o.ranges.iter().enumerate() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (mut from, mut to) = (r.from, r.to);
                    ui.label("From");
                    let a = ui.add(
                        egui::DragValue::new(&mut from)
                            .range(0.0..=(to - 0.1).max(0.0))
                            .speed(0.1)
                            .suffix(" mm"),
                    );
                    ui.label("to");
                    let b = ui.add(
                        egui::DragValue::new(&mut to)
                            .range((from + 0.1)..=10_000.0)
                            .speed(0.1)
                            .suffix(" mm"),
                    );
                    if a.changed() || b.changed() {
                        range_edit = Some(RangeEdit::Heights(j, from, to));
                    }
                    if ui
                        .small_button("Remove")
                        .on_hover_text("Remove this height range")
                        .clicked()
                    {
                        range_edit = Some(RangeEdit::Remove(j));
                    }
                });
                let id = format!("range{j}");
                if let Some(e) = overrides_editor(
                    ui,
                    &id,
                    &r.overrides,
                    &overrides::PER_OBJECT,
                    &part,
                    "the part's",
                ) {
                    range_edit = Some(RangeEdit::Settings(j, e));
                }
            }
            if ui
                .button("Add height range")
                .on_hover_text("Different settings for part of this part's height")
                .clicked()
            {
                range_edit = Some(RangeEdit::Add);
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Modifiers").strong());
            for (j, m) in o.modifiers.iter().enumerate() {
                ui.add_space(4.0);
                if let Some(e) = modifier_rows(ui, j, m, &part) {
                    modifier_edit = Some(e);
                }
            }
            egui::ComboBox::from_id_salt("add_modifier")
                .selected_text("Add modifier...")
                .show_ui(ui, |ui| {
                    for kind in ModifierKind::ALL {
                        if ui.selectable_label(false, kind.label()).clicked() {
                            modifier_edit = Some(ModifierEdit::Add(kind));
                        }
                    }
                })
                .response
                .on_hover_text(
                    "A volume inside which this part's infill differs, or that stops or \
                     forces supports",
                );
        });
        let part = self.objects[i].overrides.apply(&self.settings);
        if let Some(e) = own_edit {
            apply_override_edit(&mut self.objects[i].overrides, e, &self.settings);
        }
        match range_edit {
            Some(RangeEdit::Add) => self.add_height_range(i, height),
            Some(RangeEdit::Remove(j)) => {
                self.objects[i].ranges.remove(j);
            }
            Some(RangeEdit::Heights(j, from, to)) => {
                let r = &mut self.objects[i].ranges[j];
                (r.from, r.to) = (from, to);
            }
            Some(RangeEdit::Settings(j, e)) => {
                apply_override_edit(&mut self.objects[i].ranges[j].overrides, e, &part);
            }
            None => {}
        }
        let mods = &mut self.objects[i].modifiers;
        match modifier_edit {
            Some(ModifierEdit::Add(kind)) => {
                mods.push(Modifier::around_middle(kind, ModifierShape::Box, size));
            }
            Some(ModifierEdit::Remove(j)) => {
                mods.remove(j);
            }
            Some(ModifierEdit::Shape(j, shape)) => mods[j].shape = shape,
            Some(ModifierEdit::Place(j, size, at)) => (mods[j].size, mods[j].at) = (size, at),
            Some(ModifierEdit::Settings(j, e)) => {
                apply_override_edit(&mut mods[j].overrides, e, &part);
            }
            None => {}
        }
    }
}

/// A change to a part's modifiers.
enum ModifierEdit {
    Add(ModifierKind),
    Remove(usize),
    Shape(usize, ModifierShape),
    Place(usize, Vec3, Vec3),
    Settings(usize, OverrideEdit),
}

/// Modifier `j`'s rows: its shape and a Remove button, its size and place, and the
/// infill settings that differ inside it (on top of `part`, the part's own settings).
fn modifier_rows(
    ui: &mut Ui,
    j: usize,
    m: &Modifier,
    part: &PrintSettings,
) -> Option<ModifierEdit> {
    let mut edit = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(m.kind.label()).color(modifier_colour(m.kind)));
        let mut shape = m.shape;
        egui::ComboBox::from_id_salt(("modifier_shape", j))
            .selected_text(shape.label())
            .show_ui(ui, |ui| {
                for s in ModifierShape::ALL {
                    ui.selectable_value(&mut shape, s, s.label());
                }
            });
        if shape != m.shape {
            edit = Some(ModifierEdit::Shape(j, shape));
        }
        if ui
            .small_button("Remove")
            .on_hover_text("Remove this modifier")
            .clicked()
        {
            edit = Some(ModifierEdit::Remove(j));
        }
    });
    let (mut size, mut at) = (m.size, m.at);
    let mut moved = false;
    egui::Grid::new(("modifier_place", j))
        .num_columns(4)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            let mut field = |ui: &mut Ui, label: &str, v: &mut f32, min: f32| {
                ui.label(label);
                moved |= ui
                    .add(
                        egui::DragValue::new(v)
                            .range(min..=10_000.0)
                            .speed(0.1)
                            .max_decimals(2)
                            .suffix(" mm"),
                    )
                    .changed();
            };
            field(ui, "Width", &mut size.x, 0.1);
            field(ui, "Move X", &mut at.x, -10_000.0);
            ui.end_row();
            field(ui, "Depth", &mut size.y, 0.1);
            field(ui, "Move Y", &mut at.y, -10_000.0);
            ui.end_row();
            field(ui, "Height", &mut size.z, 0.1);
            field(ui, "Bottom at", &mut at.z, 0.0);
            ui.end_row();
        })
        .response
        .on_hover_text("Move X and Y are from the middle of the part");
    if moved {
        edit = Some(ModifierEdit::Place(j, size, at));
    }
    match m.kind {
        ModifierKind::Infill => {
            let id = format!("modifier{j}");
            if let Some(e) =
                overrides_editor(ui, &id, &m.overrides, &PER_MODIFIER, part, "the part's")
            {
                edit = Some(ModifierEdit::Settings(j, e));
            }
        }
        ModifierKind::SupportBlocker => {
            ui.label(RichText::new("No supports for overhangs inside it.").color(theme::TEXT_DIM));
        }
        ModifierKind::SupportEnforcer => {
            ui.label(
                RichText::new(
                    "Supports under every surface facing down inside it, even with supports \
                     off.",
                )
                .color(theme::TEXT_DIM),
            );
        }
    }
    edit
}

/// The colour a modifier of `kind` is drawn in (see-through in the 3D view).
pub(super) fn modifier_colour(kind: ModifierKind) -> Color32 {
    match kind {
        ModifierKind::Infill => theme::MODIFIER,
        ModifierKind::SupportBlocker => theme::SUPPORT_BLOCKER,
        ModifierKind::SupportEnforcer => theme::SUPPORT_ENFORCER,
    }
}
