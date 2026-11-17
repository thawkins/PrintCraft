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

    pub fn state(&self) -> LinkState {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
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

/// True when something accepts a TCP connection on `port` within `timeout`.
pub fn port_open(host: &str, port: u16, timeout: Duration) -> bool {
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs
        .next()
        .is_some_and(|a| TcpStream::connect_timeout(&a, timeout).is_ok())
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
