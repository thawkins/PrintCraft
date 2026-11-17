//! The project files opened or saved most recently, newest first, so the File menu can
//! offer them without going through the open dialog. Kept between runs as JSON in the
//! settings folder (`recent_projects.json`, beside `known_printers.json`).

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// How many projects the File menu lists.
pub const MAX_RECENT: usize = 5;

#[derive(Debug, Default)]
pub struct RecentProjects {
    path: Option<PathBuf>,
    /// Shows the saved list but never changes the file (used while taking screenshots).
    read_only: bool,
    pub list: Vec<PathBuf>,
}

impl RecentProjects {
    pub fn default_path() -> Option<PathBuf> {
        Some(crate::fsutil::settings_dir()?.join("recent_projects.json"))
    }

    /// Reads the list; a missing or damaged file gives an empty list. With no path,
    /// nothing is ever read or written.
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut list: Vec<PathBuf> = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| {
                v.get("recent").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(PathBuf::from)
                        .collect()
                })
            })
            .unwrap_or_default();
        list.dedup();
        list.truncate(MAX_RECENT);
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

    /// Puts `file` at the top (moving it up if it was already listed), keeps the newest
    /// `MAX_RECENT`, and saves the list.
    pub fn add(&mut self, file: &Path) {
        let file = absolute(file);
        self.list.retain(|p| p != &file);
        self.list.insert(0, file);
        self.list.truncate(MAX_RECENT);
        self.save();
    }

    /// Takes `file` off the list (it was moved or deleted) and saves the list.
    pub fn remove(&mut self, file: &Path) {
        let file = absolute(file);
        self.list.retain(|p| p != &file);
        self.save();
    }

    pub fn clear(&mut self) {
        self.list.clear();
        self.save();
    }

    /// Writes the list. A failure only means the list isn't remembered next time, so
    /// it is not reported.
    fn save(&self) {
        let Some(path) = self.path.as_ref().filter(|_| !self.read_only) else {
            return;
        };
        let names: Vec<String> = self
            .list
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let text = serde_json::to_string_pretty(&json!({ "recent": names })).unwrap_or_default();
        let _ = crate::fsutil::write_atomically(path, text.as_bytes());
    }
}

/// The full path, so the same file opened from different folders is listed once.
fn absolute(p: &Path) -> PathBuf {
    std::fs::canonicalize(p)
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("printcraft-recent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn keeps_the_five_newest_projects_newest_first() {
        let dir = temp_dir("five");
        let store = dir.join("recent_projects.json");
        let mut r = RecentProjects::load(Some(store.clone()));
        let files: Vec<PathBuf> = (0..7)
            .map(|i| {
                let f = dir.join(format!("p{i}.printcraft"));
                std::fs::write(&f, "{}").unwrap();
                f
            })
            .collect();
        for f in &files {
            r.add(f);
        }
        // Opening an older one again moves it to the top without listing it twice.
        r.add(&files[3]);
        let names = |r: &RecentProjects| -> Vec<String> {
            r.list
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        let expected = ["p3", "p6", "p5", "p4", "p2"].map(|n| format!("{n}.printcraft"));
        assert_eq!(names(&r), expected);
        // Remembered between runs.
        assert_eq!(names(&RecentProjects::load(Some(store.clone()))), expected);
        r.remove(&files[6]);
        assert_eq!(RecentProjects::load(Some(store.clone())).list.len(), 4);
        r.clear();
        assert!(RecentProjects::load(Some(store)).list.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_only_list_never_changes_the_file() {
        let dir = temp_dir("ro");
        let store = dir.join("recent_projects.json");
        std::fs::write(
            &store,
            r#"{"recent": ["/a/one.printcraft", "", "/a/one.printcraft"]}"#,
        )
        .unwrap();
        let mut r = RecentProjects::load_read_only(Some(store.clone()));
        assert_eq!(r.list, vec![PathBuf::from("/a/one.printcraft")]);
        r.add(Path::new("/b/two.printcraft"));
        assert_eq!(r.list.len(), 2);
        assert_eq!(RecentProjects::load(Some(store)).list.len(), 1);
        // A damaged file is an empty list, not an error.
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        assert!(RecentProjects::load(Some(bad)).list.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
