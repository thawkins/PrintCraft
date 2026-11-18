//! The print settings registry: one table describing every value in `PrintSettings`.
//!
//! Each entry says how a setting is named in a project file (`key`, with `.` for a
//! nested object), how it is shown (label, hint, group, simple or Advanced, widget kind
//! with range and step, when it is shown or enabled), and how it is read and written
//! (`get` and `set`, through `Value`). The settings panel (`app::settings_ui`) and the
//! project file's settings block (`project`) are both generated from this table, so a
//! setting is described in one place.
//!
//! `PrintSettings` stays a plain struct: the slicer and G-code writer read its fields
//! directly. Entries are listed in panel order. Depends on `profiles` only.

use crate::profiles::{
    limits, AccelFeature, Adhesion, InfillPattern, PrintSettings, SeamPosition, SpeedFeature,
};
use std::ops::RangeInclusive;
use std::sync::OnceLock;

/// The panel sections, in the order the settings panel shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    LayerHeight,
    Seam,
    WallsAndSurfaces,
    Infill,
    Support,
    Adhesion,
    Speed,
    CoolingAndTravel,
    SpecialMode,
}

impl Group {
    /// Every group, in panel order.
    pub const ALL: [Group; 9] = [
        Self::LayerHeight,
        Self::Seam,
        Self::WallsAndSurfaces,
        Self::Infill,
        Self::Support,
        Self::Adhesion,
        Self::Speed,
        Self::CoolingAndTravel,
        Self::SpecialMode,
    ];

    /// The section heading in the panel.
    pub fn title(self) -> &'static str {
        match self {
            Self::LayerHeight => "Layer height",
            Self::Seam => "Seam",
            Self::WallsAndSurfaces => "Walls and surfaces",
            Self::Infill => "Infill",
            Self::Support => "Support",
            Self::Adhesion => "Adhesion",
            Self::Speed => "Speed",
            Self::CoolingAndTravel => "Cooling and travel",
            Self::SpecialMode => "Special mode",
        }
    }
}

/// Whether a setting shows in the simple panel or only with Advanced on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Simple,
    Advanced,
}

/// A setting's value, independent of which field holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A number (mm, mm/s, %, degrees, seconds).
    Num(f32),
    /// A whole number (wall and layer counts, fan %).
    Count(u32),
    /// On or off.
    Flag(bool),
    /// One of a fixed list, by its label (also its name in a project file).
    Choice(&'static str),
    /// A number, or `None` for "Auto".
    Auto(Option<f32>),
}

/// What an "Auto" value is worked out from, besides the print settings themselves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoContext {
    /// The printer's own acceleration (mm/s^2), for accelerations left on Auto.
    pub machine_accel: f32,
}

