//! Talking to a printer: finding it, sending a print, watching the job and showing its
//! camera. See PRINTER_LINK.md for the protocols and how sure we are of each.
//!
//! Four kinds of connection are supported:
//! * Creality's own network service (stock Ender-3 V3 KE / K1-family firmware): a
//!   WebSocket on port 9999 for status and commands, and HTTP on port 80 for uploads.
//! * Moonraker (Klipper's web API, port 7125), which many owners install on these printers.
//! * PrusaLink (Prusa MK4/MK4S, Core One, MINI, XL, and MK3S with a Raspberry Pi):
//!   Prusa's documented local HTTP API, protected by a password.
//! * A USB cable: G-code is streamed line by line over the serial port (Marlin and Prusa
//!   firmware).

pub mod calibration;
pub mod camera;
pub mod creality;
pub mod digest;
pub mod discovery;
#[cfg(test)]
#[path = "tests.rs"]
mod end_to_end;
pub mod known;
pub mod layer_info;
pub mod link;
pub mod moonraker;
pub mod prusalink;
pub mod usb;

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    CrealityLan,
    Moonraker,
    PrusaLink,
    Usb,
}

impl Api {
    pub fn label(self) -> &'static str {
        match self {
            Self::CrealityLan => "Creality network service",
            Self::Moonraker => "Klipper (Moonraker)",
            Self::PrusaLink => "PrusaLink",
            Self::Usb => "USB cable",
        }
    }

    /// Printers running Klipper understand Klipper's own commands.
    pub fn is_klipper(self) -> bool {
        matches!(self, Self::CrealityLan | Self::Moonraker)
    }
}

impl Api {
    /// The name saved in files (projects, the known-printers list).
    pub fn key(self) -> &'static str {
        match self {
            Self::CrealityLan => "creality",
            Self::Moonraker => "moonraker",
            Self::PrusaLink => "prusalink",
            Self::Usb => "usb",
        }
    }

    pub fn from_key(key: &str) -> Option<Api> {
        [
            Self::CrealityLan,
            Self::Moonraker,
            Self::PrusaLink,
            Self::Usb,
        ]
        .into_iter()
        .find(|a| a.key() == key)
    }
}

/// A printer found on the network or a USB port (or typed in by hand, `api` still unknown).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundPrinter {
    /// Network address, or the serial port (e.g. /dev/ttyACM0) for USB.
    pub host: String,
    pub name: String,
    pub model: String,
    pub api: Option<Api>,
    /// PrusaLink user name and password (shown on the printer's screen).
    pub login: Option<(String, String)>,
    /// USB speed in baud.
    pub baud: Option<u32>,
}

impl FoundPrinter {
    /// The printer as saved in a file. The password is never written.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "name": self.name,
            "model": self.model,
            "api": self.api.map(Api::key),
            "baud": self.baud,
        })
    }

    /// A saved printer; None without a host. A missing name or model reads as empty.
    pub fn from_json(v: &serde_json::Value) -> Option<FoundPrinter> {
        let text = |k: &str| v.get(k).and_then(|x| x.as_str());
        Some(FoundPrinter {
            host: text("host")?.to_string(),
            name: text("name").unwrap_or_default().to_string(),
            model: text("model").unwrap_or_default().to_string(),
            api: text("api").and_then(Api::from_key),
            login: None,
            baud: v.get("baud").and_then(|b| b.as_u64()).map(|b| b as u32),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JobState {
    #[default]
    Unknown,
    Idle,
    Printing,
    Paused,
    Finished,
    Failed,
    Stopped,
}

impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Waiting for status…",
            Self::Idle => "Ready",
            Self::Printing => "Printing",
            Self::Paused => "Paused",
            Self::Finished => "Finished",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Printing | Self::Paused)
    }
}

/// What the printer last told us. Fields stay `None` until the printer reports them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrinterStatus {
    pub state: JobState,
    pub nozzle: Option<f32>,
    pub nozzle_target: Option<f32>,
    pub bed: Option<f32>,
    pub bed_target: Option<f32>,
    /// 0-100.
    pub progress: Option<f32>,
    pub elapsed_s: Option<u64>,
    pub remaining_s: Option<u64>,
    pub file: Option<String>,
    /// Where the printer keeps the current file, for printing it again.
    pub file_path: Option<String>,
    pub layer: Option<u32>,
    pub total_layers: Option<u32>,
    pub model: Option<String>,
    pub hostname: Option<String>,
    /// Part-cooling ("model") fan, 0-100.
    pub fan_percent: Option<f32>,
    /// Print speed as a share of the file's speeds, 100 = as sliced.
    pub speed_percent: Option<f32>,
    /// The printer's own "calibrate before printing" setting, when it reports one.
    pub printer_calibration: Option<bool>,
}

/// Things the app asks the printer to do.
#[derive(Clone, Debug)]
pub enum Command {
    UploadAndPrint {
        name: String,
        gcode: Arc<String>,
    },
    /// Print a file already on the printer (its `RemoteFile::path`).
    PrintFile(String),
    Pause,
    Resume,
    Stop,
    SetNozzle(f32),
    SetBed(f32),
    /// Model fan, 0-100 %.
    SetFan(f32),
    /// Speed, percent of the sliced speeds.
    SetSpeed(u32),
    /// Move one axis by `mm` (relative).
    Jog {
        axis: char,
        mm: f32,
    },
    Home,
    RefreshFiles,
    RefreshHistory,
    /// "Enable Calibration": whether the printer runs its own calibration (bed levelling)
    /// before the next print. Only the Creality service has such a setting; elsewhere the
    /// app adds or leaves out its own levelling step instead.
    SetCalibration(bool),
}

