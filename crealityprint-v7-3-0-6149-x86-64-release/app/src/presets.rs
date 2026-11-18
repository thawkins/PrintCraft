//! The person's own presets: named filaments and named process settings, kept between
//! runs in `presets.json` in the settings folder (see `fsutil::settings_dir`), and
//! exported to or imported from a JSON file of the same shape:
//!
//! ```text
//! {"printcraft_presets": 1,
//!  "filaments": [{"name": "Silk PLA", "nozzle_temp": 215, ...}],
//!  "processes": [{"name": "Strong", "settings": { ...project settings block... }}]}
//! ```
//!
//! Filaments use the project file's filament block and processes its settings block, so
//! values are read the same way: anything missing takes its default and everything is
//! held to the panel's limits. Names are unique within each list; saving or importing a
//! preset with an existing name replaces it. The built-in filaments' names are kept for
//! them.

use crate::profiles::{Filament, PrintSettings};
use serde_json::{json, Value};
use std::path::PathBuf;

/// The file format's version, written as `printcraft_presets`.
const FORMAT: u64 = 1;

/// A named set of process (print) settings.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessPreset {
    pub name: String,
    pub settings: PrintSettings,
}

#[derive(Debug, Default)]
pub struct UserPresets {
    /// Where they are saved; None keeps them in memory only (tests, check pictures).
    path: Option<PathBuf>,
    pub filaments: Vec<Filament>,
    pub processes: Vec<ProcessPreset>,
}

impl UserPresets {
    /// Where this user's presets live, if a settings folder can be found.
    pub fn default_path() -> Option<PathBuf> {
        Some(crate::fsutil::settings_dir()?.join("presets.json"))
    }

    /// Reads the presets; a missing or damaged file gives none. With no path, nothing is
    /// ever read or written.
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut p = Self {
            path,
            ..Self::default()
        };
        if let Some(text) = p
            .path
            .as_deref()
            .and_then(|f| std::fs::read_to_string(f).ok())
        {
            // A damaged file is left alone and starts an empty list.
            let _ = p.import_json(&text); // INTENTIONAL: a damaged file means no presets
        }
        p
    }

    /// Writes the presets to their file (nothing without one).
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::fsutil::write_atomically(path, self.export_json().as_bytes())
            .map_err(|e| format!("Could not save the presets: {e}"))
    }

    /// Adds `f` as a preset under its name, replacing one of the same name. A built-in
    /// filament's name, or an empty one, is refused.
    pub fn save_filament(&mut self, f: &Filament) -> Result<(), String> {
        let name = check_name(&f.name)?;
        if Filament::presets()
            .iter()
            .any(|b| b.name.eq_ignore_ascii_case(&name))
        {
            return Err(format!(
                "\"{name}\" is a built-in filament; pick another name."
            ));
        }
        let f = Filament { name, ..f.clone() };
        match self.filaments.iter_mut().find(|x| x.name == f.name) {
            Some(slot) => *slot = f,
            None => self.filaments.push(f),
        }
        self.save()
    }

    /// Adds `settings` as a process preset called `name`, replacing one of the same name.
    pub fn save_process(&mut self, name: &str, settings: &PrintSettings) -> Result<(), String> {
        let p = ProcessPreset {
            name: check_name(name)?,
            settings: settings.clone(),
        };
        match self.processes.iter_mut().find(|x| x.name == p.name) {
            Some(slot) => *slot = p,
            None => self.processes.push(p),
        }
        self.save()
    }

    pub fn delete_filament(&mut self, name: &str) -> Result<(), String> {
        self.filaments.retain(|f| f.name != name);
        self.save()
    }

    pub fn delete_process(&mut self, name: &str) -> Result<(), String> {
        self.processes.retain(|p| p.name != name);
        self.save()
    }

    /// Every preset as JSON (the file's and the export's text).
    pub fn export_json(&self) -> String {
        let v = json!({
            "printcraft_presets": FORMAT,
            "filaments": self.filaments.iter().map(crate::project::filament_json).collect::<Vec<_>>(),
            "processes": self.processes.iter().map(|p| json!({
                "name": p.name,
                "settings": crate::project::settings_json(&p.settings),
            })).collect::<Vec<_>>(),
        });
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }

    /// Adds the presets in `text` (an export), replacing ones of the same name; built-in
    /// filament names are skipped. Returns how many filaments and processes were taken.
    /// Nothing changes when the text isn't a presets file.
    pub fn import_json(&mut self, text: &str) -> Result<(usize, usize), String> {
        let v: Value = serde_json::from_str(text).map_err(|_| "That isn't a presets file.")?;
        let version = v.get("printcraft_presets").and_then(|x| x.as_u64());
        match version {
            Some(n) if n > FORMAT => {
                return Err("The presets file is from a newer PrintCraft.".into());
            }
            Some(_) => {}
            None => return Err("That isn't a presets file.".into()),
        }
        let list = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default()
        };
        let built_in: Vec<String> = Filament::presets().into_iter().map(|f| f.name).collect();
        let mut taken = (0, 0);
        for fv in list("filaments") {
            let f = crate::project::filament_from_json(&fv);
            let Ok(name) = check_name(&f.name) else {
                continue;
            };
            if built_in.iter().any(|b| b.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let f = Filament { name, ..f };
            match self.filaments.iter_mut().find(|x| x.name == f.name) {
                Some(slot) => *slot = f,
                None => self.filaments.push(f),
            }
            taken.0 += 1;
        }
        for pv in list("processes") {
            let Some(Ok(name)) = pv.get("name").and_then(|n| n.as_str()).map(check_name) else {
                continue;
            };
            let p = ProcessPreset {
                name,
                settings: crate::project::settings_from_json(
                    pv.get("settings").unwrap_or(&Value::Null),
                ),
            };
            match self.processes.iter_mut().find(|x| x.name == p.name) {
                Some(slot) => *slot = p,
                None => self.processes.push(p),
            }
            taken.1 += 1;
        }
        Ok(taken)
    }
}

