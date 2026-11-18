//! Modifier volumes: a box, cylinder or sphere attached to a part. Where the part is
//! inside it, its infill density and pattern can differ (`slicer::InfillModifier`).
//!
//! A modifier is placed relative to its part: `at` is the offset of its centre from the
//! centre of the part's footprint (x, y) and the height of its bottom above the plate
//! (z), in plate axes. Turning the part doesn't turn its modifiers.

use crate::geometry::Vec3;
use crate::model::{Mesh, Triangle};
use crate::overrides::Overrides;
use crate::settings_registry::Value;
use crate::slicer::InfillModifier;
use serde_json::{json, Value as Json};

/// The settings a modifier can change.
pub const PER_MODIFIER: [&str; 2] = ["infill_percent", "infill_pattern"];

/// A modifier's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierShape {
    Box,
    Cylinder,
    Sphere,
}

impl ModifierShape {
    pub const ALL: [ModifierShape; 3] = [Self::Box, Self::Cylinder, Self::Sphere];

    /// How it is named in the window.
    pub fn label(self) -> &'static str {
        match self {
            Self::Box => "Box",
            Self::Cylinder => "Cylinder",
            Self::Sphere => "Sphere",
        }
    }

    /// The name in a project file.
    pub fn key(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Cylinder => "cylinder",
            Self::Sphere => "sphere",
        }
    }
}

/// A modifier volume of a part.
#[derive(Clone, Debug, PartialEq)]
pub struct Modifier {
    pub shape: ModifierShape,
    /// Width, depth and height (mm).
    pub size: Vec3,
    /// Centre offset from the part's footprint centre (x, y) and bottom height (z), mm.
    pub at: Vec3,
    /// The settings that differ inside it (only `PER_MODIFIER` keys are used).
    pub overrides: Overrides,
}

impl Modifier {
    /// A modifier of `shape` around the middle of a part `size` across, half its size
    /// each way, with no settings yet.
    pub fn around_middle(shape: ModifierShape, size: Vec3) -> Self {
        let half = |v: f32| (v * 0.5 * 10.0).round() / 10.0;
        let s = Vec3::new(half(size.x), half(size.y), half(size.z));
        Self {
            shape,
            size: s,
            at: Vec3::new(0.0, 0.0, half(size.z - s.z)),
            overrides: Overrides::default(),
        }
    }

    /// Its triangles on the plate, for a part whose footprint is centred on (x, y).
    pub fn triangles(&self, x: f32, y: f32) -> Vec<Triangle> {
        let mesh = match self.shape {
            ModifierShape::Box => Mesh::cube(self.size),
            ModifierShape::Cylinder => {
                let mut m = Mesh::cylinder(0.5, 0.5, 1.0, 48);
                stretch(&mut m, self.size);
                m
            }
            ModifierShape::Sphere => {
                let mut m = unit_sphere(24, 16);
                stretch(&mut m, self.size);
                m
            }
        };
        let shift = Vec3::new(x + self.at.x, y + self.at.y, self.at.z);
        mesh.triangles
            .iter()
            .map(|t| t.map(|v| v + shift))
            .collect()
    }

    /// The modifier as the slicer takes it, for a part centred on (x, y).
    pub fn infill(&self, x: f32, y: f32) -> InfillModifier {
        InfillModifier {
            triangles: self.triangles(x, y),
            infill_percent: match self.overrides.get("infill_percent") {
                Some(Value::Num(p)) => Some(p),
                _ => None,
            },
            infill_pattern: match self.overrides.get("infill_pattern") {
                Some(Value::Choice(c)) => crate::profiles::InfillPattern::ALL
                    .into_iter()
                    .find(|p| p.label() == c),
                _ => None,
            },
        }
    }

    /// The modifier as a project file writes it.
    pub fn to_json(&self) -> Json {
        json!({
            "shape": self.shape.key(),
            "size": [self.size.x, self.size.y, self.size.z],
            "at": [self.at.x, self.at.y, self.at.z],
            "overrides": self.overrides.to_json(),
        })
    }

    /// A modifier from a project file; `None` for an unknown shape or a missing or
    /// non-positive size. Settings a modifier can't change are dropped.
    pub fn from_json(raw: &Json) -> Option<Self> {
        let shape = ModifierShape::ALL
            .into_iter()
            .find(|s| raw.get("shape").and_then(Json::as_str) == Some(s.key()))?;
        let triple = |k: &str| -> Option<Vec3> {
            let a = raw.get(k)?.as_array()?;
            let c = |i: usize| a.get(i).and_then(Json::as_f64).map(|x| x as f32);
            Some(Vec3::new(c(0)?, c(1)?, c(2)?))
        };
        let size = triple("size").filter(|s| s.x > 0.0 && s.y > 0.0 && s.z > 0.0)?;
        let at = triple("at").unwrap_or_default();
        let mut overrides = Overrides::default();
        let all = raw
            .get("overrides")
            .map(Overrides::from_json)
            .unwrap_or_default();
        for key in PER_MODIFIER {
            if let Some(v) = all.get(key) {
                overrides.set(key, v);
            }
        }
        Some(Self {
            shape,
            size,
            at: Vec3::new(at.x, at.y, at.z.max(0.0)),
            overrides,
        })
    }
}

