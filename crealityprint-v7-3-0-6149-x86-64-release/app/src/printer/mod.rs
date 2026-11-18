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
//! * A USB cable to a MakerBot-protocol printer (Sailfish: FlashForge Creator Pro and early
//!   Creator Pro 2, Replicator 1/2/2X): binary S3G packets.

pub mod calibration;
pub mod camera;
pub mod creality;
pub mod digest;
pub mod discovery;
pub mod duet;
#[cfg(test)]
#[path = "tests.rs"]
pub(crate) mod end_to_end;
pub mod known;
pub mod layer_info;
pub mod link;
pub mod moonraker;
pub mod octoprint;
pub mod prusalink;
pub mod sailfish;
pub mod serial_log;
pub mod usb;

use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    CrealityLan,
    Moonraker,
    PrusaLink,
    /// OctoPrint, the web host on a Raspberry Pi next to a USB printer.
    OctoPrint,
    /// A Duet board running RepRapFirmware (standalone, or run by a Raspberry Pi).
    Duet,
    Usb,
    /// The MakerBot S3G protocol over USB (Sailfish firmware).
    Sailfish,
}

impl Api {
    pub fn label(self) -> &'static str {
        match self {
            Self::CrealityLan => "Creality network service",
            Self::Moonraker => "Klipper (Moonraker)",
            Self::PrusaLink => "PrusaLink",
            Self::OctoPrint => "OctoPrint",
            Self::Duet => "Duet (RepRapFirmware)",
            Self::Usb => "USB cable",
            Self::Sailfish => "USB cable, MakerBot protocol",
        }
    }

    /// Printers running Klipper understand Klipper's own commands.
    pub fn is_klipper(self) -> bool {
        matches!(self, Self::CrealityLan | Self::Moonraker)
    }

    /// Connected by a cable to a serial port rather than over the network.
    pub fn is_serial(self) -> bool {
        matches!(self, Self::Usb | Self::Sailfish)
    }
}

impl Api {
    /// The name saved in files (projects, the known-printers list).
    pub fn key(self) -> &'static str {
        match self {
            Self::CrealityLan => "creality",
            Self::Moonraker => "moonraker",
            Self::PrusaLink => "prusalink",
            Self::OctoPrint => "octoprint",
            Self::Duet => "duet",
            Self::Usb => "usb",
            Self::Sailfish => "sailfish",
        }
    }

    pub fn from_key(key: &str) -> Option<Api> {
        [
            Self::CrealityLan,
            Self::Moonraker,
            Self::PrusaLink,
            Self::OctoPrint,
            Self::Duet,
            Self::Usb,
            Self::Sailfish,
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
    /// The second nozzle (tool 1) on a two-nozzle printer; None on one-nozzle printers.
    pub nozzle2: Option<f32>,
    pub nozzle2_target: Option<f32>,
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
    /// The firmware's own name, as a USB printer reports it to M115
    /// (`FIRMWARE_NAME:Repetier_1.0.3`, `FIRMWARE_NAME:Marlin 2.1.2`).
    pub firmware: Option<String>,
    /// The objects the running file names (Klipper's `exclude_object`), each of which
    /// can be cancelled on its own. Empty when the file names none or the printer can't.
    pub objects: Vec<PrintObject>,
}

/// One object of the running print, as the printer names it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrintObject {
    pub name: String,
    /// Cancelled: the printer skips it from now on.
    pub cancelled: bool,
}

/// The firmware families whose G-code differs where it matters to the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Firmware {
    /// Marlin, Prusa firmware and the many firmwares that follow them.
    #[default]
    Marlin,
    Klipper,
    /// Repetier-Firmware (RepRap wiki "Repetier-Firmware").
    Repetier,
}

impl Firmware {
    /// From the name a printer gives in its M115 report.
    pub fn from_name(name: &str) -> Firmware {
        let n = name.to_ascii_lowercase();
        if n.contains("repetier") {
            Self::Repetier
        } else if n.contains("klipper") {
            Self::Klipper
        } else {
            Self::Marlin
        }
    }

    pub fn is_klipper(self) -> bool {
        self == Self::Klipper
    }

    /// The command that measures the plate (bed levelling). Repetier-Firmware's is G32
    /// ("Probe Z and calculate Z plane"); its G29 only measures the height.
    pub fn level_command(self) -> &'static str {
        match self {
            Self::Klipper => "BED_MESH_CALIBRATE",
            Self::Marlin => "G29",
            Self::Repetier => "G32",
        }
    }
}

