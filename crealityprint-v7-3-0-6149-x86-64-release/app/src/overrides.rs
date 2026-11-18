//! Per-object settings: a part can override some print settings of the plate, by their
//! registry key (`settings_registry`). Only settings the slicer uses to shape a part can
//! differ per part (walls, top and bottom layers, infill, ironing, fuzzy skin, size
//! compensation). Layer heights, adhesion and supports are shared by the whole plate,
//! and speeds, temperatures, the seam and the fuzzy top are applied while the G-code is
//! written, so those stay plate-wide.

use crate::profiles::PrintSettings;
use crate::settings_registry::{self, Setting, Value};
use serde_json::{Map, Value as Json};
use std::collections::BTreeMap;

/// The registry keys a part can override.
pub const PER_OBJECT: [&str; 10] = [
    "walls",
    "top_layers",
    "bottom_layers",
    "ironing",
    "fuzzy_skin",
    "infill_overlap",
    "elephant_foot",
    "hole_compensation",
    "infill_percent",
    "infill_pattern",
];

/// The settings a part can override, in panel order.
pub fn per_object_settings() -> impl Iterator<Item = &'static Setting> {
    settings_registry::all()
        .iter()
        .filter(|s| PER_OBJECT.contains(&s.key))
}

fn setting(key: &str) -> Option<&'static Setting> {
    per_object_settings().find(|s| s.key == key)
}

/// A part's overrides: setting key to value. Empty for a part that prints with the
/// plate's settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides(BTreeMap<&'static str, Value>);

impl Overrides {
    /// Overrides setting `key` with `v` (made valid for it). Returns false, changing
    /// nothing, when `key` isn't a setting a part can override.
    pub fn set(&mut self, key: &str, v: Value) -> bool {
        match setting(key) {
            Some(s) => {
                self.0.insert(s.key, s.sanitize(v));
                true
            }
            None => false,
        }
    }

    /// Goes back to the plate's value for `key`.
    pub fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }

    /// The overriding value for `key`, if there is one.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.0.get(key).copied()
    }

    /// How many settings are overridden.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The settings the part prints with: the plate's, with these overrides.
    pub fn apply(&self, plate: &PrintSettings) -> PrintSettings {
        let mut s = plate.clone();
        for (key, v) in &self.0 {
            if let Some(setting) = setting(key) {
                setting.write(&mut s, *v);
            }
        }
        s
    }

    /// The overrides as a project file writes them: `{key: value}`.
    pub fn to_json(&self) -> Json {
        let map: Map<String, Json> = self
            .0
            .iter()
            .map(|(k, v)| ((*k).to_string(), settings_registry::value_json(*v)))
            .collect();
        Json::Object(map)
    }

    /// The overrides in a project file's `{key: value}`. Keys a part can't override and
    /// values of the wrong kind are left out.
    pub fn from_json(raw: &Json) -> Self {
        let mut out = Self::default();
        for (key, value) in raw.as_object().into_iter().flatten() {
            if let Some(s) = setting(key) {
                if let Some(v) = s.parse_json(value) {
                    out.0.insert(s.key, v);
                }
            }
        }
        out
    }
}

/// Settings for part of a part's height: from `from` up to `to` (mm above the part's
/// bottom; from inclusive, to exclusive) it prints with `overrides` on top of its own
/// settings, for example denser infill near the top.
#[derive(Clone, Debug, PartialEq)]
pub struct HeightRange {
    pub from: f32,
    pub to: f32,
    pub overrides: Overrides,
}

impl HeightRange {
    /// The range as a project file writes it: `{"from", "to", "overrides"}`.
    pub fn to_json(&self) -> Json {
        serde_json::json!({
            "from": self.from,
            "to": self.to,
            "overrides": self.overrides.to_json(),
        })
    }

