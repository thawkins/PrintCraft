//! A record of everything said over a USB printer's serial port, so a printer that
//! misbehaves can be diagnosed from what it actually answered.
//!
//! Each connection writes `usb-serial.log` in the `logs` folder of the settings folder.
//! The log of the connection before is kept as `usb-serial.previous.log`, so a printer
//! that restarts and reconnects doesn't wipe the evidence. Every line carries the time of
//! day and the seconds since the port opened, then `>>>` for what the app sent or `<<<`
//! for what the printer answered:
//!
//! ```text
//! 14:02:07.512  +2.004s  >>> M115
//! 14:02:07.530  +2.022s  <<< FIRMWARE_NAME:Repetier_1.0.3 ...
//! ```
//!
//! The file stops growing at [`LIMIT_BYTES`]; a print's start is where problems show.
//! Nothing secret goes over a serial port (only G-code and the printer's replies).

use std::fs::File;
use std::io::{LineWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The largest log kept; writing stops there.
pub const LIMIT_BYTES: u64 = 50 * 1024 * 1024;
pub const FILE_NAME: &str = "usb-serial.log";
pub const PREVIOUS_FILE_NAME: &str = "usb-serial.previous.log";

pub struct SerialLog {
    out: Option<LineWriter<File>>,
    started: Instant,
    written: u64,
    limit: u64,
    pub path: PathBuf,
}

impl SerialLog {
    /// Opens the log in the settings folder's `logs` folder. None when there is no settings
    /// folder or the file can't be written (the connection works the same without it).
    pub fn open_default(port: &str, baud: u32) -> Option<SerialLog> {
        // Tests drive pretend printers; they must not replace a real printer's log.
        let dir = if cfg!(test) {
            std::env::temp_dir().join(format!("printcraft-test-logs-{}", std::process::id()))
        } else {
            crate::fsutil::settings_dir()?.join("logs")
        };
        SerialLog::open(&dir, port, baud, LIMIT_BYTES).ok()
    }

    /// Starts a new log in `dir`, keeping the one before as the previous log.
    pub fn open(dir: &Path, port: &str, baud: u32, limit: u64) -> std::io::Result<SerialLog> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE_NAME);
        if path.exists() {
            std::fs::rename(&path, dir.join(PREVIOUS_FILE_NAME))?;
        }
        let mut log = SerialLog {
            out: Some(LineWriter::new(File::create(&path)?)),
            started: Instant::now(),
            written: 0,
            limit,
            path,
        };
        log.write_raw(&format!(
            "PrintCraft {} serial log: {port} at {baud} baud, opened {}\n",
            env!("CARGO_PKG_VERSION"),
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        Ok(log)
    }

    /// A line the app wrote to the printer.
    pub fn sent(&mut self, line: &str) {
        self.line(">>>", line);
    }

    /// A line the printer wrote to the app.
    pub fn received(&mut self, line: &str) {
        self.line("<<<", line);
    }

    /// Something that happened on the app's side (the firmware recognised, the port lost).
    pub fn note(&mut self, msg: &str) {
        self.line("---", msg);
    }

    fn line(&mut self, mark: &str, text: &str) {
        let text = text.trim_end_matches(['\r', '\n']);
        let entry = format!(
            "{}  +{:.3}s  {mark} {text}\n",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            self.started.elapsed().as_secs_f64()
        );
        if self.written + entry.len() as u64 > self.limit {
            self.write_raw("--- The log is full; nothing more is recorded.\n");
            self.out = None;
            return;
        }
        self.write_raw(&entry);
    }

    fn write_raw(&mut self, text: &str) {
        let Some(out) = &mut self.out else { return };
        if out.write_all(text.as_bytes()).is_err() {
            // A full disk or a removed folder: stop logging, keep printing.
            self.out = None;
            return;
        }
        self.written += text.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "printcraft-serial-log-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn records_both_directions_in_order() {
        let dir = temp_dir("order");
        let mut log = SerialLog::open(&dir, "/dev/ttyUSB0", 250_000, LIMIT_BYTES).unwrap();
        log.sent("N1 G28*18");
        log.received("ok 1\r\n");
        log.note("printer restarted");
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains("/dev/ttyUSB0 at 250000 baud"), "{text}");
        assert!(lines[1].ends_with(">>> N1 G28*18"), "{text}");
        assert!(
            lines[2].ends_with("<<< ok 1"),
            "line endings are dropped: {text}"
        );
        assert!(lines[3].ends_with("--- printer restarted"), "{text}");
        assert!(lines[1].contains("  +0."), "seconds since opening: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_connection_keeps_the_log_before() {
        let dir = temp_dir("previous");
        SerialLog::open(&dir, "COM3", 115_200, LIMIT_BYTES)
            .unwrap()
            .received("first connection");
        SerialLog::open(&dir, "COM3", 115_200, LIMIT_BYTES)
            .unwrap()
            .received("second connection");
        let now = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        let before = std::fs::read_to_string(dir.join(PREVIOUS_FILE_NAME)).unwrap();
        assert!(now.contains("second connection") && !now.contains("first connection"));
        assert!(before.contains("first connection"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stops_at_the_size_limit() {
        let dir = temp_dir("limit");
        let mut log = SerialLog::open(&dir, "COM3", 115_200, 400).unwrap();
        for i in 0..100 {
            log.sent(&format!("N{i} G1 X{i}"));
        }
        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(
            text.trim_end().ends_with("nothing more is recorded."),
            "{text}"
        );
        assert!(text.len() < 500, "{} bytes", text.len());
        assert_eq!(text.matches("is full").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
