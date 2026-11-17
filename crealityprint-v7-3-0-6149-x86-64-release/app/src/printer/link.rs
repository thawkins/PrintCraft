//! A live connection to one printer, run on a background thread. The window reads a
//! snapshot each frame and sends commands through a channel.

use super::{creality, moonraker, prusalink, usb, Api, Command, FoundPrinter, PrinterStatus, Wake};
use std::collections::VecDeque;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LOG_LINES: usize = 40;

/// What the background thread shares with the window.
#[derive(Clone, Debug, Default)]
pub struct LinkState {
    pub connected: bool,
    pub api: Option<Api>,
    pub status: PrinterStatus,
    /// Something slow in progress, such as "Sending cube.gcode…".
    pub busy: Option<String>,
    pub log: VecDeque<String>,
    /// Files on the printer; None until asked for.
    pub files: Option<Vec<super::RemoteFile>>,
    /// The printer's own record of past prints; None until asked for.
    pub history: Option<Vec<super::HistoryEntry>>,
    /// The Enable Calibration choice last sent to the printer.
    pub calibrate: Option<bool>,
}

impl LinkState {
    pub fn note(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if self.log.back() != Some(&msg) {
            self.log.push_back(msg);
        }
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }
}

pub type Shared = Arc<Mutex<LinkState>>;

/// Everything a protocol worker needs.
pub struct Worker {
    pub host: String,
    /// Everything known about the printer (sign-in, USB speed).
    pub printer: FoundPrinter,
    pub shared: Shared,
    pub rx: Receiver<Command>,
    pub stop: Arc<AtomicBool>,
    pub wake: Wake,
}

/// Writes connection changes to the log the same way for every kind of printer:
/// "Connected." when the printer answers, "Lost contact: …" when a working connection
/// drops, and "Not connected: …" while it can't be reached or signed in to. Each problem is
/// written once, not again on every retry.
#[derive(Debug, Default)]
pub struct ContactLog {
    connected: bool,
    last_problem: Option<String>,
}

impl ContactLog {
    /// The printer answered. Marks the link connected.
    pub fn reached(&mut self, w: &Worker) {
        let note = self.reached_note();
        w.update(|s| {
            s.connected = true;
            if let Some(n) = note {
                s.note(n);
            }
        });
    }

    /// Contact failed or dropped because of `problem`. Marks the link disconnected.
    pub fn failed(&mut self, w: &Worker, problem: &str) {
        let note = self.failed_note(problem);
        w.update(|s| {
            s.connected = false;
            if let Some(n) = note {
                s.note(n);
            }
        });
    }

    fn reached_note(&mut self) -> Option<String> {
        self.last_problem = None;
        (!std::mem::replace(&mut self.connected, true)).then(|| "Connected.".to_string())
    }

    fn failed_note(&mut self, problem: &str) -> Option<String> {
        let was_connected = std::mem::replace(&mut self.connected, false);
        if !was_connected && self.last_problem.as_deref() == Some(problem) {
            return None;
        }
        self.last_problem = Some(problem.to_string());
        Some(if was_connected {
            format!("Lost contact: {problem}")
        } else {
            format!("Not connected: {problem}")
        })
    }
}

impl Worker {
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    pub fn update(&self, f: impl FnOnce(&mut LinkState)) {
        if let Ok(mut s) = self.shared.lock() {
            f(&mut s);
        }
        (self.wake)();
    }

    /// Uploads a file that the printer starts printing as soon as it has it, showing
    /// "Sending…" meanwhile and noting the outcome when it worked (the caller reports a
    /// failure the way it reports any refused command).
    pub fn sending(
        &self,
        name: &str,
        upload: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        self.update(|s| s.busy = Some(format!("Sending {name} to the printer…")));
        let sent = upload();
        self.update(|s| {
            s.busy = None;
            if sent.is_ok() {
                s.note(format!("Sent {name}. The print is starting."));
            }
        });
        sent
    }

