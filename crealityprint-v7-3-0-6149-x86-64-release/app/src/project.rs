//! PrintCraft projects (`.printcraft` files): everything needed to carry on later.
//! * every object on the plate: its name, its shape (stored in the file, so the
//!   original STL/3MF can move or vanish), position, rotation and scale
//! * the process settings, quality preset and filament
//! * the printer profile with all its edited values
//! * the printer device it was connected to (address or USB port, never a password)
//!
//! The file is JSON. Missing or unknown values fall back to defaults, so files from
//! older or newer versions still open.

use crate::geometry::Vec3;
use crate::model::{Mesh, PlacedObject, Triangle};
use crate::printer::known::{api_from, api_name};
use crate::printer::FoundPrinter;
use crate::profiles::{
    Adhesion, Filament, InfillPattern, PrintSettings, Printer, Quality, SeamPosition,
};
use serde_json::{json, Value};
use std::path::Path;

pub const EXTENSION: &str = "printcraft";
const FORMAT: &str = "printcraft-project";
const VERSION: u64 = 1;

#[derive(Clone, Debug)]
pub struct Project {
    pub printer: Printer,
    pub filament: Filament,
    pub settings: PrintSettings,
    pub quality: Option<Quality>,
    pub objects: Vec<PlacedObject>,
    /// The printer device in use when the project was saved.
    pub device: Option<FoundPrinter>,
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
                "triangles": o.mesh.triangles.len(),
                "mesh": crate::thumbnail::base64(&mesh_bytes(&o.mesh.triangles)),
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
        },
        "filament": {
            "name": p.filament.name,
            "nozzle_temp": p.filament.nozzle_temp,
            "bed_temp": p.filament.bed_temp,
            "diameter": p.filament.diameter,
            "density": p.filament.density,
            "fan_percent": p.filament.fan_percent,
            "price_per_kg": p.filament.price_per_kg,
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
            "supports": s.supports,
            "support_angle": s.support_angle,
            "supports_on_plate_only": s.supports_on_plate_only,
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
        "device": p.device.as_ref().map(|d| json!({
            "host": d.host,
            "name": d.name,
            "model": d.model,
            "api": d.api.map(api_name),
            "baud": d.baud,
        })),
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Reads a project. Values that are missing or of the wrong kind keep their defaults.
pub fn from_json(text: &str) -> Result<Project, String> {
    let v: Value = serde_json::from_str(text)
        .map_err(|_| "This isn't a PrintCraft project file.".to_string())?;
    if v.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
        return Err("This isn't a PrintCraft project file.".into());
    }
    let num = |o: &Value, k: &str, d: f32| {
        o.get(k)
            .and_then(|x| x.as_f64())
            .map(|x| x as f32)
            .unwrap_or(d)
    };
    let int = |o: &Value, k: &str, d: u32| {
        o.get(k)
            .and_then(|x| x.as_u64())
            .map(|x| x as u32)
            .unwrap_or(d)
    };
    let flag = |o: &Value, k: &str, d: bool| o.get(k).and_then(|x| x.as_bool()).unwrap_or(d);
    let text_of = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let empty = Value::Null;

    let pr = v.get("printer").unwrap_or(&empty);
    let base = Printer::presets().into_iter().next().expect("presets");
    let printer = Printer {
        name: text_of(pr, "name").unwrap_or(base.name.clone()),
        bed_x: num(pr, "bed_x", base.bed_x).clamp(10.0, 2000.0),
        bed_y: num(pr, "bed_y", base.bed_y).clamp(10.0, 2000.0),
        max_z: num(pr, "max_z", base.max_z).clamp(10.0, 2000.0),
        nozzle_diameter: num(pr, "nozzle_diameter", base.nozzle_diameter).clamp(0.1, 2.0),
        retract_length: num(pr, "retract_length", base.retract_length),
        retract_speed: num(pr, "retract_speed", base.retract_speed),
        purge_mm: num(pr, "purge_mm", base.purge_mm),
        klipper: flag(pr, "klipper", base.klipper),
    };

    let fi = v.get("filament").unwrap_or(&empty);
    let fbase = Filament::presets().into_iter().next().expect("presets");
    let filament = Filament {
        name: text_of(fi, "name").unwrap_or(fbase.name.clone()),
        nozzle_temp: int(fi, "nozzle_temp", fbase.nozzle_temp),
        bed_temp: int(fi, "bed_temp", fbase.bed_temp),
        diameter: num(fi, "diameter", fbase.diameter),
        density: num(fi, "density", fbase.density),
        fan_percent: int(fi, "fan_percent", fbase.fan_percent).min(100),
        price_per_kg: num(fi, "price_per_kg", fbase.price_per_kg),
    };

    let st = v.get("settings").unwrap_or(&empty);
    let d = PrintSettings::default();
    let pick = |k: &str| text_of(st, k).unwrap_or_default();
    let settings = PrintSettings {
        layer_height: num(st, "layer_height", d.layer_height),
        first_layer_height: num(st, "first_layer_height", d.first_layer_height),
        walls: int(st, "walls", d.walls).max(1),
        top_layers: int(st, "top_layers", d.top_layers),
        bottom_layers: int(st, "bottom_layers", d.bottom_layers),
        infill_percent: num(st, "infill_percent", d.infill_percent),
        print_speed: num(st, "print_speed", d.print_speed),
        first_layer_speed: num(st, "first_layer_speed", d.first_layer_speed),
        travel_speed: num(st, "travel_speed", d.travel_speed),
        supports: flag(st, "supports", d.supports),
        support_angle: num(st, "support_angle", d.support_angle),
        supports_on_plate_only: flag(st, "supports_on_plate_only", d.supports_on_plate_only),
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
        fuzzy_top_mm: num(st, "fuzzy_top_mm", d.fuzzy_top_mm),
        vase_mode: flag(st, "vase_mode", d.vase_mode),
        z_hop: num(st, "z_hop", d.z_hop),
        min_layer_time: num(st, "min_layer_time", d.min_layer_time),
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
        objects.push(obj);
    }

    let device = v.get("device").filter(|d| !d.is_null()).and_then(|d| {
        let host = text_of(d, "host")?;
        Some(FoundPrinter {
            name: text_of(d, "name").unwrap_or_else(|| format!("Printer at {host}")),
            model: text_of(d, "model").unwrap_or_default(),
            api: text_of(d, "api").as_deref().and_then(api_from),
            baud: d.get("baud").and_then(|b| b.as_u64()).map(|b| b as u32),
            login: None,
            host,
        })
    });

    Ok(Project {
        printer,
        filament,
        settings,
        quality,
        objects,
        device,
    })
}

