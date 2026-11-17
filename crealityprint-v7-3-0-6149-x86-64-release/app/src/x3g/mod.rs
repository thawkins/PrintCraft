//! The MakerBot S3G protocol, spoken by Sailfish and MakerBot firmware (FlashForge
//! Creator Pro, early Creator Pro 2, Replicator 1/2/2X and their clones). An `.x3g` file
//! is a sequence of the same commands without the packet framing.
//!
//! Sources: MakerBot's protocol specification (`doc/s3gProtocol.md` in the `makerbot/s3g`
//! repository) and the Sailfish firmware source (`Packet.hh`, `Packet.cc`, `Commands.hh`,
//! `Host.cc`). See makerbot.md for what was checked and how.
//!
//! A packet is `0xD5`, the payload length, the payload (at most 32 bytes) and a CRC-8 of
//! the payload. The printer answers every packet with a packet whose first payload byte
//! is a [`Reply`] code. Integers are little-endian.

pub mod command;
pub mod machine;
pub mod query;
pub mod translate;

/// Makes an `.x3g` file from a sliced plate's G-code for a MakerBot-protocol printer
/// profile. Returns the file and anything the printer can't do that was left out.
///
/// The machine is the FlashForge Creator Pro family (the only one so far). Once the
/// printer has been connected over USB, its own settings (the EEPROM copy saved then)
/// replace the family's values, and a note says so.
pub fn export(
    gcode: &str,
    printer: &crate::profiles::Printer,
    job_name: &str,
    layers: usize,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let options = translate::Options {
        bed_mm: (printer.bed_x as f64, printer.bed_y as f64),
        job_name: job_name.to_string(),
        total_layers: layers as u32,
    };
    let (machine, note) = export_machine();
    let t = translate::translate(gcode, &machine, &options).map_err(|e| e.to_string())?;
    let mut notes = t.warnings;
    notes.extend(note);
    Ok((translate::to_x3g(&t.actions), notes))
}

/// The Creator Pro family's values, with the connected printer's own settings in their
/// place when a copy of its EEPROM has been saved.
fn export_machine() -> (machine::Machine, Option<String>) {
    let base = machine::Machine::creator_pro();
    // Tests must not depend on a printer once connected on this computer.
    if cfg!(test) {
        return (base, None);
    }
    let Some(path) = crate::fsutil::settings_dir().map(|d| {
        d.join("logs")
            .join(crate::printer::sailfish::EEPROM_FILE_NAME)
    }) else {
        return (base, None);
    };
    match std::fs::read(&path) {
        // Up to the end of the axis settings (FREE_EEPROM_STARTS in Sailfish's map).
        Ok(e) if e.len() >= 0x020C => {
            let settings = query::MachineSettings::from_eeprom(&e);
            (
                base.with_settings(&settings),
                Some(format!(
                    "Steps per mm, speeds and home settings were taken from the printer's \
                     own settings ({}).",
                    path.display()
                )),
            )
        }
        _ => (base, None),
    }
}

/// First byte of every packet.
pub const START: u8 = 0xD5;
/// Sailfish's `MAX_PACKET_PAYLOAD`.
pub const MAX_PAYLOAD: usize = 32;

/// The iButton/Maxim CRC-8 (reflected polynomial 0x8C, starting at 0), as Sailfish
/// computes it with avr-libc's `_crc_ibutton_update`.
pub fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |mut crc, &b| {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x8C
            } else {
                crc >> 1
            };
        }
        crc
    })
}

/// Wraps a payload in a packet. Panics on a payload over [`MAX_PAYLOAD`]: every payload
/// is built by this module, so that is a programming error.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    assert!(
        payload.len() <= MAX_PAYLOAD,
        "S3G payload too long: {}",
        payload.len()
    );
    let mut out = Vec::with_capacity(payload.len() + 3);
    out.push(START);
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    out.push(crc8(payload));
    out
}

/// The printer's answer code, the first byte of every reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// 0x80: the packet was damaged or not understood; send it again.
    GenericError,
    /// 0x81.
    Ok,
    /// 0x82: the command queue is full; send it again later.
    BufferFull,
    /// 0x83: the CRC didn't match; send it again.
    CrcMismatch,
    /// 0x84.
    QueryTooBig,
    /// 0x85.
    Unsupported,
    /// 0x87.
    DownstreamTimeout,
    /// 0x88: send it again.
    ToolLockTimeout,
    /// 0x89: the person cancelled the build on the printer.
    Cancelled,
    /// 0x8A: the printer is printing from its SD card.
    BuildingFromSd,
    /// 0x8B: the printer shut down because something overheated.
    Overheat,
    /// 0x8C: the packet arrived too slowly; send it again.
    PacketTimeout,
    Other(u8),
}

impl Reply {
    pub fn from_code(code: u8) -> Reply {
        match code {
            0x80 => Reply::GenericError,
            0x81 => Reply::Ok,
            0x82 => Reply::BufferFull,
            0x83 => Reply::CrcMismatch,
            0x84 => Reply::QueryTooBig,
            0x85 => Reply::Unsupported,
            0x87 => Reply::DownstreamTimeout,
            0x88 => Reply::ToolLockTimeout,
            0x89 => Reply::Cancelled,
            0x8A => Reply::BuildingFromSd,
            0x8B => Reply::Overheat,
            0x8C => Reply::PacketTimeout,
            c => Reply::Other(c),
        }
    }

