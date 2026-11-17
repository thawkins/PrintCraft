//! The S3G questions the app asks, and how to read the answers. These are "host
//! queries" (command numbers below 128): the printer answers at once and never queues
//! them, and none of them moves, heats or changes anything.
//!
//! Two host commands are deliberately absent: 3 (clear buffer) and 7 (abort). Sailfish
//! answers both by restarting the board (`Host.cc`). Stopping a print uses 22 (extended
//! stop) instead, which halts the motors and empties the command queue without a restart.
//!
//! A few of these do change the printer's state, and are only sent when the person asks:
//! 8 (pause/resume), 22 (stop), 14/15 (record a file on the SD card) and 16 (print a file
//! from the card).

/// The host version sent with the version questions. Sailfish answers version 0 to a
/// host older than 39 (ReplicatorG 0038), so anything from 39 up works.
pub const HOST_VERSION: u16 = 100;

/// The largest EEPROM read one packet can carry (32-byte payload, less the reply code).
pub const EEPROM_CHUNK: u8 = 31;
/// The MightyBoard's ATmega has 4 KB of EEPROM.
pub const EEPROM_SIZE: u16 = 4096;

/// What to ask a tool (nozzle board), command 10's subcommands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolQuery {
    /// 2: the nozzle's temperature.
    Temperature,
    /// 32: the nozzle's target temperature.
    Target,
    /// 30: the bed's temperature (asked through tool 0).
    BedTemperature,
    /// 33: the bed's target temperature.
    BedTarget,
}