/// How a setting is edited, with its allowed range.
#[derive(Clone, Debug)]
pub enum Kind {
    /// A number field. `decimals: None` shows as many as egui chooses.
    Number {
        range: RangeInclusive<f32>,
        step: f64,
        suffix: &'static str,
        decimals: Option<usize>,
    },
    /// A whole-number field.
    Count {
        range: RangeInclusive<u32>,
        suffix: &'static str,
    },
    /// A checkbox.
    Flag,
    /// A drop-down list of labels.
    Choice { labels: fn() -> Vec<&'static str> },
    /// An "Auto" checkbox with a number field once Auto is off. `auto` gives the value
    /// shown (and started from) while on Auto; `auto_hint` is the checkbox's hover text.
    Auto {
        range: RangeInclusive<f32>,
        step: f64,
        suffix: &'static str,
        auto: fn(&PrintSettings, &AutoContext) -> f32,
        auto_hint: &'static str,
    },
}

/// One print setting: its project-file key, how it is shown, and how it is read and
/// written.
#[derive(Clone, Debug)]
pub struct Setting {
    /// The name in a project file's settings block; `a.b` is key `b` inside object `a`.
    pub key: &'static str,
    /// The row label in the panel.
    pub label: &'static str,
    pub group: Group,
    pub level: Level,
    pub kind: Kind,
    /// The label's hover text, which may depend on other settings.
    pub hint: fn(&PrintSettings) -> Option<&'static str>,
    /// Whether the row is shown at all, given the other settings.
    pub shown: fn(&PrintSettings) -> bool,
    /// Whether the row can be changed, given the other settings.
    pub enabled: fn(&PrintSettings) -> bool,
    /// Reads the value from the settings.
    pub get: fn(&PrintSettings) -> Value,
    /// Writes a value of this setting's kind into the settings (other kinds are ignored).
    pub set: fn(&mut PrintSettings, Value),
}

impl Setting {
    /// `v` held to this setting's range: numbers clamped, an unknown choice or a value of
    /// the wrong kind replaced by the default.
    pub fn clamp(&self, v: Value) -> Value {
        let fit = |x: f32, r: &RangeInclusive<f32>| x.clamp(*r.start(), *r.end());
        match (&self.kind, v) {
            (Kind::Number { range, .. }, Value::Num(x)) => Value::Num(fit(x, range)),
            (Kind::Count { range, .. }, Value::Count(n)) => {
                Value::Count(n.clamp(*range.start(), *range.end()))
            }
            (Kind::Flag, Value::Flag(b)) => Value::Flag(b),
            (Kind::Choice { labels }, Value::Choice(c)) if labels().contains(&c) => {
                Value::Choice(c)
            }
            (Kind::Auto { range, .. }, Value::Auto(a)) => Value::Auto(a.map(|x| fit(x, range))),
            (
                Kind::Number { .. }
                | Kind::Count { .. }
                | Kind::Flag
                | Kind::Choice { .. }
                | Kind::Auto { .. },
                Value::Num(_)
                | Value::Count(_)
                | Value::Flag(_)
                | Value::Choice(_)
                | Value::Auto(_),
            ) => self.default_value(),
        }
    }

    /// The value in `PrintSettings::default()`.
    pub fn default_value(&self) -> Value {
        (self.get)(&PrintSettings::default())
    }
}

/// Every print setting, in panel order.
pub fn all() -> &'static [Setting] {
    static ALL: OnceLock<Vec<Setting>> = OnceLock::new();
    ALL.get_or_init(build)
}

/// The setting with project-file key `key`.
pub fn find(key: &str) -> Option<&'static Setting> {
    all().iter().find(|s| s.key == key)
}

fn no_hint(_: &PrintSettings) -> Option<&'static str> {
    None
}

fn always(_: &PrintSettings) -> bool {
    true
}

fn with_supports(s: &PrintSettings) -> bool {
    s.supports
}

/// Settings measured in millimetres, shown with two decimals.
fn mm(range: RangeInclusive<f32>, step: f64) -> Kind {
    Kind::Number {
        range,
        step,
        suffix: " mm",
        decimals: Some(2),
    }
}

fn number(range: RangeInclusive<f32>, step: f64, suffix: &'static str) -> Kind {
    Kind::Number {
        range,
        step,
        suffix,
        decimals: None,
    }
}

/// A plain row: always shown and enabled, its hint fixed.
fn row(
    key: &'static str,
    label: &'static str,
    group: Group,
    level: Level,
    kind: Kind,
    get: fn(&PrintSettings) -> Value,
    set: fn(&mut PrintSettings, Value),
) -> Setting {
    Setting {
        key,
        label,
        group,
        level,
        kind,
        hint: no_hint,
        shown: always,
        enabled: always,
        get,
        set,
    }
}

/// Reads a field of one `Value` kind; writes it back only from that kind.
macro_rules! field {
    ($variant:ident, $($field:ident).+) => {
        (
            |s: &PrintSettings| Value::$variant(s.$($field).+),
            |s: &mut PrintSettings, v: Value| {
                if let Value::$variant(x) = v {
                    s.$($field).+ = x;
                }
            },
        )
    };
}

