//! PrintCraft projects (`.printcraft` files): everything needed to carry on later.
//! * every object on the plate: its name, its shape (stored in the file, so the
//!   original STL/3MF can move or vanish), position, rotation and scale
//! * the process settings, quality preset and filament
//! * the printer profile with all its edited values
//! * the printer device it was connected to (address or USB port, never a password)
//!
//! The file is JSON. Missing or unknown values fall back to defaults, so files from
//! older or newer versions still open; a file from a newer version is flagged so the
//! window can warn that it may use features this version doesn't support.

use crate::codec::base64_decode;
use crate::geometry::Vec3;
use crate::model::{Mesh, PlacedObject, Triangle};
use crate::printer::FoundPrinter;
use crate::profiles::{
    limits, AccelFeature, Adhesion, DualNozzle, FeatureAccels, FeatureSpeeds, Filament,
    InfillPattern, PrintSettings, Printer, Quality, SeamPosition, SpeedFeature,
};
use serde_json::{json, Value};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "printcraft";
const FORMAT: &str = "printcraft-project";
pub const VERSION: u64 = 1;

#[derive(Clone, Debug)]
pub struct Project {
    pub printer: Printer,
    pub filament: Filament,
    pub settings: PrintSettings,
    pub quality: Option<Quality>,
    pub objects: Vec<PlacedObject>,
    /// The printer device in use when the project was saved.
    pub device: Option<FoundPrinter>,
    /// Two-nozzle choices (second filament, support nozzle, Ditto).
    pub dual: DualNozzle,
}

/// A project read from a file, with the format version it was written in.
pub struct Opened {
    pub project: Project,
    /// The file's format version (`VERSION` when it doesn't say, as the first files didn't).
    pub file_version: u64,
}

impl Opened {
    /// Saved by a newer PrintCraft: it may hold settings this version can't read, which
    /// are left out now and would be lost by saving it here.
    pub fn is_from_newer_version(&self) -> bool {
        self.file_version > VERSION
    }
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Fine => "fine",
        Quality::Standard => "standard",
        Quality::Draft => "draft",
    }
}

