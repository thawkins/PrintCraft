//! HTTP Digest sign-in (RFC 7616 with MD5), which PrusaLink uses for its password.

use md5::{Digest as _, Md5};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    /// Set when the server asks for qop=auth.
    pub qop_auth: bool,
}

fn hex_md5(s: &str) -> String {
    let d = Md5::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads a `WWW-Authenticate: Digest ...` header. None for other schemes.
pub fn parse_challenge(header: &str) -> Option<Challenge> {
    let rest = header.trim().strip_prefix("Digest")?.trim();
    let mut c = Challenge::default();
    let mut s = rest;
    while !s.is_empty() {
        let eq = s.find('=')?;
        let key = s[..eq]
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        let after = s[eq + 1..].trim_start();
        let (value, next) = if let Some(q) = after.strip_prefix('"') {
            let end = q.find('"')?;
            (&q[..end], &q[end + 1..])
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (after[..end].trim(), &after[end..])
        };
        match key.as_str() {
            "realm" => c.realm = value.to_string(),
            "nonce" => c.nonce = value.to_string(),
            "opaque" => c.opaque = Some(value.to_string()),
            "qop" => c.qop_auth = value.split(',').any(|q| q.trim() == "auth"),
            _ => {}
        }
        s = next.trim_start().trim_start_matches(',').trim_start();
    }
    (!c.nonce.is_empty()).then_some(c)
}

/// The `Authorization` header value for one request.
pub fn authorization(
    c: &Challenge,
    user: &str,
    pass: &str,
    method: &str,
    uri: &str,
    nc: u32,
    cnonce: &str,
) -> String {
    let ha1 = hex_md5(&format!("{user}:{}:{pass}", c.realm));
    let ha2 = hex_md5(&format!("{method}:{uri}"));
    let mut h = format!(
        "Digest username=\"{user}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", algorithm=MD5",
        c.realm, c.nonce
    );
    if c.qop_auth {
        let nc = format!("{nc:08x}");
        let response = hex_md5(&format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", c.nonce));
        h += &format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\", response=\"{response}\"");
    } else {
        let response = hex_md5(&format!("{ha1}:{}:{ha2}", c.nonce));
        h += &format!(", response=\"{response}\"");
    }
    if let Some(o) = &c.opaque {
        h += &format!(", opaque=\"{o}\"");
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_2617_example() {
        // The worked example from RFC 2617 section 3.5.
        let c = parse_challenge(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        assert!(c.qop_auth);
        let h = authorization(
            &c,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            1,
            "0a4f113b",
        );
        assert!(
            h.contains("response=\"6629fae49393a05397450978507c4ef1\""),
            "{h}"
        );
        assert!(h.contains("opaque=\"5ccc069c403ebaf9f0171e9517f40e41\""));
    }

    #[test]
    fn works_without_qop() {
        let c = parse_challenge(r#"Digest realm="Printer API", nonce="abc""#).unwrap();
        assert!(!c.qop_auth);
        let h = authorization(&c, "maker", "pw", "GET", "/api/v1/status", 1, "x");
        assert!(h.starts_with("Digest username=\"maker\", realm=\"Printer API\""));
        assert!(!h.contains("qop"));
    }
}