/// A drop-down field of enum `$ty` (which has `ALL` and `label()`).
macro_rules! choice_field {
    ($ty:ident, $field:ident) => {
        (
            |s: &PrintSettings| Value::Choice(s.$field.label()),
            |s: &mut PrintSettings, v: Value| {
                if let Value::Choice(c) = v {
                    if let Some(x) = $ty::ALL.into_iter().find(|x| x.label() == c) {
                        s.$field = x;
                    }
                }
            },
            || $ty::ALL.into_iter().map($ty::label).collect(),
        )
    };
}

/// One feature speed: Auto follows the print speed.
macro_rules! speed_row {
    ($feature:ident, $key:literal) => {{
        let f = SpeedFeature::$feature;
        Setting {
            hint: no_hint,
            shown: always,
            enabled: always,
            kind: Kind::Auto {
                range: limits::PRINT_SPEED,
                step: 1.0,
                suffix: " mm/s",
                auto: |s, _| SpeedFeature::$feature.auto_speed(s.print_speed),
                auto_hint: "Follow the print speed",
            },
            key: concat!("feature_speeds.", $key),
            label: f.label(),
            group: Group::Speed,
            level: Level::Advanced,
            get: |s| Value::Auto(s.speeds.get(SpeedFeature::$feature)),
            set: |s, v| {
                if let Value::Auto(a) = v {
                    s.speeds.set(SpeedFeature::$feature, a);
                }
            },
        }
    }};
}

/// One feature acceleration: Auto is the printer's own acceleration.
macro_rules! accel_row {
    ($feature:ident, $key:literal) => {{
        let f = AccelFeature::$feature;
        Setting {
            hint: no_hint,
            shown: always,
            enabled: always,
            kind: Kind::Auto {
                range: limits::ACCELERATION,
                step: 50.0,
                suffix: " mm/s²",
                auto: |_, ctx| ctx.machine_accel,
                auto_hint: "Use the printer's own acceleration (set in the printer editor)",
            },
            key: concat!("feature_accels.", $key),
            label: f.label(),
            group: Group::Speed,
            level: Level::Advanced,
            get: |s| Value::Auto(s.accels.get(AccelFeature::$feature)),
            set: |s, v| {
                if let Value::Auto(a) = v {
                    s.accels.set(AccelFeature::$feature, a);
                }
            },
        }
    }};
}

