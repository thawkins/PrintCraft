//! S3G "action" commands (numbers 128 and up): the printer queues them and carries them
//! out in order. An `.x3g` file is these commands back to back, without packet framing;
//! over USB each one is sent in its own packet.
//!
//! Layouts are from MakerBot's protocol specification and checked against Sailfish 7.8's
//! `Command.cc` (how many bytes each handler takes, and in what order).

/// Axis bits, used by homing, enabling and relative moves.
pub const X: u8 = 1 << 0;
pub const Y: u8 = 1 << 1;
pub const Z: u8 = 1 << 2;
pub const A: u8 = 1 << 3;
pub const B: u8 = 1 << 4;
pub const XYZ: u8 = X | Y | Z;
pub const ALL_AXES: u8 = X | Y | Z | A | B;

/// The longest text one packet can carry in a display message (32-byte payload, less
/// the command and its four option bytes, less the closing zero).
const MESSAGE_CHUNK: usize = 32 - 5 - 1;
/// The longest build name: 32 bytes, less the command, its 4 reserved bytes and the zero.
const BUILD_NAME_MAX: usize = 32 - 5 - 1;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// 131 / 132: move the given axes to their endstops. `step_us` is the time between
    /// steps on the fastest axis.
    Home {
        to_max: bool,
        axes: u8,
        step_us: u32,
        timeout_s: u16,
    },
    /// 133.
    Delay { ms: u32 },
    /// 134.
    ChangeTool { tool: u8 },
    /// 135.
    WaitForTool { tool: u8, timeout_s: u16 },
    /// 136, subcommand 3.
    SetNozzleTemp { tool: u8, celsius: i16 },
    /// 136, subcommand 31 (sent through tool 0).
    SetBedTemp { celsius: i16 },
    /// 136, subcommand 12: the part-cooling fan on that tool, on or off.
    Fan { tool: u8, on: bool },
    /// 137: bit 7 set enables the axes in the low bits, clear disables them.
    EnableAxes { axes: u8, enable: bool },
    /// 140: where the machine is, in steps.
    SetPosition { steps: [i32; 5] },
    /// 141.
    WaitForBed { timeout_s: u16 },
    /// 144: take the home position stored in the printer's EEPROM for these axes.
    RecallHome { axes: u8 },
    /// 149: one piece of a message on the printer's screen. The first piece clears what
    /// was shown; the last one puts the message up.
    Message {
        text: String,
        first: bool,
        last: bool,
    },
    /// 150.
    BuildPercent { percent: u8 },
    /// 153.
    BuildStart { name: String },
    /// 154.
    BuildEnd,
    /// 155: a move. X, Y and Z are absolute unless their bit is in `relative`; the
    /// extruders are always sent relative. `dda_rate` is steps per second on the axis with
    /// the most steps; `distance_mm` is the XYZ length (or the extrusion length for a move
    /// that only extrudes); `feedrate` is mm/s.
    Move {
        steps: [i32; 5],
        dda_rate: u32,
        relative: u8,
        distance_mm: f32,
        feedrate: f32,
    },
    /// 157: the first command of an X3G file. `bot_pid` is the USB product id of the
    /// board family the file is made for.
    StreamVersion { bot_pid: u16 },
}