    /// Whether the same packet may simply be sent again (the specification's
    /// "retryable" codes, and a full buffer, which clears as the printer works).
    pub fn worth_resending(self) -> bool {
        matches!(
            self,
            Reply::GenericError
                | Reply::BufferFull
                | Reply::CrcMismatch
                | Reply::ToolLockTimeout
                | Reply::PacketTimeout
        )
    }

    pub fn describe(self) -> String {
        match self {
            Reply::GenericError => "packet error".into(),
            Reply::Ok => "ok".into(),
            Reply::BufferFull => "buffer full".into(),
            Reply::CrcMismatch => "CRC mismatch".into(),
            Reply::QueryTooBig => "query too big".into(),
            Reply::Unsupported => "command not supported".into(),
            Reply::DownstreamTimeout => "toolhead timed out".into(),
            Reply::ToolLockTimeout => "tool lock timed out".into(),
            Reply::Cancelled => "build cancelled on the printer".into(),
            Reply::BuildingFromSd => "printing from the SD card".into(),
            Reply::Overheat => "shut down: overheated".into(),
            Reply::PacketTimeout => "packet timed out".into(),
            Reply::Other(c) => format!("unknown reply 0x{c:02X}"),
        }
    }
}

/// What [`PacketReader`] found in the bytes it was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// A whole packet's payload, CRC checked.
    Packet(Vec<u8>),
    /// A packet whose CRC didn't match (its payload, for the log).
    Damaged(Vec<u8>),
}

/// Collects packets from bytes as they arrive from the port. Bytes before a start byte
/// are skipped (noise, or the boot messages some boards print).
#[derive(Debug, Default)]
pub struct PacketReader {
    buf: Vec<u8>,
}

impl PacketReader {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole packet, if one has arrived.
    pub fn next_packet(&mut self) -> Option<Incoming> {
        loop {
            let start = self.buf.iter().position(|&b| b == START);
            match start {
                None => {
                    self.buf.clear();
                    return None;
                }
                Some(i) => {
                    self.buf.drain(..i);
                }
            }
            let len = *self.buf.get(1)? as usize;
            if len > MAX_PAYLOAD {
                // Not a real length: this 0xD5 was noise. Look for the next one.
                self.buf.remove(0);
                continue;
            }
            if self.buf.len() < len + 3 {
                return None;
            }
            let packet: Vec<u8> = self.buf.drain(..len + 3).collect();
            let payload = packet[2..2 + len].to_vec();
            return Some(if crc8(&payload) == packet[2 + len] {
                Incoming::Packet(payload)
            } else {
                Incoming::Damaged(payload)
            });
        }
    }

    /// Forgets anything half-received (after a timeout, before sending again).
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

/// Bytes as `D5 03 0A 00 02`, for the serial log.
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_maxim_check_value() {
        // The standard check value of CRC-8/MAXIM for the text "123456789".
        assert_eq!(crc8(b"123456789"), 0xA1);
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn frames_a_payload_with_length_and_crc() {
        let p = frame(&[0x00, 0x64, 0x00]);
        assert_eq!(p[..5], [0xD5, 3, 0x00, 0x64, 0x00]);
        assert_eq!(p[5], crc8(&[0x00, 0x64, 0x00]));
    }

    #[test]
    #[should_panic(expected = "too long")]
    fn refuses_an_oversized_payload() {
        frame(&[0; MAX_PAYLOAD + 1]);
    }

    #[test]
    fn reads_packets_split_across_reads_and_skips_noise() {
        let mut r = PacketReader::default();
        let mut bytes = b"boot noise\r\n".to_vec();
        bytes.extend(frame(&[0x81, 0xC4, 0x02]));
        bytes.extend(frame(&[0x81]));
        let (a, b) = bytes.split_at(15);
        r.push(a);
        assert_eq!(
            r.next_packet(),
            None,
            "only part of the first packet so far"
        );
        r.push(b);
        assert_eq!(
            r.next_packet(),
            Some(Incoming::Packet(vec![0x81, 0xC4, 0x02]))
        );
        assert_eq!(r.next_packet(), Some(Incoming::Packet(vec![0x81])));
        assert_eq!(r.next_packet(), None);
    }

    #[test]
    fn reports_a_damaged_packet() {
        let mut bytes = frame(&[0x81, 0x10]);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let mut r = PacketReader::default();
        r.push(&bytes);
        assert_eq!(r.next_packet(), Some(Incoming::Damaged(vec![0x81, 0x10])));
    }

    #[test]
    fn a_start_byte_with_an_impossible_length_is_noise() {
        let mut r = PacketReader::default();
        let mut bytes = vec![0xD5, 200];
        bytes.extend(frame(&[0x81]));
        r.push(&bytes);
        assert_eq!(r.next_packet(), Some(Incoming::Packet(vec![0x81])));
    }

    #[test]
    fn reply_codes_follow_the_specification() {
        assert_eq!(Reply::from_code(0x81), Reply::Ok);
        assert_eq!(Reply::from_code(0x85), Reply::Unsupported);
        assert_eq!(Reply::from_code(0x8A), Reply::BuildingFromSd);
        assert_eq!(Reply::from_code(0x86), Reply::Other(0x86));
        assert!(Reply::CrcMismatch.worth_resending());
        assert!(Reply::BufferFull.worth_resending());
        assert!(!Reply::Unsupported.worth_resending());
        assert!(!Reply::Overheat.worth_resending());
    }
}
