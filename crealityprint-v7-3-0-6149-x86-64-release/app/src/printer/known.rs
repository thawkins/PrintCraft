//! Printers this app has found or connected to, kept between runs so they can be picked
//! again without searching. Stored as JSON in the user's settings folder:
//! Linux `$XDG_CONFIG_HOME/printcraft/known_printers.json` (or `~/.config/…`),
//! macOS `~/Library/Application Support/printcraft/…`, Windows `%APPDATA%\printcraft\…`.
//! PrusaLink passwords are never stored.

use super::FoundPrinter;
// The tests below name `Api` through `use super::*`.
#[cfg(test)]
use super::Api;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct KnownPrinter {
    pub printer: FoundPrinter,
    /// Seconds since 1970 when it was last found or connected.
    pub last_seen: i64,
}

#[derive(Debug, Default)]
pub struct KnownPrinters {
    path: Option<PathBuf>,
    /// Shows the saved list but never changes the file (used while taking screenshots).
    read_only: bool,
    pub list: Vec<KnownPrinter>,
}

impl KnownPrinters {
    /// Where the list lives for this user, if a settings folder can be found.
    pub fn default_path() -> Option<PathBuf> {
        Some(crate::fsutil::settings_dir()?.join("known_printers.json"))
    }

    /// Reads the list; a missing or damaged file gives an empty list. With no path,
    /// nothing is ever read or written (used while taking screenshots).
    pub fn load(path: Option<PathBuf>) -> Self {
        let list = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| parse(&v))
            .unwrap_or_default();
        Self {
            path,
            read_only: false,
            list,
        }
    }

    /// Reads the list but never writes it back.
    pub fn load_read_only(path: Option<PathBuf>) -> Self {
        Self {
            read_only: true,
            ..Self::load(path)
        }
    }

    /// Adds or updates a printer (matched by its address or port) and moves it to the
    /// top. The password is dropped. Returns true when something changed.
    pub fn remember(&mut self, p: &FoundPrinter, now: i64) -> bool {
        let mut clean = p.clone();
        clean.login = None;
        if let Some(i) = self.list.iter().position(|k| k.printer.host == p.host) {
            let old = self.list.remove(i);
            // Keep a better name or model than "Printer at …" if the new one is generic.
            if clean.name.starts_with("Printer at ") && !old.printer.name.starts_with("Printer at ")
            {
                clean.name = old.printer.name.clone();
            }
            if clean.model.is_empty() {
                clean.model = old.printer.model.clone();
            }
            if clean.api.is_none() {
                clean.api = old.printer.api;
            }
            let changed = old.printer != clean;
            self.list.insert(
                0,
                KnownPrinter {
                    printer: clean,
                    last_seen: now,
                },
            );
            return changed || i != 0;
        }
        self.list.insert(
            0,
            KnownPrinter {
                printer: clean,
                last_seen: now,
            },
        );
        true
    }

    pub fn forget(&mut self, host: &str) {
        self.list.retain(|k| k.printer.host != host);
    }

    /// Writes the list (via a temporary file, so a crash never leaves half a file).
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = self.path.as_ref().filter(|_| !self.read_only) else {
            return Ok(());
        };
        crate::fsutil::write_atomically(path, to_json(&self.list).as_bytes())
            .map_err(|e| e.to_string())
    }
}

fn parse(v: &Value) -> Vec<KnownPrinter> {
    v.get("printers")
        .and_then(|p| p.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| {
                    let printer = FoundPrinter::from_json(i).filter(|p| !p.host.is_empty())?;
                    Some(KnownPrinter {
                        printer,
                        last_seen: i.get("last_seen").and_then(|t| t.as_i64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn to_json(list: &[KnownPrinter]) -> String {
    let printers: Vec<Value> = list
        .iter()
        .map(|k| {
            let mut v = k.printer.to_json();
            v["last_seen"] = json!(k.last_seen);
            v
        })
        .collect();
    serde_json::to_string_pretty(&json!({"version": 1, "printers": printers})).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("printcraft-known-{tag}-{}", std::process::id()))
            .join("known_printers.json")
    }

    #[test]
    fn remembers_printers_between_runs_without_passwords() {
        let path = temp_file("save");
        let mut known = KnownPrinters::load(Some(path.clone()));
        assert!(known.list.is_empty());
        let ke = FoundPrinter {
            host: "192.168.1.116".into(),
            name: "Ender3V3KE-A654".into(),
            model: "F005".into(),
            api: Some(Api::CrealityLan),
            ..Default::default()
        };
        let prusa = FoundPrinter {
            host: "192.168.1.50".into(),
            name: "prusa-mk4".into(),
            api: Some(Api::PrusaLink),
            login: Some(("maker".into(), "secret".into())),
            ..Default::default()
        };
        assert!(known.remember(&ke, 100));
        assert!(known.remember(&prusa, 200));
        known.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret"), "passwords are never written");

        let again = KnownPrinters::load(Some(path.clone()));
        assert_eq!(again.list.len(), 2);
        assert_eq!(again.list[0].printer.host, "192.168.1.50", "newest first");
        assert_eq!(again.list[1].printer, ke);
        assert_eq!(again.list[1].last_seen, 100);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn updates_instead_of_duplicating_and_keeps_good_names() {
        let mut known = KnownPrinters::load(None);
        let named = FoundPrinter {
            host: "10.0.0.5".into(),
            name: "Ender3V3KE-A654".into(),
            api: Some(Api::CrealityLan),
            ..Default::default()
        };
        known.remember(&named, 1);
        // Typed in by hand later: same address, generic name, kind not known yet.
        let typed = FoundPrinter {
            host: "10.0.0.5".into(),
            name: "Printer at 10.0.0.5".into(),
            ..Default::default()
        };
        known.remember(&typed, 2);
        assert_eq!(known.list.len(), 1);
        assert_eq!(known.list[0].printer.name, "Ender3V3KE-A654");
        assert_eq!(known.list[0].printer.api, Some(Api::CrealityLan));
        assert_eq!(known.list[0].last_seen, 2);
        known.forget("10.0.0.5");
        assert!(known.list.is_empty());
        // Without a path nothing is written.
        assert!(known.save().is_ok());
    }

    #[test]
    fn read_only_lists_never_change_the_file() {
        let path = temp_file("ro");
        let mut known = KnownPrinters::load(Some(path.clone()));
        known.remember(
            &FoundPrinter {
                host: "1.2.3.4".into(),
                name: "a".into(),
                ..Default::default()
            },
            1,
        );
        known.save().unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let mut ro = KnownPrinters::load_read_only(Some(path.clone()));
        assert_eq!(ro.list.len(), 1);
        ro.remember(
            &FoundPrinter {
                host: "127.0.0.1".into(),
                name: "test".into(),
                ..Default::default()
            },
            2,
        );
        ro.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_damaged_file_gives_an_empty_list() {
        let path = temp_file("bad");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert!(KnownPrinters::load(Some(path.clone())).list.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