impl Action {
    /// The command's bytes (its number first).
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Action::Home {
                to_max,
                axes,
                step_us,
                timeout_s,
            } => {
                b.push(if *to_max { 132 } else { 131 });
                b.push(*axes);
                b.extend(step_us.to_le_bytes());
                b.extend(timeout_s.to_le_bytes());
            }
            Action::Delay { ms } => {
                b.push(133);
                b.extend(ms.to_le_bytes());
            }
            Action::ChangeTool { tool } => b.extend([134, *tool]),
            Action::WaitForTool { tool, timeout_s } => {
                b.extend([135, *tool]);
                b.extend(100u16.to_le_bytes()); // ask every 100 ms, as the spec suggests
                b.extend(timeout_s.to_le_bytes());
            }
            Action::SetNozzleTemp { tool, celsius } => {
                b.extend([136, *tool, 3, 2]);
                b.extend(celsius.to_le_bytes());
            }
            Action::SetBedTemp { celsius } => {
                b.extend([136, 0, 31, 2]);
                b.extend(celsius.to_le_bytes());
            }
            Action::Fan { tool, on } => b.extend([136, *tool, 12, 1, u8::from(*on)]),
            Action::EnableAxes { axes, enable } => {
                b.extend([137, (axes & ALL_AXES) | if *enable { 0x80 } else { 0 }])
            }
            Action::SetPosition { steps } => {
                b.push(140);
                for s in steps {
                    b.extend(s.to_le_bytes());
                }
            }
            Action::WaitForBed { timeout_s } => {
                b.extend([141, 0]);
                b.extend(100u16.to_le_bytes());
                b.extend(timeout_s.to_le_bytes());
            }
            Action::RecallHome { axes } => b.extend([144, *axes]),
            Action::Message { text, first, last } => {
                // Options (Sailfish `Command.cc`): bit 0 clear wipes the screen first;
                // bit 1 set shows the message, so it marks the last piece.
                let options = u8::from(!*first) | (u8::from(*last) << 1);
                b.extend([149, options, 0, 0, 0]);
                b.extend(ascii(text, MESSAGE_CHUNK));
                b.push(0);
            }
            Action::BuildPercent { percent } => b.extend([150, (*percent).min(100), 0]),
            Action::BuildStart { name } => {
                b.push(153);
                b.extend(0u32.to_le_bytes());
                b.extend(ascii(name, BUILD_NAME_MAX));
                b.push(0);
            }
            Action::BuildEnd => b.extend([154, 0]),
            Action::Move {
                steps,
                dda_rate,
                relative,
                distance_mm,
                feedrate,
            } => {
                b.push(155);
                for s in steps {
                    b.extend(s.to_le_bytes());
                }
                b.extend(dda_rate.to_le_bytes());
                b.push(relative & ALL_AXES);
                b.extend(distance_mm.to_le_bytes());
                let f64x = (feedrate * 64.0).round().clamp(0.0, u16::MAX as f32) as u16;
                b.extend(f64x.to_le_bytes());
            }
            Action::StreamVersion { bot_pid } => {
                // Version 0.0, as GPX writes it; Sailfish reads the 20 bytes and ignores them.
                b.extend([157, 0, 0, 0]);
                b.extend(0u32.to_le_bytes());
                b.extend(bot_pid.to_le_bytes());
                b.extend(0u16.to_le_bytes());
                b.extend(0u32.to_le_bytes());
                b.extend(0u32.to_le_bytes());
                b.push(0);
            }
        }
        b
    }

    /// A display message split into pieces that each fit one packet.
    pub fn message(text: &str) -> Vec<Action> {
        let clean = String::from_utf8(ascii(text, usize::MAX)).unwrap_or_default();
        let chars: Vec<char> = clean.chars().collect();
        let pieces: Vec<String> = chars
            .chunks(MESSAGE_CHUNK)
            .map(|c| c.iter().collect())
            .collect();
        let n = pieces.len().max(1);
        let pieces = if pieces.is_empty() {
            vec![String::new()]
        } else {
            pieces
        };
        pieces
            .into_iter()
            .enumerate()
            .map(|(i, text)| Action::Message {
                text,
                first: i == 0,
                last: i + 1 == n,
            })
            .collect()
    }
}