pub fn save(path: &Path, p: &Project) -> Result<(), String> {
    let tmp = path.with_extension(format!("{EXTENSION}.tmp"));
    std::fs::write(&tmp, to_json(p)).map_err(|e| format!("Could not save the project: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Could not save the project: {e}"))
}

pub fn load(path: &Path) -> Result<Project, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Could not open the project: {e}"))?;
    from_json(&text)
}

pub fn is_project(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
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
    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    Some(
        b.chunks_exact(36)
            .enumerate()
            .map(|(i, _)| {
                let o = i * 36;
                let v = |k: usize| Vec3::new(f(o + k * 12), f(o + k * 12 + 4), f(o + k * 12 + 8));
                [v(0), v(1), v(2)]
            })
            .collect(),
    )
}

/// Standard base64 back to bytes (whitespace ignored); None when it isn't base64.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if clean.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    let last = clean.len() / 4;
    for (index, chunk) in clean.chunks(4).enumerate() {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        // Padding is at most "==" and only ends the final block.
        if pad > 2 || (pad > 0 && index + 1 != last) {
            return None;
        }
        let mut n = 0u32;
        for (k, c) in chunk.iter().enumerate() {
            let d = if *c == b'=' { 0 } else { val(*c)? };
            if *c == b'=' && k < 4 - pad {
                return None;
            }
            n = n << 6 | d;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
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
        let tube = PlacedObject::new("tube", SampleShape::Tube.mesh());
        let mut printer = Printer::presets()
            .into_iter()
            .find(|p| p.name.contains("KE"))
            .unwrap();
        printer.purge_mm = 42.0;
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