pub fn to_json(p: &Project) -> String {
    let s = &p.settings;
    let objects: Vec<Value> = p
        .objects
        .iter()
        .map(|o| {
            json!({
                "name": o.name,
                "x": o.x,
                "y": o.y,
                "rotation": [o.rotation.x, o.rotation.y, o.rotation.z],
                "scale_percent": o.scale_percent,
                "extruder": o.extruder,
                "triangles": o.mesh.triangles.len(),
                "mesh": crate::codec::base64(&mesh_bytes(&o.mesh.triangles)),
            })
        })
        .collect();
    let v = json!({
        "format": FORMAT,
        "version": VERSION,
        "printer": {
            "name": p.printer.name,
            "bed_x": p.printer.bed_x,
            "bed_y": p.printer.bed_y,
            "max_z": p.printer.max_z,
            "nozzle_diameter": p.printer.nozzle_diameter,
            "retract_length": p.printer.retract_length,
            "retract_speed": p.printer.retract_speed,
            "purge_mm": p.printer.purge_mm,
            "klipper": p.printer.klipper,
            "x3g": p.printer.x3g,
            "extruders": p.printer.extruders,
            "retract_min_travel": p.printer.retract_min_travel,
            "retract_layer_change": p.printer.retract_layer_change,
            "wipe_mm": p.printer.wipe_mm,
            "tool_change_retract": p.printer.tool_change_retract,
            "standby_drop": p.printer.standby_drop,
            "primary_nozzle": p.printer.primary_nozzle,
            "nozzle_spacing": p.printer.nozzle_spacing,
            "acceleration": p.printer.acceleration,
        },
        "filament": filament_json(&p.filament),
        "dual": {
            "second_filament": p.dual.second_filament.as_ref().map(filament_json),
            "support_tool": p.dual.support_tool,
            "ditto": p.dual.ditto,
            "prime_tower": p.dual.prime_tower,
            "tower_at": p.dual.tower_at,
            "ooze_shield": p.dual.ooze_shield,
            "shield_distance": p.dual.shield_distance,
        },
        "quality": p.quality.map(quality_name),
        "settings": {
            "layer_height": s.layer_height,
            "first_layer_height": s.first_layer_height,
            "walls": s.walls,
            "top_layers": s.top_layers,
            "bottom_layers": s.bottom_layers,
            "infill_percent": s.infill_percent,
            "print_speed": s.print_speed,
            "first_layer_speed": s.first_layer_speed,
            "travel_speed": s.travel_speed,
            "bridge_flow": s.bridge_flow,
            "bridge_fan": s.bridge_fan,
            "avoid_crossing_walls": s.avoid_crossing_walls,
            "infill_overlap": s.infill_overlap,
            "elephant_foot": s.elephant_foot,
            "hole_compensation": s.hole_compensation,
            "feature_accels": AccelFeature::ALL
                .into_iter()
                .map(|f| (f.key().to_string(), json!(s.accels.get(f))))
                .collect::<serde_json::Map<String, Value>>(),
            "feature_speeds": SpeedFeature::ALL
                .into_iter()
                .map(|f| (f.key().to_string(), json!(s.speeds.get(f))))
                .collect::<serde_json::Map<String, Value>>(),
            "supports": s.supports,
            "support_angle": s.support_angle,
            "supports_on_plate_only": s.supports_on_plate_only,
            "support_raft": s.support_raft,
            "adhesion": s.adhesion.label(),
            "infill_pattern": s.infill_pattern.label(),
            "seam": s.seam.label(),
            "ironing": s.ironing,
            "fuzzy_skin": s.fuzzy_skin,
            "fuzzy_top": s.fuzzy_top,
            "fuzzy_top_mm": s.fuzzy_top_mm,
            "vase_mode": s.vase_mode,
            "z_hop": s.z_hop,
            "min_layer_time": s.min_layer_time,
        },
        "objects": objects,
        "device": p.device.as_ref().map(FoundPrinter::to_json),
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Reads a project. Values that are missing or of the wrong kind keep their defaults.
/// The window uses `opened_from_json` / `open`; these two plain forms serve the tests.
#[cfg(test)]
pub fn from_json(text: &str) -> Result<Project, String> {
    opened_from_json(text).map(|o| o.project)
}

/// Reads a project and the format version it was written in.
pub fn opened_from_json(text: &str) -> Result<Opened, String> {
    let v: Value = serde_json::from_str(text)
        .map_err(|_| "This isn't a PrintCraft project file.".to_string())?;
    if v.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
        return Err("This isn't a PrintCraft project file.".into());
    }
    Ok(Opened {
        project: project_from(&v)?,
        file_version: v.get("version").and_then(|x| x.as_u64()).unwrap_or(VERSION),
    })
}

fn project_from(v: &Value) -> Result<Project, String> {
    let num = |o: &Value, k: &str, d: f32| {
        o.get(k)
            .and_then(|x| x.as_f64())
            .map(|x| x as f32)
            .unwrap_or(d)
    };
    let int = |o: &Value, k: &str, d: u32| {
        o.get(k)
            .and_then(|x| x.as_u64())
            .map(|x| x.min(u32::MAX as u64) as u32)
            .unwrap_or(d)
    };
    // A hand-edited or damaged file is held to what the settings panel allows.
    let num_in = |o: &Value, k: &str, d: f32, r: RangeInclusive<f32>| {
        num(o, k, d).clamp(*r.start(), *r.end())
    };
    let int_in = |o: &Value, k: &str, d: u32, r: RangeInclusive<u32>| {
        int(o, k, d).clamp(*r.start(), *r.end())
    };
    let flag = |o: &Value, k: &str, d: bool| o.get(k).and_then(|x| x.as_bool()).unwrap_or(d);
    let text_of = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_string);
    // A filament (the main one, or the second nozzle's); anything missing is the first
    // preset's.
    let filament_from = |fi: &Value| {
        let fbase = Filament::presets().into_iter().next().expect("presets");
        Filament {
            name: text_of(fi, "name").unwrap_or(fbase.name.clone()),
            nozzle_temp: int_in(fi, "nozzle_temp", fbase.nozzle_temp, limits::NOZZLE_TEMP),
            bed_temp: int_in(fi, "bed_temp", fbase.bed_temp, limits::BED_TEMP),
            // Not in the panel; kept physical so filament maths stays finite.
            diameter: num(fi, "diameter", fbase.diameter).clamp(1.0, 3.0),
            density: num(fi, "density", fbase.density).max(0.1),
            fan_percent: int_in(fi, "fan_percent", fbase.fan_percent, limits::FAN_PERCENT),
            flow_percent: num_in(fi, "flow_percent", fbase.flow_percent, limits::FLOW_PERCENT),
            price_per_kg: num(fi, "price_per_kg", fbase.price_per_kg).max(0.0),
        }
    };
    let empty = Value::Null;

    let pr = v.get("printer").unwrap_or(&empty);
    let base = Printer::presets().into_iter().next().expect("presets");
    let printer = Printer {
        name: text_of(pr, "name").unwrap_or(base.name.clone()),
        bed_x: num_in(pr, "bed_x", base.bed_x, limits::PLATE_MM),
        bed_y: num_in(pr, "bed_y", base.bed_y, limits::PLATE_MM),
        max_z: num_in(pr, "max_z", base.max_z, limits::PLATE_MM),
        nozzle_diameter: num(pr, "nozzle_diameter", base.nozzle_diameter).clamp(0.1, 2.0),
        retract_length: num_in(
            pr,
            "retract_length",
            base.retract_length,
            limits::RETRACT_MM,
        ),
        // Not in the panel; only kept from dividing the time estimate by zero.
        retract_speed: num(pr, "retract_speed", base.retract_speed).max(1.0),
        purge_mm: num_in(pr, "purge_mm", base.purge_mm, limits::PURGE_MM),
        klipper: flag(pr, "klipper", base.klipper),
        x3g: flag(pr, "x3g", base.x3g),
        // Projects from before two nozzles have one.
        extruders: num(pr, "extruders", 1.0).clamp(1.0, 2.0) as u8,
        retract_min_travel: num_in(
            pr,
            "retract_min_travel",
            base.retract_min_travel,
            limits::RETRACT_MIN_TRAVEL,
        ),
        retract_layer_change: flag(pr, "retract_layer_change", base.retract_layer_change),
        wipe_mm: num_in(pr, "wipe_mm", base.wipe_mm, limits::WIPE_MM),
        tool_change_retract: num_in(
            pr,
            "tool_change_retract",
            base.tool_change_retract,
            limits::TOOL_CHANGE_RETRACT,
        ),
        standby_drop: int_in(pr, "standby_drop", base.standby_drop, limits::STANDBY_DROP),
        primary_nozzle: num(pr, "primary_nozzle", base.primary_nozzle as f32).clamp(0.0, 1.0) as u8,
        nozzle_spacing: num_in(
            pr,
            "nozzle_spacing",
            base.nozzle_spacing,
            limits::NOZZLE_SPACING,
        ),
        // Projects from before this setting: the preset of the same name's value.
        acceleration: num_in(
            pr,
            "acceleration",
            crate::profiles::preset_acceleration(&text_of(pr, "name").unwrap_or_default()),
            limits::ACCELERATION,
        ),
    };

    let filament = filament_from(v.get("filament").unwrap_or(&empty));
    let du = v.get("dual").unwrap_or(&empty);
    let dual = DualNozzle {
        second_filament: du
            .get("second_filament")
            .filter(|f| f.is_object())
            .map(filament_from),
        support_tool: num(du, "support_tool", printer.primary_nozzle as f32).clamp(0.0, 1.0) as u8,
        ditto: flag(du, "ditto", false),
        prime_tower: flag(du, "prime_tower", true),
        tower_at: du.get("tower_at").and_then(|a| a.as_array()).and_then(|a| {
            let c = |k: usize| a.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
            Some([c(0)?, c(1)?])
        }),
        ooze_shield: flag(du, "ooze_shield", false),
        shield_distance: num_in(
            du,
            "shield_distance",
            crate::profiles::DEFAULT_SHIELD_DISTANCE_MM,
            limits::SHIELD_DISTANCE,
        ),
    };

    let st = v.get("settings").unwrap_or(&empty);
    let d = PrintSettings::default();
    let pick = |k: &str| text_of(st, k).unwrap_or_default();
    // Each feature speed: a number, or missing / null for "Auto".
    // Each acceleration: a number, or missing / null for "Auto".
    let mut feature_accels = FeatureAccels::default();
    if let Some(fa) = st.get("feature_accels") {
        for f in AccelFeature::ALL {
            let r = limits::ACCELERATION;
            let v = fa.get(f.key()).and_then(Value::as_f64);
            feature_accels.set(f, v.map(|v| (v as f32).clamp(*r.start(), *r.end())));
        }
    }
    let mut feature_speeds = FeatureSpeeds::default();
    if let Some(fs) = st.get("feature_speeds") {
        for f in SpeedFeature::ALL {
            let r = limits::PRINT_SPEED;
            let v = fs.get(f.key()).and_then(Value::as_f64);
            feature_speeds.set(f, v.map(|v| (v as f32).clamp(*r.start(), *r.end())));
        }
    }
    let settings = PrintSettings {
        layer_height: num_in(st, "layer_height", d.layer_height, limits::LAYER_HEIGHT),
        first_layer_height: num_in(
            st,
            "first_layer_height",
            d.first_layer_height,
            limits::FIRST_LAYER_HEIGHT,
        ),
        walls: int_in(st, "walls", d.walls, limits::WALLS),
        top_layers: int_in(st, "top_layers", d.top_layers, limits::SOLID_LAYERS),
        bottom_layers: int_in(st, "bottom_layers", d.bottom_layers, limits::SOLID_LAYERS),
        infill_percent: num_in(
            st,
            "infill_percent",
            d.infill_percent,
            limits::INFILL_PERCENT,
        ),
        print_speed: num_in(st, "print_speed", d.print_speed, limits::PRINT_SPEED),
        first_layer_speed: num_in(
            st,
            "first_layer_speed",
            d.first_layer_speed,
            limits::FIRST_LAYER_SPEED,
        ),
        travel_speed: num_in(st, "travel_speed", d.travel_speed, limits::TRAVEL_SPEED),
        speeds: feature_speeds,
        accels: feature_accels,
        bridge_flow: num_in(st, "bridge_flow", d.bridge_flow, limits::BRIDGE_FLOW),
        bridge_fan: int_in(st, "bridge_fan", d.bridge_fan, limits::FAN_PERCENT),
        avoid_crossing_walls: flag(st, "avoid_crossing_walls", d.avoid_crossing_walls),
        infill_overlap: num_in(
            st,
            "infill_overlap",
            d.infill_overlap,
            limits::INFILL_OVERLAP,
        ),
        elephant_foot: num_in(st, "elephant_foot", d.elephant_foot, limits::ELEPHANT_FOOT),
        hole_compensation: num_in(
            st,
            "hole_compensation",
            d.hole_compensation,
            limits::HOLE_COMPENSATION,
        ),
        supports: flag(st, "supports", d.supports),
        support_angle: num_in(st, "support_angle", d.support_angle, limits::SUPPORT_ANGLE),
        supports_on_plate_only: flag(st, "supports_on_plate_only", d.supports_on_plate_only),
        support_raft: flag(st, "support_raft", d.support_raft),
        adhesion: Adhesion::ALL
            .into_iter()
            .find(|a| a.label() == pick("adhesion"))
            .unwrap_or(d.adhesion),
        infill_pattern: InfillPattern::ALL
            .into_iter()
            .find(|a| a.label() == pick("infill_pattern"))
            .unwrap_or(d.infill_pattern),
        seam: SeamPosition::ALL
            .into_iter()
            .find(|a| a.label() == pick("seam"))
            .unwrap_or(d.seam),
        ironing: flag(st, "ironing", d.ironing),
        fuzzy_skin: flag(st, "fuzzy_skin", d.fuzzy_skin),
        fuzzy_top: flag(st, "fuzzy_top", d.fuzzy_top),
        fuzzy_top_mm: num_in(st, "fuzzy_top_mm", d.fuzzy_top_mm, limits::FUZZY_TOP_MM),
        vase_mode: flag(st, "vase_mode", d.vase_mode),
        z_hop: num_in(st, "z_hop", d.z_hop, limits::Z_HOP),
        min_layer_time: num_in(
            st,
            "min_layer_time",
            d.min_layer_time,
            limits::MIN_LAYER_TIME,
        ),
    };
    let quality = v
        .get("quality")
        .and_then(|q| q.as_str())
        .and_then(Quality::parse);

    let mut objects = Vec::new();
    for (i, o) in v
        .get("objects")
        .and_then(|o| o.as_array())
        .into_iter()
        .flatten()
        .enumerate()
    {
        let data = o.get("mesh").and_then(|m| m.as_str()).unwrap_or("");
        let mesh = base64_decode(data)
            .and_then(|b| mesh_from_bytes(&b))
            .map(|triangles| Mesh { triangles })
            .filter(Mesh::is_finite)
            .ok_or_else(|| format!("Object {} in the project is damaged.", i + 1))?;
        if mesh.triangles.is_empty() {
            continue;
        }
        let mut obj = PlacedObject::new(
            text_of(o, "name").unwrap_or_else(|| format!("Object {}", i + 1)),
            mesh,
        );
        obj.x = num(o, "x", 0.0);
        obj.y = num(o, "y", 0.0);
        if let Some(r) = o.get("rotation").and_then(|r| r.as_array()) {
            let c = |k: usize| r.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
            obj.rotation = Vec3::new(c(0), c(1), c(2));
        }
        obj.scale_percent = num(o, "scale_percent", 100.0).clamp(1.0, 10_000.0);
        obj.extruder = num(o, "extruder", printer.primary_nozzle as f32).clamp(0.0, 1.0) as u8;
        objects.push(obj);
    }

    let device = v.get("device").and_then(|d| {
        let mut p = FoundPrinter::from_json(d)?;
        // A project names its printer even when the file did not.
        if text_of(d, "name").is_none() {
            p.name = format!("Printer at {}", p.host);
        }
        Some(p)
    });

    Ok(Project {
        printer,
        filament,
        settings,
        quality,
        objects,
        device,
        dual,
    })
}

fn filament_json(f: &Filament) -> Value {
    json!({
        "name": f.name,
        "nozzle_temp": f.nozzle_temp,
        "bed_temp": f.bed_temp,
        "diameter": f.diameter,
        "density": f.density,
        "fan_percent": f.fan_percent,
        "flow_percent": f.flow_percent,
        "price_per_kg": f.price_per_kg,
    })
}

pub fn save(path: &Path, p: &Project) -> Result<(), String> {
    crate::fsutil::write_atomically(path, to_json(p).as_bytes())
        .map_err(|e| format!("Could not save the project: {e}"))
}

#[cfg(test)]
pub fn load(path: &Path) -> Result<Project, String> {
    open(path).map(|o| o.project)
}

/// Like `load`, keeping the file's format version (to warn about newer files).
pub fn open(path: &Path) -> Result<Opened, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Could not open the project: {e}"))?;
    opened_from_json(&text)
}