impl ToolQuery {
    fn code(self) -> u8 {
        match self {
            ToolQuery::Temperature => 2,
            ToolQuery::BedTemperature => 30,
            ToolQuery::Target => 32,
            ToolQuery::BedTarget => 33,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// 0: the firmware version.
    Version,
    /// 27: firmware version, internal version and variant (Sailfish or MakerBot).
    AdvancedVersion,
    /// 10: a tool's temperature or target.
    Tool { tool: u8, what: ToolQuery },
    /// 23: the motherboard's status bits.
    BoardStatus,
    /// 24: the build's state and running time.
    BuildStatistics,
    /// 20: the name of the build running now.
    BuildName,
    /// 21: the position in steps and the endstops.
    Position,
    /// 12: `len` bytes of EEPROM from `offset` (at most [`EEPROM_CHUNK`]).
    ReadEeprom { offset: u16, len: u8 },
    /// 8: pause the build, or resume it if it is paused.
    PauseResume,
    /// 22: halt the motors at once and empty the command queue.
    ExtendedStop,
    /// 18: the next file name on the SD card (`restart` starts from the first).
    NextFileName { restart: bool },
    /// 16: print a file from the SD card.
    PlayFile { name: String },
    /// 14: record the following commands into a file on the SD card instead of running
    /// them (only in firmware built with `S3G_CAPTURE_2_SD`).
    CaptureToFile { name: String },
    /// 15: stop recording; answers the number of bytes written.
    EndCapture,
}

impl Query {
    pub fn payload(&self) -> Vec<u8> {
        let hv = HOST_VERSION.to_le_bytes();
        let named = |code: u8, name: &str| {
            let mut v = vec![code];
            v.extend(name.bytes().take(SD_NAME_MAX));
            v.push(0);
            v
        };
        match self {
            Query::Version => vec![0, hv[0], hv[1]],
            Query::AdvancedVersion => vec![27, hv[0], hv[1]],
            Query::Tool { tool, what } => vec![10, *tool, what.code()],
            Query::BoardStatus => vec![23],
            Query::BuildStatistics => vec![24],
            Query::BuildName => vec![20],
            Query::Position => vec![21],
            Query::ReadEeprom { offset, len } => {
                let o = offset.to_le_bytes();
                vec![12, o[0], o[1], (*len).min(EEPROM_CHUNK)]
            }
            Query::PauseResume => vec![8],
            // Bit 0 halts the motors, bit 1 empties the queue (`Host.cc`).
            Query::ExtendedStop => vec![22, 0b11],
            Query::NextFileName { restart } => vec![18, u8::from(*restart)],
            Query::PlayFile { name } => named(16, name),
            Query::CaptureToFile { name } => named(14, name),
            Query::EndCapture => vec![15],
        }
    }

    /// A few words for the serial log.
    pub fn describe(&self) -> String {
        match self {
            Query::Version => "version?".into(),
            Query::AdvancedVersion => "advanced version?".into(),
            Query::Tool { tool, what } => format!(
                "tool {tool}: {}?",
                match what {
                    ToolQuery::Temperature => "temperature",
                    ToolQuery::Target => "target temperature",
                    ToolQuery::BedTemperature => "bed temperature",
                    ToolQuery::BedTarget => "bed target",
                }
            ),
            Query::BoardStatus => "motherboard status?".into(),
            Query::BuildStatistics => "build statistics?".into(),
            Query::BuildName => "build name?".into(),
            Query::Position => "position?".into(),
            Query::ReadEeprom { offset, len } => format!("EEPROM 0x{offset:04X} +{len}?"),
            Query::PauseResume => "pause / resume".into(),
            Query::ExtendedStop => "stop: halt the motors and empty the queue".into(),
            Query::NextFileName { restart: true } => "first SD file?".into(),
            Query::NextFileName { restart: false } => "next SD file?".into(),
            Query::PlayFile { name } => format!("print {name} from the SD card"),
            Query::CaptureToFile { name } => format!("record into {name} on the SD card"),
            Query::EndCapture => "end recording".into(),
        }
    }
}

/// The longest file name the SD commands take: 8.3, so 12 characters.
pub const SD_NAME_MAX: usize = 12;

/// A file name the printer's SD card (FAT16, 8.3 names) can hold: up to 8 letters,
/// digits, `-` or `_` from the job's name, then `.x3g`.
pub fn sd_file_name(job: &str) -> String {
    let stem = job.rsplit_once('.').map(|(s, _)| s).unwrap_or(job);
    let clean: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(8)
        .collect();
    format!("{}.x3g", if clean.is_empty() { "print" } else { &clean })
}

/// What the SD status byte of commands 14, 16 and 18 means (0 is success).
pub fn sd_problem(code: u8) -> &'static str {
    match code {
        0 => "no problem",
        1 => "there is no SD card in the printer",
        2 => "the SD card could not be started",
        3 => "the SD card's partition table can't be read",
        4 => "the SD card's file system can't be opened",
        5 => "the SD card's main folder can't be opened",
        6 => "the SD card is locked (write-protected)",
        _ => "the SD card reported an unknown problem",
    }
}

/// Command 18's answer: Ok(None) when the list is done.
pub fn read_next_file_name(answer: &[u8]) -> Result<Option<String>, u8> {
    let code = *answer.first().ok_or(0xFF)?;
    if code != 0 {
        return Err(code);
    }
    let name = read_text(&answer[1..]);
    Ok(Some(name).filter(|n| !n.is_empty()))
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn i16_at(b: &[u8], i: usize) -> Option<i16> {
    Some(i16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

/// A little-endian u32 at `i` in an answer (command 15's byte count).
pub fn read_u32(b: &[u8], i: usize) -> Option<u32> {
    u32_at(b, i)
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], i: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// The answers below are read from the reply payload *after* the reply code.
pub fn read_version(answer: &[u8]) -> Option<u16> {
    u16_at(answer, 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdvancedVersion {
    pub firmware: u16,
    pub internal: u16,
    pub variant: u8,
}

pub fn read_advanced_version(answer: &[u8]) -> Option<AdvancedVersion> {
    Some(AdvancedVersion {
        firmware: u16_at(answer, 0)?,
        internal: u16_at(answer, 2)?,
        variant: *answer.get(4)?,
    })
}

/// A firmware version number as the firmware writes it: 708 is "7.8", 705 is "7.5".
pub fn version_text(v: u16) -> String {
    format!("{}.{}", v / 100, v % 100)
}

/// The firmware's name from the variant byte. The specification and Sailfish's
/// `Configuration.hh` give 0x80 for Sailfish; Sailfish's own `SoftwareVariant` enum says 2,
/// so both are accepted.
pub fn variant_name(variant: u8) -> &'static str {
    match variant {
        0x80 | 0x02 => "Sailfish",
        0x01 => "MakerBot firmware",
        _ => "MakerBot-protocol firmware",
    }
}

/// A temperature or target in °C.
pub fn read_temperature(answer: &[u8]) -> Option<f32> {
    i16_at(answer, 0).map(f32::from)
}

/// Command 23's status bits, as the specification names them.
pub fn board_status_words(bits: u8) -> Vec<&'static str> {
    const NAMES: [&str; 8] = [
        "preheating",
        "manual mode",
        "running an onboard script",
        "running an onboard process",
        "waiting for a button press",
        "cancelling the build",
        "heaters shut down",
        "power error",
    ];
    (0..8)
        .filter(|i| bits & (1 << i) != 0)
        .map(|i| NAMES[i])
        .collect()
}

/// Command 24's build state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildState {
    None,
    Running,
    Finished,
    Paused,
    Cancelled,
    Sleeping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildStatistics {
    pub state: BuildState,
    pub hours: u8,
    pub minutes: u8,
}

pub fn read_build_statistics(answer: &[u8]) -> Option<BuildStatistics> {
    let state = match *answer.first()? {
        0 => BuildState::None,
        1 => BuildState::Running,
        2 => BuildState::Finished,
        3 => BuildState::Paused,
        4 => BuildState::Cancelled,
        5 => BuildState::Sleeping,
        _ => return None,
    };
    Some(BuildStatistics {
        state,
        hours: *answer.get(1)?,
        minutes: *answer.get(2)?,
    })
}

/// A null-terminated name (the build name).
pub fn read_text(answer: &[u8]) -> String {
    let end = answer.iter().position(|&b| b == 0).unwrap_or(answer.len());
    String::from_utf8_lossy(&answer[..end]).trim().to_string()
}

/// X, Y, Z, A, B in steps.
pub fn read_position(answer: &[u8]) -> Option<[i32; 5]> {
    let mut p = [0; 5];
    for (i, v) in p.iter_mut().enumerate() {
        *v = i32_at(answer, i * 4)?;
    }
    Some(p)
}

/// Where Sailfish keeps its settings (`EepromMap.hh`, Sailfish 7.8).
mod offsets {
    pub const AXIS_INVERSION: usize = 0x0002;
    pub const AXIS_HOME_DIRECTION: usize = 0x000C;
    pub const AXIS_HOME_POSITIONS_STEPS: usize = 0x000E;
    pub const MACHINE_NAME: usize = 0x0022;
    pub const TOOL_COUNT: usize = 0x0042;
    pub const HBP_PRESENT: usize = 0x004C;
    pub const TOOLHEAD_OFFSET_SETTINGS: usize = 0x0162;
    pub const AXIS_LENGTHS: usize = 0x018C;
    pub const AXIS_STEPS_PER_MM: usize = 0x01A4;
    pub const AXIS_MAX_FEEDRATES: usize = 0x01F4;
    pub const TOOLHEAD_OFFSET_SYSTEM: usize = 0x0F93;
    pub const DITTO_PRINT_ENABLED: usize = 0x0FFF;
}

/// The machine settings Sailfish keeps in EEPROM, read from a copy of it. A value an
/// erased EEPROM would hold (all ones) reads as None.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineSettings {
    pub name: String,
    pub tool_count: Option<u8>,
    pub heated_bed: Option<bool>,
    /// Bit 0 = X … bit 4 = B.
    pub axis_inversion: u8,
    pub home_direction: u8,
    pub home_steps: [Option<i32>; 5],
    pub steps_per_mm: [Option<f64>; 5],
    /// Axis lengths in steps.
    pub length_steps: [Option<i32>; 5],
    /// mm/s.
    pub max_feedrate: [Option<u32>; 5],
    /// The second nozzle's offset correction in X, Y, Z (steps). Sailfish's comment calls
    /// it "how far out of tolerance" the nozzle distance is; what it is relative to
    /// depends on `offset_system`.
    pub toolhead_offset_steps: [Option<i32>; 3],
    pub offset_system: Option<u8>,
    pub ditto: Option<bool>,
}

impl MachineSettings {
    /// Reads the settings from an EEPROM copy starting at offset 0. Missing parts of a
    /// short copy read as None.
    pub fn from_eeprom(e: &[u8]) -> MachineSettings {
        use offsets::*;
        let byte = |i: usize| e.get(i).copied().filter(|&b| b != 0xFF);
        let flag = |i: usize| byte(i).map(|b| b != 0);
        let i32s = |base: usize| -> [Option<i32>; 5] {
            std::array::from_fn(|k| i32_at(e, base + k * 4).filter(|&v| v != -1))
        };
        let u32s = |base: usize| -> [Option<u32>; 5] {
            std::array::from_fn(|k| u32_at(e, base + k * 4).filter(|&v| v != u32::MAX))
        };
        let name_bytes = e.get(MACHINE_NAME..MACHINE_NAME + 16).unwrap_or(&[]);
        let name: Vec<u8> = name_bytes
            .iter()
            .copied()
            .take_while(|&b| b != 0 && b != 0xFF)
            .collect();
        MachineSettings {
            name: String::from_utf8_lossy(&name).trim().to_string(),
            tool_count: byte(TOOL_COUNT),
            heated_bed: flag(HBP_PRESENT),
            axis_inversion: byte(AXIS_INVERSION).unwrap_or(0),
            home_direction: byte(AXIS_HOME_DIRECTION).unwrap_or(0),
            home_steps: i32s(AXIS_HOME_POSITIONS_STEPS),
            steps_per_mm: u32s(AXIS_STEPS_PER_MM).map(|v| v.map(|s| s as f64 / 1_000_000.0)),
            length_steps: i32s(AXIS_LENGTHS),
            max_feedrate: u32s(AXIS_MAX_FEEDRATES),
            toolhead_offset_steps: std::array::from_fn(|k| {
                i32_at(e, TOOLHEAD_OFFSET_SETTINGS + k * 4).filter(|&v| v != -1)
            }),
            offset_system: byte(TOOLHEAD_OFFSET_SYSTEM),
            ditto: flag(DITTO_PRINT_ENABLED),
        }
    }

    /// The settings as lines for the connection log and the serial log.
    pub fn summary(&self) -> Vec<String> {
        const AXES: [&str; 5] = ["X", "Y", "Z", "A", "B"];
        let unknown = || "unknown".to_string();
        let mut out = Vec::new();
        out.push(format!(
            "Machine \"{}\": {} nozzle(s), heated bed {}, Ditto printing {}.",
            if self.name.is_empty() {
                "(no name)"
            } else {
                &self.name
            },
            self.tool_count
                .map(|t| t.to_string())
                .unwrap_or_else(unknown),
            yes_no(self.heated_bed),
            match self.ditto {
                Some(true) => "on",
                Some(false) => "off",
                None => "unknown",
            }
        ));
        let mut axes = Vec::new();
        for (i, axis) in AXES.iter().enumerate() {
            let spm = self.steps_per_mm[i];
            let mut parts = vec![match spm {
                Some(s) => format!("{s:.6} steps/mm"),
                None => "steps/mm unknown".into(),
            }];
            if let (Some(len), Some(s)) = (self.length_steps[i], spm.filter(|s| *s > 0.0)) {
                parts.push(format!("length {:.1} mm", len as f64 / s));
            }
            if let Some(f) = self.max_feedrate[i] {
                parts.push(format!("max {f} mm/s"));
            }
            if i < 3 {
                parts.push(format!(
                    "homes to {}",
                    if self.home_direction & (1 << i) != 0 {
                        "max"
                    } else {
                        "min"
                    }
                ));
            }
            if self.axis_inversion & (1 << i) != 0 {
                parts.push("inverted".into());
            }
            axes.push(format!("{axis}: {}", parts.join(", ")));
        }
        out.push(format!("Axes: {}.", axes.join("; ")));
        out.push(format!(
            "Home position (steps): {}. Nozzle offset correction (steps): {} (offset system {}).",
            self.home_steps
                .iter()
                .take(3)
                .map(|v| v.map(|v| v.to_string()).unwrap_or_else(unknown))
                .collect::<Vec<_>>()
                .join(", "),
            self.toolhead_offset_steps
                .iter()
                .map(|v| v.map(|v| v.to_string()).unwrap_or_else(unknown))
                .collect::<Vec<_>>()
                .join(", "),
            self.offset_system
                .map(|v| v.to_string())
                .unwrap_or_else(unknown),
        ));
        out
    }
}

fn yes_no(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_questions_carry_the_host_version() {
        assert_eq!(Query::Version.payload(), vec![0, 100, 0]);
        assert_eq!(Query::AdvancedVersion.payload(), vec![27, 100, 0]);
        const { assert!(HOST_VERSION >= 39, "Sailfish answers 0 to older hosts") };
    }

    #[test]
    fn no_question_restarts_the_printer() {
        // 3 (clear buffer) and 7 (abort) make Sailfish restart the board.
        let all = [
            Query::Version,
            Query::AdvancedVersion,
            Query::Tool {
                tool: 0,
                what: ToolQuery::Temperature,
            },
            Query::BoardStatus,
            Query::BuildStatistics,
            Query::BuildName,
            Query::Position,
            Query::ReadEeprom { offset: 0, len: 31 },
        ];
        for q in all {
            let code = q.payload()[0];
            assert!(code < 128, "{q:?} must be a query");
            assert!(code != 3 && code != 7, "{q:?} would restart the board");
        }
    }

    #[test]
    fn printing_questions_follow_the_specification() {
        assert_eq!(Query::ExtendedStop.payload(), vec![22, 0b11]);
        assert_eq!(Query::PauseResume.payload(), vec![8]);
        assert_eq!(
            Query::PlayFile {
                name: "cube.x3g".into()
            }
            .payload(),
            b"\x10cube.x3g\0".to_vec()
        );
        assert_eq!(Query::NextFileName { restart: true }.payload(), vec![18, 1]);
        for q in [
            Query::PauseResume,
            Query::ExtendedStop,
            Query::EndCapture,
            Query::NextFileName { restart: false },
        ] {
            let code = q.payload()[0];
            assert!(code != 3 && code != 7, "{q:?} would restart the board");
        }
    }

    #[test]
    fn makes_sd_card_names_and_reads_the_file_list() {
        assert_eq!(sd_file_name("XYZ calibration cube.gcode"), "XYZcalib.x3g");
        assert_eq!(sd_file_name("ORBIT_print-in-place"), "ORBIT_pr.x3g");
        assert_eq!(sd_file_name("äöü"), "print.x3g");
        assert!(sd_file_name("a-very-long-name").len() <= SD_NAME_MAX);
        assert_eq!(
            read_next_file_name(b"\0CUBE.X3G\0"),
            Ok(Some("CUBE.X3G".into()))
        );
        assert_eq!(read_next_file_name(b"\0\0"), Ok(None));
        assert_eq!(read_next_file_name(&[1, 0]), Err(1));
        assert_eq!(sd_problem(1), "there is no SD card in the printer");
    }

    #[test]
    fn tool_and_eeprom_questions_follow_the_specification() {
        let q = Query::Tool {
            tool: 1,
            what: ToolQuery::Target,
        };
        assert_eq!(q.payload(), vec![10, 1, 32]);
        let q = Query::Tool {
            tool: 0,
            what: ToolQuery::BedTemperature,
        };
        assert_eq!(q.payload(), vec![10, 0, 30]);
        assert_eq!(
            Query::ReadEeprom {
                offset: 0x01A4,
                len: 200
            }
            .payload(),
            vec![12, 0xA4, 0x01, 31],
            "a read is capped at what one packet carries"
        );
    }

    #[test]
    fn reads_versions_and_names_the_variant() {
        assert_eq!(read_version(&[0xC4, 0x02]), Some(708));
        assert_eq!(version_text(708), "7.8");
        assert_eq!(version_text(705), "7.5");
        let a = read_advanced_version(&[0xC4, 0x02, 0, 0, 0x80, 0, 0, 0]).unwrap();
        assert_eq!((a.firmware, a.variant), (708, 0x80));
        assert_eq!(variant_name(0x80), "Sailfish");
        assert_eq!(variant_name(2), "Sailfish");
        assert_eq!(variant_name(1), "MakerBot firmware");
        assert_eq!(read_version(&[0x01]), None);
    }

    #[test]
    fn reads_temperatures_states_and_positions() {
        assert_eq!(read_temperature(&[0xD2, 0x00]), Some(210.0));
        assert_eq!(read_temperature(&[0xFB, 0xFF]), Some(-5.0));
        assert_eq!(
            read_build_statistics(&[3, 1, 20, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(BuildStatistics {
                state: BuildState::Paused,
                hours: 1,
                minutes: 20
            })
        );
        assert_eq!(read_build_statistics(&[9, 0, 0]), None);
        let mut pos = Vec::new();
        for v in [100i32, -200, 3, 0, 7] {
            pos.extend(v.to_le_bytes());
        }
        assert_eq!(read_position(&pos), Some([100, -200, 3, 0, 7]));
        assert_eq!(read_text(b"cube.x3g\0junk"), "cube.x3g");
        assert_eq!(
            board_status_words(0b0100_0001),
            vec!["preheating", "heaters shut down"]
        );
    }

    fn put_u32(e: &mut [u8], at: usize, v: u32) {
        e[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn reads_machine_settings_from_the_eeprom() {
        let mut e = vec![0xFF; EEPROM_SIZE as usize];
        e[0x22..0x22 + 11].copy_from_slice(b"Creator Pro");
        e[0x22 + 11] = 0;
        e[0x42] = 2;
        e[0x4C] = 1;
        e[0x0C] = 0b011; // X and Y home to max
        put_u32(&mut e, 0x01A4, 94_117_647);
        put_u32(&mut e, 0x01A4 + 8, 400_000_000);
        put_u32(&mut e, 0x018C, (227.0 * 94.117647f64).round() as u32);
        put_u32(&mut e, 0x0162, 15);
        e[0x0FFF] = 1;
        let m = MachineSettings::from_eeprom(&e);
        assert_eq!(m.name, "Creator Pro");
        assert_eq!(m.tool_count, Some(2));
        assert_eq!(m.heated_bed, Some(true));
        assert_eq!(m.ditto, Some(true));
        assert!((m.steps_per_mm[0].unwrap() - 94.117647).abs() < 1e-9);
        assert_eq!(m.steps_per_mm[1], None, "erased EEPROM reads as unknown");
        assert_eq!(m.toolhead_offset_steps[0], Some(15));
        let text = m.summary().join("\n");
        assert!(text.contains("\"Creator Pro\": 2 nozzle(s)"), "{text}");
        assert!(text.contains("Ditto printing on"), "{text}");
        assert!(text.contains("length 227.0 mm"), "{text}");
        assert!(text.contains("X: 94.117647 steps/mm"), "{text}");
        assert!(text.contains("homes to max"), "{text}");
    }

    #[test]
    fn a_short_eeprom_copy_reads_as_unknown() {
        let m = MachineSettings::from_eeprom(&[0; 10]);
        assert_eq!(m.tool_count, None);
        assert_eq!(m.ditto, None);
        assert!(m.summary()[0].contains("(no name)"));
    }
}