/// The table, in panel order (within each group, the order rows are drawn).
fn build() -> Vec<Setting> {
    use Group as G;
    use Level::{Advanced as A, Simple as S};
    let pct = |range| number(range, 1.0, " %");
    let speed = |range| number(range, 1.0, " mm/s");
    let (seam_get, seam_set, seam_labels) = choice_field!(SeamPosition, seam);
    let (pattern_get, pattern_set, pattern_labels) = choice_field!(InfillPattern, infill_pattern);
    let (adhesion_get, adhesion_set, adhesion_labels) = choice_field!(Adhesion, adhesion);
    macro_rules! r {
        ($key:literal, $label:literal, $g:expr, $l:expr, $kind:expr, $variant:ident, $($f:ident).+) => {{
            let (get, set) = field!($variant, $($f).+);
            row($key, $label, $g, $l, $kind, get, set)
        }};
    }
    let hinted = |mut s: Setting, hint: fn(&PrintSettings) -> Option<&'static str>| {
        s.hint = hint;
        s
    };
    vec![
        // Layer height
        r!(
            "layer_height",
            "Layer height",
            G::LayerHeight,
            S,
            mm(limits::LAYER_HEIGHT, 0.01),
            Num,
            layer_height
        ),
        r!(
            "first_layer_height",
            "First layer height",
            G::LayerHeight,
            A,
            mm(limits::FIRST_LAYER_HEIGHT, 0.01),
            Num,
            first_layer_height
        ),
        // Seam
        row(
            "seam",
            "Seam position",
            G::Seam,
            S,
            Kind::Choice {
                labels: seam_labels,
            },
            seam_get,
            seam_set,
        ),
        // Walls and surfaces
        r!(
            "walls",
            "Walls",
            G::WallsAndSurfaces,
            S,
            Kind::Count {
                range: limits::WALLS,
                suffix: ""
            },
            Count,
            walls
        ),
        r!(
            "top_layers",
            "Top layers",
            G::WallsAndSurfaces,
            S,
            Kind::Count {
                range: limits::SOLID_LAYERS,
                suffix: ""
            },
            Count,
            top_layers
        ),
        r!(
            "bottom_layers",
            "Bottom layers",
            G::WallsAndSurfaces,
            S,
            Kind::Count {
                range: limits::SOLID_LAYERS,
                suffix: ""
            },
            Count,
            bottom_layers
        ),
        r!(
            "ironing",
            "Ironing (smooth tops)",
            G::WallsAndSurfaces,
            S,
            Kind::Flag,
            Flag,
            ironing
        ),
        hinted(
            r!(
                "fuzzy_top",
                "Fuzzy top (textured top)",
                G::WallsAndSurfaces,
                S,
                Kind::Flag,
                Flag,
                fuzzy_top
            ),
            |_| {
                Some("While laying the visible top surface, the nozzle randomly lifts by up to the amount below, for a textured, matte top. Ironing is skipped while this is on.")
            },
        ),
        Setting {
            shown: |s| s.fuzzy_top,
            ..r!(
                "fuzzy_top_mm",
                "Fuzzy top amount",
                G::WallsAndSurfaces,
                S,
                mm(limits::FUZZY_TOP_MM, 0.01),
                Num,
                fuzzy_top_mm
            )
        },
        r!(
            "fuzzy_skin",
            "Fuzzy skin",
            G::WallsAndSurfaces,
            A,
            Kind::Flag,
            Flag,
            fuzzy_skin
        ),
        hinted(
            r!(
                "infill_overlap",
                "Infill/wall overlap",
                G::WallsAndSurfaces,
                A,
                pct(limits::INFILL_OVERLAP),
                Num,
                infill_overlap
            ),
            |_| {
                Some("How far infill reaches into the innermost wall, as a share of a line width, so the two bond")
            },
        ),
        hinted(
            r!(
                "elephant_foot",
                "Elephant foot",
                G::WallsAndSurfaces,
                A,
                mm(limits::ELEPHANT_FOOT, 0.01),
                Num,
                elephant_foot
            ),
            |_| {
                Some("Draw the first layer in by this much, to offset the squash that makes it wider than the rest (not with a raft)")
            },
        ),
        hinted(
            r!(
                "hole_compensation",
                "Hole compensation",
                G::WallsAndSurfaces,
                A,
                mm(limits::HOLE_COMPENSATION, 0.01),
                Num,
                hole_compensation
            ),
            |_| Some("Make every hole this much larger all round, as printed holes come out small"),
        ),
        // Infill
        r!(
            "infill_percent",
            "Density",
            G::Infill,
            S,
            Kind::Number {
                range: limits::INFILL_PERCENT,
                step: 1.0,
                suffix: " %",
                decimals: Some(0)
            },
            Num,
            infill_percent
        ),
        row(
            "infill_pattern",
            "Pattern",
            G::Infill,
            S,
            Kind::Choice {
                labels: pattern_labels,
            },
            pattern_get,
            pattern_set,
        ),
        // Support
        r!(
            "supports",
            "Enable support",
            G::Support,
            S,
            Kind::Flag,
            Flag,
            supports
        ),
        Setting {
            shown: with_supports,
            ..r!(
                "support_angle",
                "Overhang angle",
                G::Support,
                S,
                number(limits::SUPPORT_ANGLE, 1.0, "°"),
                Num,
                support_angle
            )
        },
        Setting {
            shown: with_supports,
            ..r!(
                "supports_on_plate_only",
                "On build plate only",
                G::Support,
                S,
                Kind::Flag,
                Flag,
                supports_on_plate_only
            )
        },
        Setting {
            shown: with_supports,
            // With a full raft the supports already stand on it.
            enabled: |s| s.adhesion != Adhesion::Raft,
            hint: |s| {
                Some(if s.adhesion == Adhesion::Raft {
                    "Adhesion is set to Raft, which already covers the supports"
                } else {
                    "Supports that reach the bed stand on a small raft of their own, so they stick"
                })
            },
            ..r!(
                "support_raft",
                "Raft under support",
                G::Support,
                S,
                Kind::Flag,
                Flag,
                support_raft
            )
        },
        // Adhesion
        row(
            "adhesion",
            "Type",
            G::Adhesion,
            S,
            Kind::Choice {
                labels: adhesion_labels,
            },
            adhesion_get,
            adhesion_set,
        ),
        // Speed
        r!(
            "print_speed",
            "Print speed",
            G::Speed,
            S,
            speed(limits::PRINT_SPEED),
            Num,
            print_speed
        ),
        r!(
            "first_layer_speed",
            "First layer speed",
            G::Speed,
            A,
            speed(limits::FIRST_LAYER_SPEED),
            Num,
            first_layer_speed
        ),
        r!(
            "travel_speed",
            "Travel speed",
            G::Speed,
            A,
            speed(limits::TRAVEL_SPEED),
            Num,
            travel_speed
        ),
        speed_row!(OuterWall, "outer_wall"),
        speed_row!(InnerWall, "inner_wall"),
        speed_row!(SparseInfill, "sparse_infill"),
        speed_row!(SolidInfill, "solid_infill"),
        speed_row!(TopSurface, "top_surface"),
        speed_row!(Support, "support"),
        speed_row!(Bridge, "bridge"),
        accel_row!(OuterWall, "outer_wall"),
        accel_row!(Infill, "infill"),
        accel_row!(Travel, "travel"),
        accel_row!(FirstLayer, "first_layer"),
        // Cooling and travel
        r!(
            "min_layer_time",
            "Minimum layer time",
            G::CoolingAndTravel,
            A,
            number(limits::MIN_LAYER_TIME, 0.5, " s"),
            Num,
            min_layer_time
        ),
        r!(
            "z_hop",
            "Z-hop",
            G::CoolingAndTravel,
            A,
            mm(limits::Z_HOP, 0.05),
            Num,
            z_hop
        ),
        hinted(
            r!(
                "avoid_crossing_walls",
                "Avoid crossing walls",
                G::CoolingAndTravel,
                A,
                Kind::Flag,
                Flag,
                avoid_crossing_walls
            ),
            |_| {
                Some("Travel along the inside of the outer wall instead of out over air or across holes, and retract only when a travel must leave the part")
            },
        ),
        hinted(
            r!(
                "bridge_flow",
                "Bridge flow",
                G::CoolingAndTravel,
                A,
                pct(limits::BRIDGE_FLOW),
                Num,
                bridge_flow
            ),
            |_| Some("Plastic on bridge lines, as a share of a normal line"),
        ),
        hinted(
            r!(
                "bridge_fan",
                "Bridge fan",
                G::CoolingAndTravel,
                A,
                Kind::Count {
                    range: limits::FAN_PERCENT,
                    suffix: " %"
                },
                Count,
                bridge_fan
            ),
            |_| Some("Part fan while printing bridge lines"),
        ),
        // Special mode
        r!(
            "vase_mode",
            "Spiral vase",
            G::SpecialMode,
            A,
            Kind::Flag,
            Flag,
            vase_mode
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{
        AccelFeature, Adhesion, FeatureAccels, FeatureSpeeds, InfillPattern, PrintSettings,
        SeamPosition, SpeedFeature,
    };

    /// Every print setting away from its default. A struct literal with no `..`: adding a
    /// field to `PrintSettings` fails to compile here until the registry is updated too.
    fn every_setting_changed() -> PrintSettings {
        let mut speeds = FeatureSpeeds::default();
        let mut accels = FeatureAccels::default();
        for (i, f) in SpeedFeature::ALL.into_iter().enumerate() {
            speeds.set(f, Some(20.0 + i as f32));
        }
        for (i, f) in AccelFeature::ALL.into_iter().enumerate() {
            accels.set(f, Some(1500.0 + i as f32));
        }
        PrintSettings {
            layer_height: 0.12,
            first_layer_height: 0.24,
            walls: 3,
            top_layers: 5,
            bottom_layers: 4,
            infill_percent: 40.0,
            print_speed: 70.0,
            first_layer_speed: 20.0,
            travel_speed: 180.0,
            speeds,
            accels,
            bridge_flow: 90.0,
            bridge_fan: 80,
            avoid_crossing_walls: true,
            infill_overlap: 20.0,
            elephant_foot: 0.15,
            hole_compensation: 0.1,
            supports: true,
            support_angle: 55.0,
            supports_on_plate_only: true,
            support_raft: false,
            adhesion: Adhesion::Raft,
            infill_pattern: InfillPattern::Triangles,
            seam: SeamPosition::Rear,
            ironing: true,
            fuzzy_skin: true,
            fuzzy_top: true,
            fuzzy_top_mm: 0.2,
            vase_mode: true,
            z_hop: 0.4,
            min_layer_time: 10.0,
        }
    }

    #[test]
    fn test_registry_covers_every_field() {
        let full = every_setting_changed();
        assert_ne!(full, PrintSettings::default());
        let mut d = PrintSettings::default();
        for s in all() {
            (s.set)(&mut d, (s.get)(&full));
        }
        assert_eq!(d, full);
    }

    #[test]
    fn test_registry_keys_unique_and_defaults_match() {
        let mut keys: Vec<&str> = all().iter().map(|s| s.key).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate keys");
        for s in all() {
            assert_eq!(
                s.default_value(),
                (s.get)(&PrintSettings::default()),
                "{}",
                s.key
            );
        }
        // Feature keys follow the features' own project names.
        for f in SpeedFeature::ALL {
            assert!(
                find(&format!("feature_speeds.{}", f.key())).is_some(),
                "{f:?}"
            );
        }
        for f in AccelFeature::ALL {
            assert!(
                find(&format!("feature_accels.{}", f.key())).is_some(),
                "{f:?}"
            );
        }
    }

    #[test]
    fn test_registry_clamps_and_rejects_unknown_choices() {
        let walls = find("walls").unwrap();
        assert_eq!(walls.clamp(Value::Count(0)), Value::Count(1));
        assert_eq!(walls.clamp(Value::Count(99)), Value::Count(10));
        let outer = find("feature_speeds.outer_wall").unwrap();
        assert_eq!(
            outer.clamp(Value::Auto(Some(9999.0))),
            Value::Auto(Some(500.0))
        );
        assert_eq!(outer.clamp(Value::Auto(None)), Value::Auto(None));
        let seam = find("seam").unwrap();
        assert_eq!(seam.clamp(Value::Choice("Nowhere")), seam.default_value());
        assert_eq!(seam.clamp(Value::Choice("Rear")), Value::Choice("Rear"));
        // A value of the wrong kind falls back to the default.
        assert_eq!(walls.clamp(Value::Flag(true)), walls.default_value());
        assert!(find("no_such_setting").is_none());
    }

    #[test]
    fn test_registry_conditional_rows() {
        let off = PrintSettings::default();
        assert!(!off.supports);
        for key in ["support_angle", "supports_on_plate_only", "support_raft"] {
            assert!(!(find(key).unwrap().shown)(&off), "{key}");
            let on = PrintSettings {
                supports: true,
                ..PrintSettings::default()
            };
            assert!((find(key).unwrap().shown)(&on), "{key}");
        }
        let raft = find("support_raft").unwrap();
        let full_raft = PrintSettings {
            adhesion: Adhesion::Raft,
            ..PrintSettings::default()
        };
        assert!(!(raft.enabled)(&full_raft));
        assert!((raft.enabled)(&off));
        assert_ne!((raft.hint)(&full_raft), (raft.hint)(&off));
        let amount = find("fuzzy_top_mm").unwrap();
        assert!(!(amount.shown)(&off));
        let fuzzy = PrintSettings {
            fuzzy_top: true,
            ..PrintSettings::default()
        };
        assert!((amount.shown)(&fuzzy));
    }
}