/// The firmware name in an M115 report line such as
/// `FIRMWARE_NAME:Repetier_1.0.3 FIRMWARE_URL:https://... PROTOCOL_VERSION:1.0`, made
/// readable ("Repetier 1.0.3").
pub fn firmware_name(line: &str) -> Option<String> {
    let rest = &line[line.find("FIRMWARE_NAME:")? + "FIRMWARE_NAME:".len()..];
    // The name runs until the next KEY: field.
    let end = rest
        .match_indices(' ')
        .map(|(i, _)| i)
        .find(|&i| {
            rest[i + 1..].split_whitespace().next().is_some_and(|w| {
                w.contains(':') && w.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            })
        })
        .unwrap_or(rest.len());
    let name = rest[..end].trim().replace('_', " ");
    (!name.is_empty()).then_some(name)
}

/// The version number in a firmware name ("Repetier 1.0.3" → "1.0.3"): the first word
/// that starts with a digit.
fn version_in(name: &str) -> Option<&str> {
    name.split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))
}

/// Which Repetier-Firmware line a printer runs, from its M115 name: 1 for the 1.x
/// releases (and the 0.9x releases they grew from), 2 for the 2.x rewrite. None when the
/// firmware isn't Repetier or gave no version.
pub fn repetier_version(name: &str) -> Option<u32> {
    if Firmware::from_name(name) != Firmware::Repetier {
        return None;
    }
    let major: u32 = version_in(name)?.split('.').next()?.parse().ok()?;
    Some(major.max(1))
}

/// How a USB printer is being driven, for the person to see: "Repetier-Firmware
/// protocol, version 1 (1.0.3)" or "Marlin-style protocol (Marlin 2.1.2)".
pub fn protocol_label(name: &str) -> String {
    match Firmware::from_name(name) {
        Firmware::Repetier => match (repetier_version(name), version_in(name)) {
            (Some(v), Some(full)) => format!("Repetier-Firmware protocol, version {v} ({full})"),
            _ => "Repetier-Firmware protocol, version not reported".into(),
        },
        Firmware::Klipper => format!("Klipper ({name})"),
        Firmware::Marlin => format!("Marlin-style protocol ({name})"),
    }
}

/// Things the app asks the printer to do.
#[derive(Clone, Debug)]
pub enum Command {
    UploadAndPrint {
        name: String,
        gcode: Arc<String>,
    },
    /// Upload a binary G-code file (`name` ends in `.bgcode`) and print it. PrusaLink
    /// only.
    UploadBinaryAndPrint {
        name: String,
        data: Arc<Vec<u8>>,
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
    /// Stop printing one object (its name in `PrinterStatus::objects`) while the others
    /// go on. Klipper (Moonraker) only.
    CancelObject(String),
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
            Self::OctoPrint | Self::Duet => Abilities {
                controls: true,
                files: true,
                history: false,
            },
            Self::Usb => Abilities {
                controls: true,
                files: false,
                history: false,
            },
            // The SD card's files can be listed and printed; there is no print history.
            Self::Sailfish => Abilities {
                controls: true,
                files: true,
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
        for api in [
            Api::CrealityLan,
            Api::Moonraker,
            Api::PrusaLink,
            Api::OctoPrint,
            Api::Duet,
            Api::Usb,
            Api::Sailfish,
        ] {
            assert_eq!(Api::from_key(api.key()), Some(api));
        }
        assert_eq!(Api::from_key("bambu"), None);
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

    #[test]
    fn names_the_repetier_protocol_and_its_version() {
        assert_eq!(
            protocol_label("Repetier 1.0.3"),
            "Repetier-Firmware protocol, version 1 (1.0.3)"
        );
        assert_eq!(
            protocol_label("Repetier 2.0.0"),
            "Repetier-Firmware protocol, version 2 (2.0.0)"
        );
        // 0.9x releases are the start of the version 1 line.
        assert_eq!(
            protocol_label("Repetier 0.92.9"),
            "Repetier-Firmware protocol, version 1 (0.92.9)"
        );
        assert_eq!(
            protocol_label("Repetier"),
            "Repetier-Firmware protocol, version not reported"
        );
        assert_eq!(
            protocol_label("Marlin 2.1.2.1 (Jun 12 2024)"),
            "Marlin-style protocol (Marlin 2.1.2.1 (Jun 12 2024))"
        );
        assert_eq!(repetier_version("Repetier 2.0.0"), Some(2));
        assert_eq!(repetier_version("Marlin 2.1.2"), None);
    }
}
