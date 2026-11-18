//! Binary G-code (`.bgcode`), the print file Prusa printers read fastest: the same
//! G-code, packed (MeatPack) and compressed (heatshrink) in checksummed blocks, with the
//! thumbnails and the job's details in blocks of their own.
//!
//! Written from the published format description (prusa3d/libbgcode
//! `doc/specifications.md`) plus facts about details it leaves out (the checksum's
//! coverage, the INI layout, MeatPack's character table, heatshrink's bit stream), not
//! from anyone's code. The layout:
//!
//! * File header: `GCDE`, version 1 (u32), checksum type 1 = CRC32 (u16). All numbers are
//!   little-endian.
//! * Blocks, in order: file metadata, printer metadata, thumbnails, print metadata,
//!   slicer metadata, then G-code. Each block is a header (type u16, compression u16,
//!   uncompressed size u32, and the compressed size u32 when compressed), its parameters
//!   (u16 encoding, or u16 format/width/height for a thumbnail), its data, and a CRC32
//!   (standard CRC-32, as zlib's) over header, parameters and stored data.
//! * Metadata is INI: `key=value` lines, each ending in a newline, never compressed (the
//!   printers ignore compressed metadata). Thumbnails are PNG, uncompressed.
//! * G-code goes in blocks of up to 64 KiB of whole lines, each packed with MeatPack
//!   (keeping comment lines) and compressed with heatshrink (window 2^12, lookahead 2^4),
//!   the combination Prusa's firmware reads.
//!
//! The plain file is the input: its `; thumbnail begin` pictures become thumbnail blocks
//! and its settings block (`; key = value` lines) the metadata, so a file made for any
//! printer can be turned into binary as the last step.

use crate::gcode::Estimate;

/// Block types.
const FILE_METADATA: u16 = 0;
const GCODE: u16 = 1;
const SLICER_METADATA: u16 = 2;
const PRINTER_METADATA: u16 = 3;
const PRINT_METADATA: u16 = 4;
const THUMBNAIL: u16 = 5;
/// Compression types.
const NO_COMPRESSION: u16 = 0;
const HEATSHRINK_12_4: u16 = 3;
/// G-code encoding: MeatPack, keeping comment lines.
const MEATPACK_COMMENTS: u16 = 2;
/// Thumbnail format: PNG.
const PNG: u16 = 0;
/// Raw G-code (before packing) per block, as the reference writer uses.
const GCODE_BLOCK_BYTES: usize = 65536;

/// Settings-block keys also written as printer metadata (the ones a printer checks the
/// file against), when the file has them.
const PRINTER_KEYS: [&str; 7] = [
    "printer_model",
    "filament_type",
    "nozzle_diameter",
    "temperature",
    "bed_temperature",
    "layer_height",
    "filament_diameter",
];

/// The binary file for the plain print file `gcode`, with `estimate`'s time and
/// filament in the metadata.
pub fn encode(gcode: &str, estimate: &Estimate) -> Vec<u8> {
    let (body, pictures) = split_thumbnails(gcode);
    let settings = settings_block(gcode);
    let totals = vec![
        (
            "filament used [mm]".to_string(),
            format!("{:.2}", estimate.filament_mm),
        ),
        (
            "filament used [g]".to_string(),
            format!("{:.2}", estimate.grams),
        ),
        (
            "estimated printing time (normal mode)".to_string(),
            duration_text(estimate.seconds),
        ),
    ];
    let mut printer: Vec<(String, String)> = PRINTER_KEYS
        .iter()
        .filter_map(|k| {
            settings
                .iter()
                .find(|(key, _)| key == k)
                .map(|(k, v)| (k.clone(), v.clone()))
        })
        .collect();
    printer.extend(totals.iter().cloned());
    let producer = vec![(
        "Producer".to_string(),
        format!("PrintCraft {}", env!("CARGO_PKG_VERSION")),
    )];

    let mut out = Vec::with_capacity(gcode.len() / 4);
    out.extend_from_slice(b"GCDE");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    let meta = |out: &mut Vec<u8>, kind: u16, pairs: &[(String, String)]| {
        write_block(out, kind, NO_COMPRESSION, &[0, 0], &ini(pairs), None);
    };
    meta(&mut out, FILE_METADATA, &producer);
    meta(&mut out, PRINTER_METADATA, &printer);
    for (w, h, png) in &pictures {
        let mut params = Vec::with_capacity(6);
        for v in [PNG, *w, *h] {
            params.extend_from_slice(&v.to_le_bytes());
        }
        write_block(&mut out, THUMBNAIL, NO_COMPRESSION, &params, png, None);
    }
    meta(&mut out, PRINT_METADATA, &totals);
    meta(&mut out, SLICER_METADATA, &settings);
    for chunk in line_chunks(&body, GCODE_BLOCK_BYTES) {
        let packed = meatpack(chunk);
        let squeezed = heatshrink(&packed);
        write_block(
            &mut out,
            GCODE,
            HEATSHRINK_12_4,
            &MEATPACK_COMMENTS.to_le_bytes(),
            &squeezed,
            Some(packed.len()),
        );
    }
    out
}

