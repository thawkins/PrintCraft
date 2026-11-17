//! Standard base64 (RFC 4648, with padding): G-code thumbnails and project meshes.

/// Standard base64 (with padding).
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64 back to bytes (whitespace ignored); None when it isn't base64.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if clean.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    let last = clean.len() / 4;
    for (index, chunk) in clean.chunks(4).enumerate() {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        // Padding is at most "==" and only ends the final block.
        if pad > 2 || (pad > 0 && index + 1 != last) {
            return None;
        }
        let mut n = 0u32;
        for (k, c) in chunk.iter().enumerate() {
            let d = if *c == b'=' { 0 } else { val(*c)? };
            if *c == b'=' && k < 4 - pad {
                return None;
            }
            n = n << 6 | d;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_value_round_trips() {
        let all: Vec<u8> = (0..=255).collect();
        for len in [0, 1, 2, 3, 254, 255, 256] {
            assert_eq!(base64_decode(&base64(&all[..len])).unwrap(), &all[..len]);
        }
    }

    #[test]
    fn line_breaks_in_the_text_are_ignored() {
        assert_eq!(base64_decode("Zm9v\nYmFy\r\n").unwrap(), b"foobar");
        assert_eq!(base64_decode("Zm9v YmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("Zm9v!mFy"), None);
    }
}
