//! Finding printers on the local network: every address in this computer's /24 network is
//! asked (with a short timeout) whether it has a printer port open, then each hit is
//! identified. Only started when the person presses "Find printers".
//!
//! `relocate` finds a saved printer again when its address has changed: connecting to a
//! known printer (or opening a project with one) first checks the saved address, and if
//! the printer isn't there, asks the network for the printer of the same kind and name.

use super::link::port_open;
use super::{creality, moonraker, prusalink, Api, FoundPrinter};
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

/// Every address of this computer's /24 network (empty when it isn't on a network).
pub fn local_hosts() -> Vec<String> {
    let Some(ip) = local_ipv4() else {
        return Vec::new();
    };
    let [a, b, c, _] = ip.octets();
    (1..=254u8).map(|d| format!("{a}.{b}.{c}.{d}")).collect()
}

/// The addresses among `hosts` with any of `ports` open, asked `PARALLEL` at a time.
/// `counter` goes up by one per address checked.
fn hosts_with_open_port(hosts: &[String], ports: &[u16], counter: &Arc<AtomicU32>) -> Vec<String> {
    let mut hits: Vec<String> = Vec::new();
    for chunk in hosts.chunks(PARALLEL) {
        let handles: Vec<_> = chunk
            .iter()
            .cloned()
            .map(|h| {
                let counter = counter.clone();
                let ports = ports.to_vec();
                std::thread::spawn(move || {
                    let open = ports.iter().any(|p| port_open(&h, *p, PROBE_TIMEOUT));
                    counter.fetch_add(1, Ordering::Relaxed);
                    open.then_some(h)
                })
            })
            .collect();
        hits.extend(handles.into_iter().filter_map(|h| h.join().ok().flatten()));
    }
    hits
}

pub fn start() -> Result<Scan, String> {
    let ip = local_ipv4().ok_or("This computer doesn't seem to be on a network.")?;
    let [a, b, c, _] = ip.octets();
    let checked = Arc::new(AtomicU32::new(0));
    let (tx, rx) = mpsc::channel();
    let counter = checked.clone();
    std::thread::spawn(move || {
        let hosts = local_hosts();
        let ports = [creality::WS_PORT, moonraker::PORT, prusalink::PORT];
        let hits = hosts_with_open_port(&hosts, &ports, &counter);
        let found = hits.iter().filter_map(|h| identify(h)).collect();
        let _ = tx.send(found);
    });
    Ok(Scan {
        checked,
        rx,
        network: format!("{a}.{b}.{c}.x"),
    })
}

/// The port a kind of network printer answers on.
fn api_port(api: Api) -> Option<u16> {
    match api {
        Api::CrealityLan => Some(creality::WS_PORT),
        Api::Moonraker => Some(moonraker::PORT),
        Api::PrusaLink => Some(prusalink::PORT),
        Api::Usb => None,
    }
}

fn identify_as(api: Api, host: &str) -> Option<FoundPrinter> {
    match api {
        Api::CrealityLan => creality::identify(host),
        Api::Moonraker => moonraker::identify(host),
        Api::PrusaLink => prusalink::identify(host),
        Api::Usb => None,
    }
}

/// A name the printer gave itself (its hostname), not one made up from its address.
fn has_own_name(p: &FoundPrinter) -> bool {
    let n = p.name.trim();
    !n.is_empty() && !n.starts_with("Printer at ") && n != p.host
}

/// How long the saved address is given to answer.
const SAVED_ADDRESS_TIMEOUT: Duration = Duration::from_millis(1500);

/// Finds a saved network printer, whose address may have changed since (a router hands out
/// addresses anew, e.g. after a restart). If its saved address answers on its kind's port
/// and the printer there has the same name (or gives none), that is it. Otherwise the
/// addresses in `search` (normally the whole local network, `local_hosts`) are asked, and
/// the printer of the same kind and name is returned with its new address. None when it
/// can't be found, or can't be recognised (no name of its own, or a USB printer).
pub fn relocate(p: &FoundPrinter, search: &[String]) -> Option<FoundPrinter> {
    let api = p.api?;
    let port = api_port(api)?;
    if port_open(&p.host, port, SAVED_ADDRESS_TIMEOUT) {
        if !has_own_name(p) {
            return Some(p.clone());
        }
        match identify_as(api, &p.host) {
            // Another printer now has this address: look further.
            Some(there) if has_own_name(&there) && !there.name.eq_ignore_ascii_case(&p.name) => {}
            // The same printer, or one that didn't say its name: keep the address.
            _ => return Some(p.clone()),
        }
    }
    if !has_own_name(p) {
        return None;
    }
    let others: Vec<String> = search.iter().filter(|h| **h != p.host).cloned().collect();
    let open = hosts_with_open_port(&others, &[port], &Arc::new(AtomicU32::new(0)));
    open.iter()
        .find_map(|h| identify_as(api, h).filter(|f| f.name.eq_ignore_ascii_case(&p.name)))
        .map(|f| FoundPrinter {
            host: f.host,
            ..p.clone()
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