/// One block: header, parameters, the stored `data` and its CRC32. `unpacked` is the
/// size before compression when `data` is compressed.
fn write_block(
    out: &mut Vec<u8>,
    kind: u16,
    compression: u16,
    params: &[u8],
    data: &[u8],
    unpacked: Option<usize>,
) {
    let start = out.len();
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&compression.to_le_bytes());
    match unpacked {
        Some(n) => {
            out.extend_from_slice(&(n as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        }
        None => out.extend_from_slice(&(data.len() as u32).to_le_bytes()),
    }
    out.extend_from_slice(params);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_le_bytes());
}

/// `key=value` lines, each ending in a newline.
fn ini(pairs: &[(String, String)]) -> Vec<u8> {
    pairs
        .iter()
        .flat_map(|(k, v)| format!("{k}={v}\n").into_bytes())
        .collect()
}

/// A time as PrusaSlicer writes it in metadata: `1h 2m 3s`, `5m 0s`.
fn duration_text(seconds: f32) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (d, h, m, s) = (s / 86400, s / 3600 % 24, s / 60 % 60, s % 60);
    match (d, h) {
        (0, 0) => format!("{m}m {s}s"),
        (0, _) => format!("{h}h {m}m {s}s"),
        _ => format!("{d}d {h}h {m}m {s}s"),
    }
}

/// The `; key = value` lines of the file's settings block (`; CONFIG_BLOCK_START` ..
/// `; CONFIG_BLOCK_END`), in order.
fn settings_block(gcode: &str) -> Vec<(String, String)> {
    gcode
        .lines()
        .skip_while(|l| l.trim() != "; CONFIG_BLOCK_START")
        .skip(1)
        .take_while(|l| l.trim() != "; CONFIG_BLOCK_END")
        .filter_map(|l| {
            let (k, v) = l.strip_prefix(';')?.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// The file without its `; thumbnail begin` .. `; thumbnail end` blocks, and the
/// pictures they held (width, height, PNG bytes).
fn split_thumbnails(gcode: &str) -> (String, Vec<(u16, u16, Vec<u8>)>) {
    let mut body = String::with_capacity(gcode.len());
    let mut pictures = Vec::new();
    let mut picture: Option<(u16, u16, String)> = None;
    for line in gcode.split_inclusive('\n') {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("; thumbnail begin ") {
            let size = rest.split_whitespace().next().unwrap_or("");
            let (w, h) = size.split_once('x').unwrap_or(("0", "0"));
            picture = Some((
                w.parse().unwrap_or(0),
                h.parse().unwrap_or(0),
                String::new(),
            ));
        } else if t == "; thumbnail end" {
            if let Some((w, h, text)) = picture.take() {
                if let Some(png) = crate::codec::base64_decode(&text) {
                    pictures.push((w, h, png));
                }
            }
        } else if let Some((_, _, text)) = &mut picture {
            text.push_str(t.trim_start_matches(';').trim());
        } else {
            body.push_str(line);
        }
    }
    (body, pictures)
}

/// `text` cut into pieces of whole lines, each at most `max` bytes (a longer line is a
/// piece of its own).
fn line_chunks(text: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        if end > start && end - start + line.len() > max {
            out.push(&text[start..end]);
            start = end;
        }
        end += line.len();
    }
    if end > start {
        out.push(&text[start..end]);
    }
    out
}

/// Standard CRC-32 (reflected polynomial 0xEDB88320, start and final XOR 0xFFFFFFFF).
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, &b| {
        table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8)
    })
}