/// Scales a mesh one unit across (and one tall, from z = 0) to `size`.
fn stretch(m: &mut Mesh, size: Vec3) {
    for t in &mut m.triangles {
        for v in t.iter_mut() {
            *v = Vec3::new(v.x * size.x, v.y * size.y, v.z * size.z);
        }
    }
}

/// A sphere one unit across, resting on z = 0: `around` segments round, `up` bands
/// from the bottom pole to the top one, faces pointing out.
fn unit_sphere(around: usize, up: usize) -> Mesh {
    let point = |a: usize, b: usize| {
        let (theta, phi) = (
            std::f32::consts::TAU * a as f32 / around as f32,
            std::f32::consts::PI * b as f32 / up as f32,
        );
        Vec3::new(
            0.5 * phi.sin() * theta.cos(),
            0.5 * phi.sin() * theta.sin(),
            0.5 - 0.5 * phi.cos(),
        )
    };
    let mut triangles = Vec::new();
    for b in 0..up {
        for a in 0..around {
            let (p00, p10) = (point(a, b), point(a + 1, b));
            let (p01, p11) = (point(a, b + 1), point(a + 1, b + 1));
            if b > 0 {
                triangles.push([p00, p10, p11]);
            }
            if b + 1 < up {
                triangles.push([p00, p11, p01]);
            }
        }
    }
    Mesh { triangles }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(tris: &[Triangle]) -> f32 {
        tris.iter().map(|t| t[0].dot(t[1].cross(t[2])) / 6.0).sum()
    }

    #[test]
    fn test_modifier_shapes_have_their_size_and_place() {
        for shape in ModifierShape::ALL {
            let m = Modifier {
                shape,
                size: Vec3::new(10.0, 6.0, 4.0),
                at: Vec3::new(1.0, -2.0, 3.0),
                overrides: Overrides::default(),
            };
            let tris = m.triangles(20.0, 30.0);
            let b = crate::geometry::Bounds::of(tris.iter().flatten().copied());
            let s = b.size();
            assert!(
                (s.x - 10.0).abs() < 0.05 && (s.y - 6.0).abs() < 0.2,
                "{shape:?} {s:?}"
            );
            assert!((s.z - 4.0).abs() < 1e-3, "{shape:?} {s:?}");
            assert!((b.center().x - 21.0).abs() < 0.05 && (b.center().y - 28.0).abs() < 0.1);
            assert!((b.min.z - 3.0).abs() < 1e-3);
            // Closed and facing out: the volume is about the shape's.
            let want = match shape {
                ModifierShape::Box => 240.0,
                ModifierShape::Cylinder => std::f32::consts::PI * 5.0 * 3.0 * 4.0,
                ModifierShape::Sphere => 4.0 / 3.0 * std::f32::consts::PI * 5.0 * 3.0 * 2.0,
            };
            let v = volume(&tris);
            assert!((v - want).abs() < 0.05 * want, "{shape:?}: {v} vs {want}");
        }
    }

    #[test]
    fn test_modifier_round_trips_and_keeps_only_infill_settings() {
        let mut m = Modifier::around_middle(ModifierShape::Cylinder, Vec3::new(20.0, 20.0, 30.0));
        assert_eq!(m.size, Vec3::new(10.0, 10.0, 15.0));
        assert_eq!(m.at, Vec3::new(0.0, 0.0, 7.5));
        m.overrides.set("infill_percent", Value::Num(100.0));
        m.overrides.set("infill_pattern", Value::Choice("Lines"));
        assert_eq!(Modifier::from_json(&m.to_json()), Some(m.clone()));
        let infill = m.infill(0.0, 0.0);
        assert_eq!(infill.infill_percent, Some(100.0));
        assert_eq!(
            infill.infill_pattern,
            Some(crate::profiles::InfillPattern::Lines)
        );
        // Settings a modifier can't change, unknown shapes and empty sizes are dropped.
        let mut j = m.to_json();
        j["overrides"]["walls"] = json!(5);
        assert_eq!(
            Modifier::from_json(&j).unwrap().overrides.get("walls"),
            None
        );
        assert_eq!(
            Modifier::from_json(&json!({"shape": "cone", "size": [1, 1, 1]})),
            None
        );
        assert_eq!(
            Modifier::from_json(&json!({"shape": "box", "size": [0, 1, 1]})),
            None
        );
    }
}