/// A print file stored on the printer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteFile {
    pub name: String,
    /// What the printer needs to print it.
    pub path: String,
    pub size: Option<u64>,
    /// Seconds since 1970.
    pub modified: Option<i64>,
}

/// One finished, failed or stopped print.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryEntry {
    pub name: String,
    pub outcome: String,
    /// Seconds since 1970.
    pub ended: Option<i64>,
    pub duration_s: Option<u64>,
}

/// What each kind of connection can do beyond printing and watching.
pub struct Abilities {
    /// Temperatures, fan, speed, moving the axes and homing.
    pub controls: bool,
    pub files: bool,
    pub history: bool,
}

impl Api {
    pub fn abilities(self) -> Abilities {
        match self {
            Self::CrealityLan | Self::Moonraker => Abilities {
                controls: true,
                files: true,
                history: true,
            },
            Self::PrusaLink => Abilities {
                controls: false,
                files: true,
                history: false,
            },
            Self::Usb => Abilities {
                controls: true,
                files: false,
                history: false,
            },
        }
    }
}

/// G-code for a relative move of one axis: Z moves slower than X and Y.
pub fn jog_gcode(axis: char, mm: f32) -> Vec<String> {
    let feed = if axis == 'Z' { JOG_FEED_Z } else { JOG_FEED_XY };
    vec!["G91".into(), format!("G1 {axis}{mm} F{feed}"), "G90".into()]
}

/// G-code to set the nozzle temperature (no waiting).
pub fn nozzle_gcode(celsius: f32) -> String {
    format!("M104 S{celsius:.0}")
}

/// G-code to set the plate temperature (no waiting).
pub fn bed_gcode(celsius: f32) -> String {
    format!("M140 S{celsius:.0}")
}

/// A plain-HTTP client with a time limit to connect and one for the whole request.
pub fn http_agent(connect: std::time::Duration, total: std::time::Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(connect)
        .timeout(total)
        .build()
}

/// G-code to set the model fan (0-100 %). `P0` picks the model fan on Creality firmware;
/// standard Klipper and Marlin ignore it.
pub fn fan_gcode(percent: f32) -> String {
    let v = (percent.clamp(0.0, 100.0) * 2.55).round() as u32;
    if v == 0 {
        "M107".into()
    } else {
        format!("M106 P0 S{v}")
    }
}

pub const JOG_FEED_XY: u32 = 3000;
pub const JOG_FEED_Z: u32 = 600;

/// Percent-encodes text for a URL query.
pub(crate) fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Called by background threads when there is something new to draw.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Reads a JSON number that may also arrive as text ("25.120000").
pub(crate) fn json_number(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// A multipart/form-data body. Each part is (field name, file name, contents).
pub(crate) fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let boundary = format!("----PrintCraft{:016x}", fastrand_seed());
    let mut body = Vec::new();
    for (field, file, data) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some(f) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{field}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{field}\"\r\n\r\n").as_bytes()),
        }
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn fastrand_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5eed)
}

/// A file name the printer will accept: letters, digits, dash, underscore and dot.
pub fn safe_file_name(name: &str) -> String {
    let stem: String = name
        .trim_end_matches(".gcode")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = if stem.is_empty() {
        "print".to_string()
    } else {
        stem
    };
    format!("{stem}.gcode")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_connection_kind_round_trips_through_its_saved_name() {
        for api in [Api::CrealityLan, Api::Moonraker, Api::PrusaLink, Api::Usb] {
            assert_eq!(Api::from_key(api.key()), Some(api));
        }
        assert_eq!(Api::from_key("octoprint"), None);
    }

    #[test]
    fn a_saved_printer_round_trips_without_its_password() {
        let p = FoundPrinter {
            host: "192.168.1.20".into(),
            name: "Workshop".into(),
            model: "Prusa printer (PrusaLink)".into(),
            api: Some(Api::PrusaLink),
            login: Some(("maker".into(), "secret".into())),
            baud: None,
        };
        let v = p.to_json();
        assert!(!v.to_string().contains("secret"));
        let back = FoundPrinter::from_json(&v).unwrap();
        assert_eq!(back, FoundPrinter { login: None, ..p });
        assert_eq!(
            FoundPrinter::from_json(&serde_json::json!({"name": "x"})),
            None
        );
    }

    #[test]
    fn numbers_may_be_text() {
        assert_eq!(json_number(&serde_json::json!("25.5")), Some(25.5));
        assert_eq!(json_number(&serde_json::json!(60)), Some(60.0));
        assert_eq!(json_number(&serde_json::json!(null)), None);
    }

    #[test]
    fn file_names_are_cleaned() {
        assert_eq!(safe_file_name("My part (v2)"), "My_part__v2_.gcode");
        assert_eq!(safe_file_name("cube.gcode"), "cube.gcode");
        assert_eq!(safe_file_name(""), "print.gcode");
    }

    #[test]
    fn multipart_body_has_parts() {
        let (ct, body) = multipart(&[("file", Some("a.gcode"), b"G28"), ("print", None, b"true")]);
        let text = String::from_utf8(body).unwrap();
        let boundary = ct.split("boundary=").nth(1).unwrap();
        assert!(text.contains("filename=\"a.gcode\""));
        assert!(text.contains("name=\"print\"\r\n\r\ntrue"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn control_gcode() {
        assert_eq!(jog_gcode('X', -10.0), vec!["G91", "G1 X-10 F3000", "G90"]);
        assert_eq!(jog_gcode('Z', 1.0)[1], "G1 Z1 F600");
        assert_eq!(fan_gcode(48.0), "M106 P0 S122");
        assert_eq!(fan_gcode(100.0), "M106 P0 S255");
        assert_eq!(fan_gcode(0.0), "M107");
        assert_eq!(url_encode("G91\nG1 X1"), "G91%0AG1%20X1");
    }
}