// ----- MeatPack ---------------------------------------------------------------------------

/// MeatPack's signal byte; two in a row are followed by a command.
const SIGNAL: u8 = 0xFF;
const ENABLE_PACKING: u8 = 251;
const DISABLE_PACKING: u8 = 250;
const ENABLE_NO_SPACES: u8 = 247;
/// The nibble meaning "a full character follows".
const FULL: u8 = 0xF;

/// The 4-bit code of `c` in no-spaces mode (where `E` takes the space's code), if it
/// has one.
fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'.' => Some(0xA),
        b'E' => Some(0xB),
        b'\n' => Some(0xC),
        b'G' => Some(0xD),
        b'X' => Some(0xE),
        _ => None,
    }
}

/// What MeatPack (keeping comments) makes of the lines of `text` once unpacked: each
/// line trimmed, blank lines dropped, comment lines kept, other lines cut at a `;`, and
/// G0-G3 moves without spaces.
pub fn meatpack_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let t = l.trim();
            if t.is_empty() {
                return None;
            }
            if t.starts_with(';') {
                return Some(t.to_string());
            }
            let code = t.split(';').next().unwrap_or("").trim();
            if code.is_empty() {
                return None;
            }
            let b = code.as_bytes();
            let mv = b.len() >= 2
                && b[0] == b'G'
                && (b'0'..=b'3').contains(&b[1])
                && b.get(2).is_none_or(|c| *c == b' ');
            Some(if mv {
                code.replace(' ', "")
            } else {
                code.to_string()
            })
        })
        .collect()
}

/// `text` packed with MeatPack in no-spaces mode, keeping comment lines (sent unpacked,
/// with packing switched off around them). Self-contained: it switches packing on at the
/// start and leaves no half-filled byte at the end.
fn meatpack(text: &str) -> Vec<u8> {
    let mut out = vec![
        SIGNAL,
        SIGNAL,
        ENABLE_PACKING,
        SIGNAL,
        SIGNAL,
        ENABLE_NO_SPACES,
    ];
    let mut packing = true;
    let mut pending: Option<u8> = None;
    let flush = |out: &mut Vec<u8>, pending: &mut Option<u8>| {
        // Pairs a lone character with a newline (an empty line, which does nothing).
        if let Some(c) = pending.take() {
            pack_pair(out, c, b'\n');
        }
    };
    for line in meatpack_lines(text) {
        if line.starts_with(';') {
            flush(&mut out, &mut pending);
            if packing {
                out.extend_from_slice(&[SIGNAL, SIGNAL, DISABLE_PACKING]);
                packing = false;
            }
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
            continue;
        }
        if !packing {
            out.extend_from_slice(&[SIGNAL, SIGNAL, ENABLE_PACKING]);
            packing = true;
        }
        for &c in line.as_bytes().iter().chain(std::iter::once(&b'\n')) {
            match pending.take() {
                Some(first) => pack_pair(&mut out, first, c),
                None => pending = Some(c),
            }
        }
    }
    if packing {
        flush(&mut out, &mut pending);
    }
    out
}

/// Two characters in one byte (the first in the low nibble), each one that has no code
/// sent in full after it.
fn pack_pair(out: &mut Vec<u8>, first: u8, second: u8) {
    let (a, b) = (nibble(first), nibble(second));
    out.push(b.unwrap_or(FULL) << 4 | a.unwrap_or(FULL));
    if a.is_none() {
        out.push(first);
    }
    if b.is_none() {
        out.push(second);
    }
}

// ----- heatshrink -------------------------------------------------------------------------

/// Window and lookahead sizes (powers of two) of heatshrink 12/4.
const WINDOW_BITS: u32 = 12;
const LOOKAHEAD_BITS: u32 = 4;
/// Shortest repeat worth a back-reference (one costs 1 + 12 + 4 bits).
const MIN_MATCH: usize = 3;
/// How many earlier places with the same 3 bytes are tried for each match.
const CHAIN_DEPTH: usize = 128;