/// Printable ASCII only (the printer's screen has no other characters), cut to `max`.
fn ascii(text: &str, max: usize) -> Vec<u8> {
    text.chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .take(max)
        .map(|c| c as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_fits_one_packet_and_has_sailfish_length() {
        // Lengths Sailfish's Command.cc waits for before it carries a command out.
        let cases: Vec<(Action, usize)> = vec![
            (
                Action::Home {
                    to_max: true,
                    axes: X | Y,
                    step_us: 500,
                    timeout_s: 60,
                },
                8,
            ),
            (Action::Delay { ms: 10 }, 5),
            (Action::ChangeTool { tool: 1 }, 2),
            (
                Action::WaitForTool {
                    tool: 0,
                    timeout_s: 600,
                },
                6,
            ),
            (
                Action::SetNozzleTemp {
                    tool: 0,
                    celsius: 210,
                },
                6,
            ),
            (Action::SetBedTemp { celsius: 60 }, 6),
            (Action::Fan { tool: 0, on: true }, 5),
            (
                Action::EnableAxes {
                    axes: ALL_AXES,
                    enable: false,
                },
                2,
            ),
            (Action::SetPosition { steps: [0; 5] }, 21),
            (Action::WaitForBed { timeout_s: 600 }, 6),
            (Action::RecallHome { axes: XYZ }, 2),
            (Action::BuildPercent { percent: 5 }, 3),
            (Action::BuildEnd, 2),
            (
                Action::Move {
                    steps: [1, 2, 3, 4, 5],
                    dda_rate: 1000,
                    relative: A | B,
                    distance_mm: 1.5,
                    feedrate: 40.0,
                },
                32,
            ),
            (Action::StreamVersion { bot_pid: 0xD314 }, 21),
        ];
        for (a, len) in cases {
            let bytes = a.encode();
            assert_eq!(bytes.len(), len, "{a:?}");
            assert!(bytes.len() <= crate::x3g::MAX_PAYLOAD, "{a:?}");
            assert!(bytes[0] >= 128, "{a:?} must be an action");
        }
    }

    #[test]
    fn a_move_is_laid_out_as_sailfish_reads_it() {
        let b = Action::Move {
            steps: [100, -200, 300, -40, 0],
            dda_rate: 2500,
            relative: A | B,
            distance_mm: 12.5,
            feedrate: 40.0,
        }
        .encode();
        assert_eq!(b[0], 155);
        assert_eq!(i32::from_le_bytes(b[1..5].try_into().unwrap()), 100);
        assert_eq!(i32::from_le_bytes(b[5..9].try_into().unwrap()), -200);
        assert_eq!(i32::from_le_bytes(b[13..17].try_into().unwrap()), -40);
        assert_eq!(u32::from_le_bytes(b[21..25].try_into().unwrap()), 2500);
        assert_eq!(b[25], A | B);
        assert_eq!(f32::from_le_bytes(b[26..30].try_into().unwrap()), 12.5);
        assert_eq!(u16::from_le_bytes(b[30..32].try_into().unwrap()), 40 * 64);
    }

    #[test]
    fn temperatures_and_homing_follow_the_specification() {
        assert_eq!(
            Action::SetNozzleTemp {
                tool: 1,
                celsius: 230
            }
            .encode(),
            vec![136, 1, 3, 2, 230, 0]
        );
        assert_eq!(
            Action::SetBedTemp { celsius: 110 }.encode(),
            vec![136, 0, 31, 2, 110, 0]
        );
        assert_eq!(
            Action::Home {
                to_max: false,
                axes: Z,
                step_us: 1000,
                timeout_s: 60
            }
            .encode(),
            vec![131, Z, 0xE8, 0x03, 0, 0, 60, 0]
        );
        assert_eq!(
            Action::EnableAxes {
                axes: A,
                enable: true
            }
            .encode(),
            vec![137, 0x80 | A]
        );
    }

    #[test]
    fn long_texts_are_split_and_cleaned() {
        let parts = Action::message("Printing a very long name, more than one packet: café");
        assert_eq!(parts.len(), 3);
        for p in &parts {
            assert!(p.encode().len() <= crate::x3g::MAX_PAYLOAD);
        }
        let first = parts[0].encode();
        assert_eq!(
            first[1], 0,
            "the first piece clears the screen, not the last"
        );
        let last = parts[2].encode();
        assert_eq!(last[1], 0b11, "the last piece keeps the text and shows it");
        assert!(last.ends_with(b"\0"), "each piece is zero-terminated");
        let text: String = parts
            .iter()
            .map(|p| match p {
                Action::Message { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(text.ends_with("caf_"), "non-ASCII becomes _: {text}");
        let start = Action::BuildStart {
            name: "a".repeat(60),
        }
        .encode();
        assert_eq!(start.len(), 32);
    }
}
