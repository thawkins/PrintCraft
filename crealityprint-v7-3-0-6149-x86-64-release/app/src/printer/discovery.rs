//! Finding printers on the local network: every address in this computer's /24 network is
//! asked (with a short timeout) whether it has a printer port open, then each hit is
//! identified. Only started when the person presses "Find printers".

use super::link::port_open;
use super::{creality, moonraker, prusalink, FoundPrinter};
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_millis(400);
const PARALLEL: usize = 64;

pub struct Scan {
    /// Addresses checked so far, out of 254.
    pub checked: Arc<AtomicU32>,
    pub rx: Receiver<Vec<FoundPrinter>>,
    pub network: String,
}

/// This computer's address on the local network. Connecting a UDP socket only picks a
/// route; nothing is sent.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

pub fn start() -> Result<Scan, String> {
    let ip = local_ipv4().ok_or("This computer doesn't seem to be on a network.")?;
    let [a, b, c, _] = ip.octets();
    let checked = Arc::new(AtomicU32::new(0));
    let (tx, rx) = mpsc::channel();
    let counter = checked.clone();
    std::thread::spawn(move || {
        let hosts: Vec<String> = (1..=254u8).map(|d| format!("{a}.{b}.{c}.{d}")).collect();
        let mut hits: Vec<String> = Vec::new();
        for chunk in hosts.chunks(PARALLEL) {
            let handles: Vec<_> = chunk
                .iter()
                .cloned()
                .map(|h| {
                    let counter = counter.clone();
                    std::thread::spawn(move || {
                        let open = port_open(&h, creality::WS_PORT, PROBE_TIMEOUT)
                            || port_open(&h, moonraker::PORT, PROBE_TIMEOUT)
                            || port_open(&h, prusalink::PORT, PROBE_TIMEOUT);
                        counter.fetch_add(1, Ordering::Relaxed);
                        open.then_some(h)
                    })
                })
                .collect();
            hits.extend(handles.into_iter().filter_map(|h| h.join().ok().flatten()));
        }
        let found = hits.iter().filter_map(|h| identify(h)).collect();
        let _ = tx.send(found);
    });
    Ok(Scan {
        checked,
        rx,
        network: format!("{a}.{b}.{c}.x"),
    })
}

/// Works out what kind of printer answers at `host`.
pub fn identify(host: &str) -> Option<FoundPrinter> {
    if port_open(host, creality::WS_PORT, PROBE_TIMEOUT) {
        if let Some(p) = creality::identify(host) {
            return Some(p);
        }
    }
    if port_open(host, moonraker::PORT, PROBE_TIMEOUT) {
        if let Some(p) = moonraker::identify(host) {
            return Some(p);
        }
    }
    // Many devices answer on port 80; only PrusaLink printers are kept.
    if port_open(host, prusalink::PORT, PROBE_TIMEOUT) {
        return prusalink::identify(host);
    }
    None
}