/// Writes bits, most significant first.
struct Bits {
    out: Vec<u8>,
    byte: u8,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        for i in (0..count).rev() {
            self.byte = self.byte << 1 | ((value >> i) & 1) as u8;
            self.used += 1;
            if self.used == 8 {
                self.out.push(self.byte);
                self.byte = 0;
                self.used = 0;
            }
        }
    }

    /// The last byte, its unused low bits zero.
    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.out.push(self.byte << (8 - self.used));
        }
        self.out
    }
}

/// `data` compressed with heatshrink (window 2^12, lookahead 2^4): a 1 bit and the byte
/// for a literal, or a 0 bit, the distance back minus one (12 bits) and the length
/// minus one (4 bits) for a repeat. Repeats are found greedily through chains of earlier
/// places that start with the same three bytes, and never reach before the start.
pub fn heatshrink(data: &[u8]) -> Vec<u8> {
    let window = 1usize << WINDOW_BITS;
    let longest = 1usize << LOOKAHEAD_BITS;
    let mut bits = Bits {
        out: Vec::with_capacity(data.len() / 2),
        byte: 0,
        used: 0,
    };
    const HASH_BITS: u32 = 15;
    let hash = |i: usize| -> usize {
        let v = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
        (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
    };
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut prev = vec![usize::MAX; data.len()];
    // Files position `i` under its first three bytes.
    let insert = |i: usize, head: &mut [usize], prev: &mut [usize]| {
        if i + MIN_MATCH <= data.len() {
            let h = hash(i);
            prev[i] = head[h];
            head[h] = i;
        }
    };
    let mut i = 0;
    while i < data.len() {
        let (mut best_len, mut best_dist) = (0, 0);
        if i + MIN_MATCH <= data.len() {
            let max = longest.min(data.len() - i);
            let mut at = head[hash(i)];
            let mut tries = 0;
            while at != usize::MAX && i - at <= window && tries < CHAIN_DEPTH {
                let len = (0..max)
                    .take_while(|&k| data[at + k] == data[i + k])
                    .count();
                if len > best_len {
                    (best_len, best_dist) = (len, i - at);
                    if len == max {
                        break;
                    }
                }
                at = prev[at];
                tries += 1;
            }
        }
        if best_len >= MIN_MATCH {
            bits.put(0, 1);
            bits.put((best_dist - 1) as u32, WINDOW_BITS);
            bits.put((best_len - 1) as u32, LOOKAHEAD_BITS);
            for k in i..i + best_len {
                insert(k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            bits.put(1, 1);
            bits.put(data[i] as u32, 8);
            insert(i, &mut head, &mut prev);
            i += 1;
        }
    }
    bits.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads heatshrink 12/4 back (the window starts as zeros).
    fn unheatshrink(data: &[u8], expected_len: usize) -> Vec<u8> {
        let mut pos = 0usize;
        let mut read = |n: u32| -> Option<u32> {
            let mut v = 0;
            for _ in 0..n {
                let byte = *data.get(pos / 8)?;
                v = v << 1 | ((byte >> (7 - pos % 8)) & 1) as u32;
                pos += 1;
            }
            Some(v)
        };
        let mut out: Vec<u8> = Vec::new();
        while out.len() < expected_len {
            let Some(tag) = read(1) else { break };
            if tag == 1 {
                out.push(read(8).unwrap() as u8);
            } else {
                let dist = read(WINDOW_BITS).unwrap() as usize + 1;
                let len = read(LOOKAHEAD_BITS).unwrap() as usize + 1;
                for _ in 0..len {
                    let b = if dist > out.len() {
                        0
                    } else {
                        out[out.len() - dist]
                    };
                    out.push(b);
                }
            }
        }
        out
    }

    /// Unpacks MeatPack (no-spaces mode once switched on), honouring its commands.
    fn unmeatpack(data: &[u8]) -> String {
        let table = b"0123456789.E\nGX";
        let mut out = Vec::new();
        let mut packing = false;
        let mut i = 0;
        while i < data.len() {
            if data[i] == SIGNAL && data.get(i + 1) == Some(&SIGNAL) {
                match data[i + 2] {
                    ENABLE_PACKING => packing = true,
                    DISABLE_PACKING => packing = false,
                    ENABLE_NO_SPACES => {}
                    c => panic!("unknown command {c}"),
                }
                i += 3;
                continue;
            }
            let b = data[i];
            i += 1;
            if !packing {
                out.push(b);
                continue;
            }
            for n in [b & 0xF, b >> 4] {
                if n == FULL {
                    out.push(data[i]);
                    i += 1;
                } else {
                    out.push(table[n as usize]);
                }
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn test_bgcode_crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn test_bgcode_heatshrink_bit_layout() {
        // 'a' as a literal, then four more 'a's as a repeat one byte back:
        // 1 01100001 | 0 000000000000 0011 | padding zeros.
        assert_eq!(heatshrink(b"aaaaa"), vec![0xB0, 0x80, 0x00, 0xC0]);
    }

    #[test]
    fn test_bgcode_heatshrink_round_trips() {
        let text: String = (0..3000)
            .map(|i| {
                format!(
                    "G1 X{:.3} Y{:.3} E{:.5}\n",
                    i as f32 * 0.37,
                    100.0 - i as f32 * 0.11,
                    i as f32 * 0.021
                )
            })
            .collect();
        let mut noise = Vec::new();
        let mut x: u32 = 12345;
        for _ in 0..5000 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            noise.push((x >> 16) as u8);
        }
        for data in [text.as_bytes(), &noise, b"", b"ab"] {
            let packed = heatshrink(data);
            assert_eq!(unheatshrink(&packed, data.len()), data);
        }
        assert!(heatshrink(text.as_bytes()).len() < text.len() / 2);
    }

    #[test]
    fn test_bgcode_meatpack_round_trips_to_the_cleaned_lines() {
        let text = "; header comment\n\nG28 ; home\nM104 S210\nG1 X10.5 Y20 E0.123 F1200\n  \
                    G0 F6000 X1 Y2\n;TYPE:WALL-OUTER\nG1 X1.5 Y2 E1\nM486 Apart one\nT0\n";
        let unpacked = unmeatpack(&meatpack(text));
        let lines: Vec<&str> = unpacked.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, meatpack_lines(text));
        assert_eq!(
            meatpack_lines(text),
            vec![
                "; header comment",
                "G28",
                "M104 S210",
                "G1X10.5Y20E0.123F1200",
                "G0F6000X1Y2",
                ";TYPE:WALL-OUTER",
                "G1X1.5Y2E1",
                "M486 Apart one",
                "T0",
            ]
        );
        // Exact bytes: switch packing and no-spaces on, then 'G' (0xD) in the low nibble
        // and '1' (0x1) in the high one; then 'M' (no code: 0xF, sent after) with '1';
        // then the newline left over paired with a newline (0xC, 0xC).
        let on = [0xFF, 0xFF, 251, 0xFF, 0xFF, 247];
        assert_eq!(meatpack("G1"), [&on[..], &[0x1D, 0xCC]].concat());
        assert_eq!(meatpack("M1"), [&on[..], &[0x1F, b'M', 0xCC]].concat());
        // A comment line goes out whole, with packing off around it.
        assert_eq!(
            meatpack("; a"),
            [&on[..], &[0xFF, 0xFF, 250], b"; a\n"].concat()
        );
        // Packed moves take about half the bytes.
        let moves = "G1 X10.512 Y20.250 E0.12345\n".repeat(100);
        assert!(meatpack(&moves).len() < moves.len() * 6 / 10);
    }

    /// One block as read back.
    struct Block {
        kind: u16,
        compression: u16,
        params: Vec<u8>,
        data: Vec<u8>,
        /// Size before compression.
        size: usize,
    }

    /// The blocks of a file, each checked against its CRC32.
    fn blocks(file: &[u8]) -> Vec<Block> {
        assert_eq!(&file[..4], b"GCDE");
        assert_eq!(u32::from_le_bytes(file[4..8].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(file[8..10].try_into().unwrap()), 1);
        let u16_at = |i: usize| u16::from_le_bytes(file[i..i + 2].try_into().unwrap());
        let u32_at = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().unwrap());
        let mut out = Vec::new();
        let mut i = 10;
        while i < file.len() {
            let start = i;
            let (kind, comp, size) = (u16_at(i), u16_at(i + 2), u32_at(i + 4) as usize);
            i += 8;
            let stored = if comp == 0 {
                size
            } else {
                i += 4;
                u32_at(i - 4) as usize
            };
            let params = if kind == THUMBNAIL { 6 } else { 2 };
            let p = file[i..i + params].to_vec();
            i += params;
            let data = file[i..i + stored].to_vec();
            i += stored;
            assert_eq!(crc32(&file[start..i]), u32_at(i), "block at {start}");
            i += 4;
            out.push(Block {
                kind,
                compression: comp,
                params: p,
                data,
                size,
            });
        }
        out
    }

    #[test]
    fn test_bgcode_file_round_trips_through_a_decoder() {
        use crate::geometry::Vec3;
        use crate::model::{Mesh, PlacedObject};
        let tris =
            PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 10.0))).world_triangles();
        let printer = crate::profiles::Printer::presets()
            .into_iter()
            .find(|p| p.name.contains("MK4S"))
            .unwrap();
        let sliced = crate::pipeline::slice_plate(
            &tris,
            &printer,
            &crate::profiles::Filament::presets()[0],
            &crate::profiles::PrintSettings::default(),
            &|_| {},
        );
        let text = &sliced.gcode.text;
        let file = encode(text, &sliced.gcode.estimate);
        assert!(
            file.len() < text.len() / 3,
            "{} vs {}",
            file.len(),
            text.len()
        );
        let bl = blocks(&file);
        let kinds: Vec<u16> = bl.iter().map(|b| b.kind).collect();
        // File, printer metadata, two thumbnails, print and slicer metadata, G-code.
        assert_eq!(&kinds[..6], &[0, 3, 5, 5, 4, 2]);
        assert!(kinds[6..].iter().all(|k| *k == GCODE) && kinds.len() > 6);
        // Metadata: uncompressed INI.
        let ini_of = |k: u16| {
            let b = bl.iter().find(|b| b.kind == k).unwrap();
            assert_eq!((b.compression, b.params.as_slice()), (0, &[0u8, 0][..]));
            String::from_utf8(b.data.clone()).unwrap()
        };
        let printer_meta = ini_of(PRINTER_METADATA);
        assert!(
            printer_meta.contains("printer_model=Prusa MK4S\n"),
            "{printer_meta}"
        );
        assert!(
            printer_meta.contains("nozzle_diameter=0.40\n"),
            "{printer_meta}"
        );
        assert!(ini_of(PRINT_METADATA).contains("estimated printing time (normal mode)="));
        assert!(ini_of(FILE_METADATA).starts_with("Producer=PrintCraft "));
        assert!(ini_of(SLICER_METADATA).contains("layer_height=0.20\n"));
        // Thumbnails: PNG, the sizes the plain file had.
        let thumbs: Vec<&Block> = bl.iter().filter(|b| b.kind == THUMBNAIL).collect();
        assert_eq!(thumbs[0].params, [0, 0, 44, 1, 44, 1]);
        assert_eq!(thumbs[1].params, [0, 0, 96, 0, 96, 0]);
        assert!(thumbs.iter().all(|t| t.data.starts_with(b"\x89PNG")));
        // G-code: heatshrink 12/4 and MeatPack with comments; unpacked, the plain file's
        // lines (without the pictures), cleaned as MeatPack does.
        let mut unpacked = String::new();
        for b in bl.iter().filter(|b| b.kind == GCODE) {
            assert_eq!(
                (b.compression, b.params.as_slice()),
                (HEATSHRINK_12_4, &[2u8, 0][..])
            );
            let packed = unheatshrink(&b.data, b.size);
            assert_eq!(packed.len(), b.size);
            unpacked.push_str(&unmeatpack(&packed));
        }
        let got: Vec<&str> = unpacked.lines().filter(|l| !l.is_empty()).collect();
        let (body, _) = split_thumbnails(text);
        assert!(!body.contains("thumbnail"));
        assert_eq!(got, meatpack_lines(&body));
    }

    #[test]
    fn test_bgcode_line_chunks_keep_whole_lines() {
        let text = "aaaa\nbbbb\ncc\nverylongline\n";
        assert_eq!(
            line_chunks(text, 10),
            vec!["aaaa\nbbbb\n", "cc\n", "verylongline\n"]
        );
        assert_eq!(duration_text(3725.0), "1h 2m 5s");
        assert_eq!(duration_text(59.6), "1m 0s");
    }
}