/// A preset name trimmed; refused when empty or longer than 60 characters.
fn check_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        Err("Give the preset a name.".into())
    } else if n.chars().count() > 60 {
        Err("That name is too long (60 characters at most).".into())
    } else {
        Ok(n.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh scratch file under `target/temp` (tests never write in the repository).
    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/temp")
            .join(format!("presets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("presets.json")
    }

    fn silk() -> Filament {
        Filament {
            name: "Silk PLA".into(),
            nozzle_temp: 215,
            bed_temp: 55,
            fan_percent: 80,
            flow_percent: 97.5,
            price_per_kg: 28.0,
            ..Filament::presets()[0].clone()
        }
    }

    fn strong() -> PrintSettings {
        PrintSettings {
            walls: 4,
            infill_percent: 40.0,
            label_objects: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_presets_save_and_reload() {
        let path = scratch("reload");
        let mut p = UserPresets::load(Some(path.clone()));
        assert!(p.filaments.is_empty() && p.processes.is_empty());
        p.save_filament(&silk()).unwrap();
        p.save_process("Strong", &strong()).unwrap();
        let back = UserPresets::load(Some(path.clone()));
        assert_eq!(back.filaments, vec![silk()]);
        assert_eq!(
            back.processes,
            vec![ProcessPreset {
                name: "Strong".into(),
                settings: strong()
            }]
        );
        // Saving under the same name replaces; deleting removes.
        let mut p = back;
        let hotter = Filament {
            nozzle_temp: 225,
            ..silk()
        };
        p.save_filament(&hotter).unwrap();
        p.delete_process("Strong").unwrap();
        let back = UserPresets::load(Some(path));
        assert_eq!(back.filaments, vec![hotter]);
        assert!(back.processes.is_empty());
    }

    #[test]
    fn test_presets_export_and_import_round_trip() {
        let mut a = UserPresets::load(None);
        a.save_filament(&silk()).unwrap();
        a.save_process("Strong", &strong()).unwrap();
        a.save_process("Draft fast", &PrintSettings::default())
            .unwrap();
        let text = a.export_json();
        let mut b = UserPresets::load(None);
        assert_eq!(b.import_json(&text), Ok((1, 2)));
        assert_eq!(b.filaments, a.filaments);
        assert_eq!(b.processes, a.processes);
        // Importing again replaces rather than duplicates.
        assert_eq!(b.import_json(&text), Ok((1, 2)));
        assert_eq!(b.processes.len(), 2);
    }

    #[test]
    fn test_presets_refuse_bad_files_and_names() {
        let mut p = UserPresets::load(None);
        assert!(p.import_json("not json").is_err());
        assert!(
            p.import_json("{\"filaments\": []}").is_err(),
            "no format marker"
        );
        assert!(
            p.import_json("{\"printcraft_presets\": 99}").is_err(),
            "newer"
        );
        assert!(p
            .save_filament(&Filament {
                name: "  ".into(),
                ..silk()
            })
            .is_err());
        assert!(p
            .save_filament(&Filament {
                name: "pla".into(),
                ..silk()
            })
            .is_err());
        assert!(p.save_process("", &strong()).is_err());
        // Out-of-range values are held to the panel's limits; built-in names are skipped.
        let text = r#"{"printcraft_presets": 1,
            "filaments": [{"name": "Hot", "nozzle_temp": 5000, "flow_percent": 1},
                          {"name": "PETG", "nozzle_temp": 200}],
            "processes": [{"name": "Odd", "settings": {"walls": 9999}}, {"settings": {}}]}"#;
        assert_eq!(p.import_json(text), Ok((1, 1)));
        let hot = &p.filaments[0];
        assert_eq!(hot.nozzle_temp, *crate::profiles::limits::NOZZLE_TEMP.end());
        assert_eq!(
            hot.flow_percent,
            *crate::profiles::limits::FLOW_PERCENT.start()
        );
        assert!(p.processes[0].settings.walls < 9999);
        // A damaged file loads as no presets.
        let path = scratch("damaged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{broken").unwrap();
        assert!(UserPresets::load(Some(path)).filaments.is_empty());
    }

    #[test]
    fn test_presets_built_in_filaments_include_the_generic_five() {
        let names: Vec<String> = Filament::presets().into_iter().map(|f| f.name).collect();
        for want in ["PLA", "PETG", "ABS", "TPU (flexible)", "ASA"] {
            assert!(names.iter().any(|n| n == want), "{want} missing: {names:?}");
        }
    }
}
