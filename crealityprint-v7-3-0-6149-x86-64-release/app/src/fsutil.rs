//! Saving files so a crash or power cut never leaves a half-written one behind.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Writes `data` to a temporary file beside `path`, flushes it to disk, then renames it
/// over `path`. The folder is created if needed; the temporary file never outlives a
/// failure.
pub fn write_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(path);
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        // Without this the rename can reach the disk before the data does.
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `name.ext` → `name.ext.tmp`, in the same folder (a rename only replaces atomically
/// within one file system).
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("printcraft-fsutil-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_and_replaces_a_file_creating_its_folder() {
        let dir = scratch("write");
        let path = dir.join("sub").join("a.json");
        write_atomically(&path, b"one").unwrap();
        write_atomically(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(!temp_path(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_save_leaves_no_temporary_file() {
        let dir = scratch("fail");
        // A folder where the file should go: the final rename fails.
        let path = dir.join("taken");
        std::fs::create_dir_all(path.join("inside")).unwrap();
        assert!(write_atomically(&path, b"data").is_err());
        assert!(!temp_path(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