    /// Waits up to `secs`, returning early when asked to stop.
    pub fn pause(&self, secs: f32) {
        let steps = (secs * 10.0) as u32;
        for _ in 0..steps {
            if self.stopped() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub struct PrinterLink {
    pub printer: FoundPrinter,
    shared: Shared,
    tx: Sender<Command>,
    stop: Arc<AtomicBool>,
}

impl PrinterLink {
    pub fn connect(printer: FoundPrinter, wake: Wake) -> Self {
        let shared: Shared = Arc::default();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            host: printer.host.clone(),
            printer: printer.clone(),
            shared: shared.clone(),
            rx,
            stop: stop.clone(),
            wake,
        };
        let known = printer.api;
        std::thread::spawn(move || {
            worker.update(|s| s.note(format!("Connecting to {}…", worker.host)));
            let api = loop {
                if worker.stopped() {
                    return;
                }
                if let Some(api) = known.or_else(|| detect_api(&worker.host)) {
                    break api;
                }
                worker.update(|s| s.note("No printer answered at that address. Trying again…"));
                worker.pause(5.0);
            };
            worker.update(|s| s.api = Some(api));
            match api {
                Api::CrealityLan => creality::run(&worker),
                Api::Moonraker => moonraker::run(&worker),
                Api::PrusaLink => prusalink::run(&worker),
                Api::Usb => usb::run(&worker),
            }
        });
        Self {
            printer,
            shared,
            tx,
            stop,
        }
    }

    /// A copy of everything known about the printer (log, files and history included).
    pub fn state(&self) -> LinkState {
        self.with_state(LinkState::clone)
    }

    /// Reads the state in place, without copying it: for small questions asked often,
    /// such as "connected?" from every frame.
    pub fn with_state<R>(&self, read: impl FnOnce(&LinkState) -> R) -> R {
        match self.shared.lock() {
            Ok(s) => read(&s),
            Err(_) => read(&LinkState::default()),
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

impl Drop for PrinterLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Connects to `host:port`, trying every address the name resolves to (a name such as
/// "localhost" can give IPv6 first while the printer only listens on IPv4).
pub fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, String> {
    let addrs = (host, port).to_socket_addrs().map_err(|e| e.to_string())?;
    let mut last = "no address".to_string();
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// True when something accepts a TCP connection on `port` within `timeout`.
pub fn port_open(host: &str, port: u16, timeout: Duration) -> bool {
    connect(host, port, timeout).is_ok()
}

/// Which protocol a printer speaks, judged by the ports it has open.
pub fn detect_api(host: &str) -> Option<Api> {
    let t = Duration::from_millis(800);
    if port_open(host, creality::WS_PORT, t) {
        Some(Api::CrealityLan)
    } else if port_open(host, moonraker::PORT, t) {
        Some(Api::Moonraker)
    } else if port_open(host, prusalink::PORT, t) && prusalink::identify(host).is_some() {
        Some(Api::PrusaLink)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_changes_are_logged_once_each() {
        let mut log = ContactLog::default();
        assert_eq!(
            log.failed_note("refused").as_deref(),
            Some("Not connected: refused")
        );
        assert_eq!(
            log.failed_note("refused"),
            None,
            "the same problem is not repeated"
        );
        assert_eq!(
            log.failed_note("timed out").as_deref(),
            Some("Not connected: timed out")
        );
        assert_eq!(log.reached_note().as_deref(), Some("Connected."));
        assert_eq!(log.reached_note(), None);
        assert_eq!(
            log.failed_note("timed out").as_deref(),
            Some("Lost contact: timed out")
        );
        assert_eq!(log.failed_note("timed out"), None);
        assert_eq!(log.reached_note().as_deref(), Some("Connected."));
    }

    #[test]
    fn connects_by_name_when_only_a_later_address_listens() {
        // IPv4 only; "localhost" commonly resolves to ::1 first.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(connect("localhost", port, Duration::from_secs(2)).is_ok());
        assert!(port_open("localhost", port, Duration::from_secs(2)));
    }
}