pub fn is_project(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// The path with ".printcraft" added when it lacks it. Appended, never replacing what
/// follows a dot: "v1.2" becomes "v1.2.printcraft", not "v1.printcraft".
pub fn with_project_extension(path: PathBuf) -> PathBuf {
    if is_project(&path) {
        return path;
    }
    let mut name = path.into_os_string();
    name.push(".");
    name.push(EXTENSION);
    PathBuf::from(name)
}

fn mesh_bytes(tris: &[Triangle]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tris.len() * 36);
    for t in tris {
        for v in t {
            for c in [v.x, v.y, v.z] {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
    out
}

fn mesh_from_bytes(b: &[u8]) -> Option<Vec<Triangle>> {
    if b.len() % 36 != 0 {
        return None;
    }
    // Each triangle: three corners of three little-endian f32s (36 bytes).
    let (triangles, _) = b.as_chunks::<36>();
    Some(
        triangles
            .iter()
            .map(|t| {
                let f = |o: usize| f32::from_le_bytes([t[o], t[o + 1], t[o + 2], t[o + 3]]);
                let v = |k: usize| Vec3::new(f(k * 12), f(k * 12 + 4), f(k * 12 + 8));
                [v(0), v(1), v(2)]
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SampleShape;
    use crate::printer::Api;

    fn sample() -> Project {
        let mut cube = PlacedObject::new("cube", SampleShape::Cube.mesh());
        cube.x = 30.0;
        cube.y = -12.5;
        cube.rotation = Vec3::new(0.0, 0.0, 45.0);
        cube.scale_percent = 150.0;
        cube.extruder = 1;
        let tube = PlacedObject::new("tube", SampleShape::Tube.mesh());
        let mut printer = Printer::presets()
            .into_iter()
            .find(|p| p.name.contains("KE"))
            .unwrap();
        printer.purge_mm = 42.0;
        printer.primary_nozzle = 1;
        printer.nozzle_spacing = 34.5;
        let mut filament = Filament::presets()
            .into_iter()
            .find(|f| f.name == "PETG")
            .unwrap();
        filament.nozzle_temp = 245;
        let settings = PrintSettings {
            layer_height: 0.16,
            supports: true,
            fuzzy_top: true,
            seam: SeamPosition::Random,
            infill_pattern: InfillPattern::Triangles,
            adhesion: Adhesion::Brim,
            ..PrintSettings::default()
        };
        Project {
            printer,
            filament,
            settings,
            quality: None,
            objects: vec![cube, tube],
            device: Some(FoundPrinter {
                host: "192.168.1.116".into(),
                name: "Ender3V3KE-A654".into(),
                model: "Creality printer".into(),
                api: Some(Api::CrealityLan),
                login: Some(("maker".into(), "secret".into())),
                baud: None,
            }),
            dual: DualNozzle {
                second_filament: Filament::presets().into_iter().find(|f| f.name == "PLA"),
                support_tool: 1,
                ditto: false,
                prime_tower: false,
                tower_at: Some([30.0, 120.0]),
                ooze_shield: true,
                shield_distance: 4.5,
            },
        }
    }

    #[test]
    fn round_trips_everything_except_passwords() {
        let p = sample();
        let text = to_json(&p);
        assert!(!text.contains("secret"), "passwords are never saved");
        let back = from_json(&text).unwrap();
        assert_eq!(back.printer, p.printer);
        assert_eq!(back.filament, p.filament);
        assert_eq!(back.settings, p.settings);
        assert_eq!(back.dual, p.dual, "two-nozzle choices");
        assert_eq!(back.printer.nozzle_spacing, 34.5, "the nozzle spacing");
        assert_eq!(back.objects[0].extruder, 1, "each part's nozzle");
        assert_eq!(
            back.printer.primary_nozzle, 1,
            "the printer's primary nozzle"
        );
        // A part saved without a nozzle (an older file) goes to the printer's primary.
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["objects"][0].as_object_mut().unwrap().remove("extruder");
        let older = v.to_string();
        assert_eq!(from_json(&older).unwrap().objects[0].extruder, 1);
        assert_eq!(back.quality, None);
        assert_eq!(back.objects.len(), 2);
        let (a, b) = (&back.objects[0], &p.objects[0]);
        assert_eq!(
            (a.name.as_str(), a.x, a.y, a.rotation, a.scale_percent),
            (b.name.as_str(), b.x, b.y, b.rotation, b.scale_percent)
        );
        assert_eq!(
            a.mesh.triangles, b.mesh.triangles,
            "shapes are stored exactly"
        );
        assert_eq!(
            back.objects[1].mesh.triangles.len(),
            p.objects[1].mesh.triangles.len()
        );
        let d = back.device.unwrap();
        assert_eq!(
            (d.host.as_str(), d.api, d.login),
            ("192.168.1.116", Some(Api::CrealityLan), None)
        );
    }

    #[test]
    fn saves_and_loads_a_file() {
        let dir = std::env::temp_dir().join(format!("printcraft-project-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("my plate.printcraft");
        save(&path, &sample()).unwrap();
        assert!(is_project(&path));
        let back = load(&path).unwrap();
        assert_eq!(back.objects.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_or_partial_files_still_open_with_defaults() {
        let p = from_json(r#"{"format":"printcraft-project","version":1,"settings":{"walls":3,"seam":"Nowhere"}}"#).unwrap();
        assert_eq!(p.settings.walls, 3);
        assert_eq!(
            p.settings.seam,
            PrintSettings::default().seam,
            "unknown names fall back"
        );
        assert!(p.objects.is_empty() && p.device.is_none());
        assert!(from_json("{}").is_err(), "not a project");
        assert!(from_json("hello").is_err());
    }

    #[test]
    fn test_project_feature_speeds_round_trip_and_clamp() {
        let mut p = sample();
        p.settings.speeds.set(SpeedFeature::OuterWall, Some(33.0));
        p.settings.speeds.set(SpeedFeature::Support, Some(70.0));
        let back = from_json(&to_json(&p)).unwrap();
        assert_eq!(back.settings.speeds, p.settings.speeds);
        assert_eq!(back.settings.speeds.get(SpeedFeature::InnerWall), None);
        // Out-of-range values are held to the panel's limits; an old file has none.
        let bad = from_json(
            r#"{"format":"printcraft-project","version":1,"settings":{"feature_speeds":{"outer_wall":9999,"top_surface":null}}}"#,
        )
        .unwrap();
        let max = *limits::PRINT_SPEED.end();
        assert_eq!(bad.settings.speeds.get(SpeedFeature::OuterWall), Some(max));
        assert_eq!(bad.settings.speeds.get(SpeedFeature::TopSurface), None);
        let old =
            from_json(r#"{"format":"printcraft-project","version":1,"settings":{}}"#).unwrap();
        assert_eq!(old.settings.speeds, FeatureSpeeds::default());
    }

    #[test]
    fn test_project_accelerations_round_trip_and_default_by_printer() {
        let mut p = sample();
        p.printer.acceleration = 3456.0;
        p.settings.accels.set(AccelFeature::Infill, Some(4200.0));
        p.settings.bridge_flow = 88.0;
        p.settings.bridge_fan = 70;
        p.settings.avoid_crossing_walls = true;
        p.settings.infill_overlap = 25.0;
        p.settings.elephant_foot = 0.15;
        p.settings.hole_compensation = 0.1;
        p.filament.flow_percent = 96.5;
        let back = from_json(&to_json(&p)).unwrap();
        assert_eq!(back.printer.acceleration, 3456.0);
        assert_eq!(back.settings.accels, p.settings.accels);
        assert_eq!(
            (back.settings.bridge_flow, back.settings.bridge_fan),
            (88.0, 70)
        );
        assert!(back.settings.avoid_crossing_walls);
        assert_eq!(
            (
                back.settings.infill_overlap,
                back.settings.elephant_foot,
                back.settings.hole_compensation,
                back.filament.flow_percent
            ),
            (25.0, 0.15, 0.1, 96.5)
        );
        // A project from before accelerations: the preset of the same name's value.
        let old = from_json(
            r#"{"format":"printcraft-project","version":1,"printer":{"name":"Creality Ender-3 V3 KE"},
                "settings":{"feature_accels":{"travel":99999}}}"#,
        )
        .unwrap();
        assert_eq!(old.printer.acceleration, 8000.0);
        let max = *limits::ACCELERATION.end();
        assert_eq!(old.settings.accels.get(AccelFeature::Travel), Some(max));
        assert_eq!(old.settings.accels.get(AccelFeature::OuterWall), None);
    }

    #[test]
    fn base64_round_trips() {
        for data in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0u8, 255, 128, 7][..],
        ] {
            assert_eq!(
                base64_decode(&crate::thumbnail::base64(data)).unwrap(),
                data
            );
        }
        assert!(base64_decode("abc").is_none());
        assert!(base64_decode("a=bc").is_none());
    }

    #[test]
    fn out_of_range_values_are_held_to_the_settings_limits() {
        let p = from_json(
            r#"{"format":"printcraft-project","version":1,
            "printer":{"bed_x":5,"retract_speed":0},
            "filament":{"nozzle_temp":1000,"bed_temp":4294967297},
            "settings":{"walls":4000000,"print_speed":0,"travel_speed":-3}}"#,
        )
        .unwrap();
        assert_eq!(p.printer.bed_x, *limits::PLATE_MM.start());
        assert!(p.printer.retract_speed >= 1.0);
        assert_eq!(p.filament.nozzle_temp, *limits::NOZZLE_TEMP.end());
        assert_eq!(p.filament.bed_temp, *limits::BED_TEMP.end(), "no u32 wrap");
        assert_eq!(p.settings.walls, *limits::WALLS.end());
        assert_eq!(p.settings.print_speed, *limits::PRINT_SPEED.start());
        assert_eq!(p.settings.travel_speed, *limits::TRAVEL_SPEED.start());
    }

    #[test]
    fn save_as_appends_the_extension_instead_of_replacing_a_dotted_name() {
        let p = |s: &str| with_project_extension(PathBuf::from(s));
        assert_eq!(p("/x/v1.2"), PathBuf::from("/x/v1.2.printcraft"));
        assert_eq!(p("/x/plate"), PathBuf::from("/x/plate.printcraft"));
        assert_eq!(
            p("/x/plate.PrintCraft"),
            PathBuf::from("/x/plate.PrintCraft")
        );
    }

    #[test]
    fn flags_a_file_from_a_newer_version() {
        let open = |v: &str| {
            opened_from_json(&format!(r#"{{"format":"printcraft-project"{v}}}"#)).unwrap()
        };
        assert!(open(&format!(r#","version":{}"#, VERSION + 1)).is_from_newer_version());
        assert!(!open(&format!(r#","version":{VERSION}"#)).is_from_newer_version());
        assert!(
            !open("").is_from_newer_version(),
            "the first files had no version"
        );
    }

    #[test]
    fn rejects_malformed_base64_padding_without_crashing() {
        assert!(base64_decode("====").is_none(), "all padding");
        assert!(base64_decode("a===").is_none(), "three pad characters");
        assert!(base64_decode("Zg==Zg==").is_none(), "padding mid-string");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
    }

    #[test]
    fn a_project_with_an_all_padding_mesh_is_damaged_not_a_crash() {
        let text =
            r#"{"format":"printcraft-project","version":1,"objects":[{"name":"x","mesh":"===="}]}"#;
        let err = from_json(text).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
    }

    #[test]
    fn a_project_mesh_with_infinite_coordinates_is_damaged() {
        let mut bytes = Vec::new();
        for k in 0..9 {
            let v = if k == 0 { f32::INFINITY } else { k as f32 };
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let text = format!(
            r#"{{"format":"printcraft-project","version":1,"objects":[{{"name":"x","mesh":"{}"}}]}}"#,
            crate::thumbnail::base64(&bytes)
        );
        let err = from_json(&text).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
    }
}