    /// A range from a project file; `None` without both heights, or when `to` isn't
    /// above `from`.
    pub fn from_json(raw: &Json) -> Option<Self> {
        let height = |k: &str| raw.get(k).and_then(Json::as_f64).map(|x| x as f32);
        let (from, to) = (height("from")?.max(0.0), height("to")?);
        (to > from).then(|| Self {
            from,
            to,
            overrides: raw
                .get("overrides")
                .map(Overrides::from_json)
                .unwrap_or_default(),
        })
    }
}

/// The settings a part prints with, height ranges included, given the plate's.
pub fn part_settings(
    plate: &PrintSettings,
    own: &Overrides,
    ranges: &[HeightRange],
) -> crate::slicer::PartSettings {
    let base = own.apply(plate);
    crate::slicer::PartSettings {
        ranges: ranges
            .iter()
            .map(|r| (r.from, r.to, r.overrides.apply(&base)))
            .collect(),
        base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_overrides_apply_over_the_plate_settings() {
        let plate = PrintSettings::default();
        let mut o = Overrides::default();
        assert!(o.is_empty());
        assert_eq!(o.apply(&plate), plate);
        assert!(o.set("infill_percent", Value::Num(80.0)));
        assert!(o.set("walls", Value::Count(5)));
        let s = o.apply(&plate);
        assert_eq!(s.infill_percent, 80.0);
        assert_eq!(s.walls, 5);
        assert_eq!(
            s.layer_height, plate.layer_height,
            "the rest is the plate's"
        );
        o.remove("walls");
        assert_eq!(o.apply(&plate).walls, plate.walls);
    }

    #[test]
    fn test_overrides_only_take_per_object_settings_in_range() {
        let mut o = Overrides::default();
        assert!(!o.set("layer_height", Value::Num(0.1)), "plate-wide");
        assert!(!o.set("no_such_setting", Value::Flag(true)));
        assert!(o.is_empty());
        o.set("infill_percent", Value::Num(500.0));
        assert_eq!(o.get("infill_percent"), Some(Value::Num(100.0)));
        // Every listed key is a real registry setting.
        assert_eq!(per_object_settings().count(), PER_OBJECT.len());
    }

    #[test]
    fn test_overrides_round_trip_through_json() {
        let mut o = Overrides::default();
        o.set("infill_pattern", Value::Choice("Triangles"));
        o.set("ironing", Value::Flag(true));
        o.set("top_layers", Value::Count(7));
        let j = o.to_json();
        assert_eq!(
            j,
            json!({"infill_pattern": "Triangles", "ironing": true, "top_layers": 7})
        );
        assert_eq!(Overrides::from_json(&j), o);
        // Unknown keys, plate-wide keys and wrong kinds are dropped.
        let odd = json!({"walls": "lots", "layer_height": 0.1, "bogus": 1, "bottom_layers": 2});
        let mut want = Overrides::default();
        want.set("bottom_layers", Value::Count(2));
        assert_eq!(Overrides::from_json(&odd), want);
    }

    #[test]
    fn test_height_range_round_trips_and_builds_part_settings() {
        let mut own = Overrides::default();
        own.set("walls", Value::Count(3));
        let mut top = Overrides::default();
        top.set("infill_percent", Value::Num(90.0));
        let range = HeightRange {
            from: 8.0,
            to: 10.0,
            overrides: top,
        };
        assert_eq!(
            HeightRange::from_json(&range.to_json()),
            Some(range.clone())
        );
        assert_eq!(
            HeightRange::from_json(&json!({"from": 4.0, "to": 4.0})),
            None
        );
        assert_eq!(HeightRange::from_json(&json!({"to": 4.0})), None);
        let plate = PrintSettings::default();
        let ps = part_settings(&plate, &own, &[range]);
        assert_eq!(ps.base.walls, 3);
        assert_eq!(ps.at(9.0).infill_percent, 90.0);
        assert_eq!(
            ps.at(9.0).walls,
            3,
            "a range builds on the part's own settings"
        );
        assert_eq!(ps.at(10.0).infill_percent, plate.infill_percent);
    }
}
