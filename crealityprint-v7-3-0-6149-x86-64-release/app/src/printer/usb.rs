//! Printing over a USB cable: the G-code is sent one line at a time over the serial port,
//! the way Marlin and Prusa firmware expect from a host program.
//!
//! The line protocol: every line carries a number and a checksum
//! (`N12 G1 X10*85`), the printer answers `ok` when it has taken a line, asks for a line
//! again with `Resend: 12` when one arrives damaged, and reports temperatures as
//! `T:210.0 /210.0 B:60.0 /60.0`. Only one line is in flight at a time.
//!
//! Klipper printers such as the Ender-3 V3 KE can't be driven this way: their USB socket
//! leads to Klipper's own controller, not to a G-code host. Use the network instead.
//!
//! **Repetier-Firmware** (RepRap wiki "Repetier-Firmware" and the "Replies from the RepRap
//! machine" part of the G-code page) speaks the same numbered, checksummed ASCII lines; it
//! works out by itself whether a host uses ASCII or its binary protocol, so the ASCII
//! lines here suit it. The app asks every USB printer for its name with M115 and, when it
//! says Repetier:
//! * announces the job on the printer's screen with `M530 S1 L<layers>` and
//!   `M531 <name>`, reports progress with `M532 X<percent> L<layer>` on every 0.1 %
//!   change or new layer, and ends with `M530 S0`;
//! * asks for temperatures (M105) every few seconds while printing, instead of Marlin's
//!   M155 automatic reports;
//! * levels the plate with G32 when calibration is on (see calibration.rs).
//!
//! Replies understood from every firmware: `ok` (also `ok 12`), `Resend: 12`,
//! `Resend: N12`, `rs 12` and `rs:12` (send line 12 again), `wait` (the printer's buffer is
//! empty, so an `ok` was lost), `busy:` (still working), `start` (the board restarted, so
//! a running print is lost), and `!!` / `Error:` / `fatal:` (shown to the person; `fatal:`
//! ends the print, since the firmware has shut down).

use super::link::{ContactLog, Worker};
use super::serial_log::SerialLog;
use super::{
    bed_gcode, fan_gcode, firmware_name, jog_gcode, nozzle_gcode, Command, Firmware, JobState,
    PrinterStatus,
};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub const DEFAULT_BAUD: u32 = 115_200;
pub const BAUD_RATES: [u32; 5] = [115_200, 250_000, 230_400, 57_600, 500_000];
/// Ask for temperatures this often when nothing else is going on.
const IDLE_POLL: Duration = Duration::from_secs(3);
/// Opening the port restarts many printer boards; give them time to wake up.
const BOOT_WAIT: Duration = Duration::from_secs(2);

/// Serial ports that look like printers (USB serial adapters).
pub fn list_ports() -> Vec<String> {
    let mut out: Vec<String> = serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    // Old built-in serial ports (/dev/ttyS*) are almost never printers and clutter the list.
    out.retain(|p| !p.starts_with("/dev/ttyS"));
    out.sort();
    out
}

/// `N12 G1 X10*85`: the line with its number and XOR checksum.
pub fn numbered(n: u64, cmd: &str) -> String {
    let body = format!("N{n} {cmd}");
    let cs = body.bytes().fold(0u8, |a, b| a ^ b);
    format!("{body}*{cs}")
}

/// A print file reduced to the commands the printer needs: comments and blank lines
/// dropped. Each command keeps the number of the layer it belongs to.
pub fn prepare(gcode: &str) -> (Vec<String>, Vec<u32>, u32) {
    let mut cmds = Vec::new();
    let mut layers = Vec::new();
    let mut layer = 0u32;
    let mut total = 0u32;
    for line in gcode.lines() {
        if let Some(n) = line.strip_prefix(";LAYER:") {
            if let Ok(i) = n.trim().parse::<u32>() {
                layer = i + 1;
                total = total.max(layer);
            }
        }
        let cmd = line.split(';').next().unwrap_or("").trim();
        if !cmd.is_empty() {
            cmds.push(cmd.to_string());
            layers.push(layer);
        }
    }
    (cmds, layers, total)
}

/// Reads temperatures from a report such as `ok T:210.0 /210.0 B:60.0 /60.0 @:0`.
pub fn parse_temps(line: &str, s: &mut PrinterStatus) -> bool {
    let mut found = false;
    let words: Vec<&str> = line.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let (key, rest) = match w.split_once(':') {
            Some(kv) => kv,
            None => continue,
        };
        let now: Option<f32> = rest.parse().ok();
        let target: Option<f32> = words
            .get(i + 1)
            .and_then(|t| t.strip_prefix('/'))
            .and_then(|t| t.parse().ok());
        match key {
            "T" | "T0" => {
                if now.is_some() {
                    s.nozzle = now;
                    s.nozzle_target = target.or(s.nozzle_target);
                    found = true;
                }
            }
            "B" if now.is_some() => {
                s.bed = now;
                s.bed_target = target.or(s.bed_target);
                found = true;
            }
            _ => {}
        }
    }
    found
}

/// The sending side of the protocol, kept apart from the port so it can be tested.
#[derive(Default)]
pub struct Streamer {
    /// Numbered history, so lines can be sent again on request (index = line number).
    sent: Vec<String>,
    job: Vec<String>,
    job_layers: Vec<u32>,
    pub total_layers: u32,
    pub next: usize,
    pub waiting_ok: bool,
    pub paused: bool,
    pub printing: bool,
    resend_from: Option<usize>,
    /// Extra commands (unnumbered) to slip in between job lines, e.g. after Stop.
    urgent: Vec<String>,
    /// What the printer said it runs (M115); Marlin until it says otherwise.
    pub firmware: Firmware,
    /// Repetier: the progress (in 0.1 %) and layer last shown on the printer's screen.
    shown_progress: Option<(u32, u32)>,
    /// Repetier: `M530 S0` (print ended) has been queued for this job.
    end_announced: bool,
}

/// A job name the printer can show: no comment or checksum marks, one line, not too long.
fn display_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !matches!(c, ';' | '*' | '\n' | '\r'))
        .take(40)
        .collect();
    clean.trim().to_string()
}

impl Streamer {
    pub fn start(&mut self, gcode: &str, name: &str) {
        let (cmds, layers, total) = prepare(gcode);
        self.job = cmds;
        self.job_layers = layers;
        self.total_layers = total;
        self.next = 0;
        self.printing = true;
        self.paused = false;
        self.resend_from = None;
        self.sent = vec![String::new()];
        self.shown_progress = None;
        self.end_announced = false;
        // Start numbering again at 1.
        self.urgent.push("M110 N0".into());
        if self.firmware == Firmware::Repetier {
            // Repetier's print mode: its screen shows the job, its layers and progress.
            self.urgent.push(format!("M530 S1 L{total}"));
            let name = display_name(name);
            if !name.is_empty() {
                self.urgent.push(format!("M531 {name}"));
            }
        } else {
            // Ask the firmware to report temperatures by itself every 2 s.
            self.urgent.push("M155 S2".into());
        }
    }

    /// Repetier: queue `M532` when the progress has moved on by 0.1 % or a new layer began.
    fn report_progress(&mut self) {
        if self.firmware != Firmware::Repetier || self.job.is_empty() {
            return;
        }
        let tenths = (self.next * 1000 / self.job.len()) as u32;
        let now = (tenths, self.layer());
        if self.shown_progress != Some(now) {
            self.shown_progress = Some(now);
            self.urgent
                .push(format!("M532 X{}.{} L{}", tenths / 10, tenths % 10, now.1));
        }
    }

    /// Sends a command between job lines (temperatures, fan, speed, moves).
    pub fn send_now(&mut self, cmd: &str) {
        self.urgent.push(cmd.to_string());
    }

    pub fn stop(&mut self) {
        self.printing = false;
        self.paused = false;
        self.job.clear();
        // Forget the job's numbered history too: a "Resend:" arriving around Stop must
        // not replay motion lines after the print was stopped.
        self.resend_from = None;
        self.sent = vec![String::new()];
        if self.firmware == Firmware::Repetier {
            self.urgent.push("M530 S0".into());
            self.end_announced = true;
        }
        for c in ["M104 S0", "M140 S0", "M107", "M84"] {
            self.urgent.push(c.into());
        }
    }

    pub fn progress(&self) -> f32 {
        if self.job.is_empty() {
            0.0
        } else {
            self.next as f32 / self.job.len() as f32 * 100.0
        }
    }

    pub fn layer(&self) -> u32 {
        self.job_layers
            .get(self.next.saturating_sub(1))
            .copied()
            .unwrap_or(0)
    }

    pub fn finished(&self) -> bool {
        self.printing
            && self.next >= self.job.len()
            && !self.waiting_ok
            && self.resend_from.is_none()
            && self.urgent.is_empty()
            && (self.firmware != Firmware::Repetier || self.end_announced)
    }

    /// The next line to write, if it's time to write one.
    pub fn next_out(&mut self, idle_poll_due: bool) -> Option<String> {
        if self.waiting_ok {
            return None;
        }
        // Repetier: the last line has been taken; leave print mode.
        if self.printing
            && self.firmware == Firmware::Repetier
            && self.next >= self.job.len()
            && self.resend_from.is_none()
            && !self.end_announced
        {
            self.end_announced = true;
            self.urgent.push("M530 S0".into());
        }
        if let Some(n) = self.resend_from {
            if n < self.sent.len() {
                self.resend_from = (n + 1 < self.sent.len()).then_some(n + 1);
                self.waiting_ok = true;
                return Some(numbered(n as u64, &self.sent[n]));
            }
            self.resend_from = None;
        }
        if !self.urgent.is_empty() {
            self.waiting_ok = true;
            let cmd = self.urgent.remove(0);
            if cmd.starts_with("M110") {
                self.sent = vec![String::new()];
            }
            return Some(cmd);
        }
        if self.printing && !self.paused && self.next < self.job.len() {
            let cmd = self.job[self.next].clone();
            self.next += 1;
            let n = self.sent.len();
            self.sent.push(cmd.clone());
            self.waiting_ok = true;
            self.report_progress();
            return Some(numbered(n as u64, &cmd));
        }
        if idle_poll_due {
            self.waiting_ok = true;
            return Some("M105".into());
        }
        None
    }

    /// Handles one line from the printer.
    pub fn on_line(&mut self, line: &str) {
        let l = line.trim();
        let lower = l.to_ascii_lowercase();
        if let Some(n) = resend_line(&lower) {
            self.resend_from = Some(n);
        } else if lower.starts_with("ok") {
            self.waiting_ok = false;
        } else if lower == "wait" {
            // The printer's buffer is empty and it is waiting for us: an "ok" went
            // missing. Carry on (a lost line is asked for again by number).
            self.waiting_ok = false;
        }
    }
}

/// The line number in `resend: 12`, `resend: n12`, `resend:n:12`, `rs 12` or `rs:12`.
fn resend_line(lower: &str) -> Option<usize> {
    let rest = lower
        .strip_prefix("resend")
        .or_else(|| lower.strip_prefix("rs "))
        .or_else(|| lower.strip_prefix("rs:"))?;
    let rest = rest.trim_start_matches([':', ' ', 'n', ':']);
    rest.split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|d| d.parse().ok())
}

/// What a line from the printer means for the session, beyond the protocol.
#[derive(Debug, PartialEq)]
enum Notice {
    /// The firmware's name (M115).
    Firmware(String),
    /// The board restarted (`start`): a running print is lost.
    Restarted,
    /// A fatal error: the firmware has shut down.
    Fatal,
    /// An error worth showing.
    Error,
}

fn notice(line: &str) -> Option<Notice> {
    if let Some(name) = firmware_name(line) {
        return Some(Notice::Firmware(name));
    }
    let lower = line.trim().to_ascii_lowercase();
    if lower == "start" {
        Some(Notice::Restarted)
    } else if lower.starts_with("fatal:") {
        Some(Notice::Fatal)
    } else if lower.starts_with("error") || lower.starts_with("!!") {
        Some(Notice::Error)
    } else {
        None
    }
}

pub fn run(w: &Worker) {
    let baud = w.printer.baud.unwrap_or(DEFAULT_BAUD);
    let mut contact = ContactLog::default();
    while !w.stopped() {
        let problem = match serialport::new(&w.host, baud)
            .timeout(Duration::from_millis(50))
            .open()
        {
            Ok(port) => {
                let mut log = SerialLog::open_default(&w.host, baud);
                let where_ = log
                    .as_ref()
                    .map(|l| format!(" Recording what is said in {}.", l.path.display()))
                    .unwrap_or_default();
                w.update(|s| s.note(format!("Opened {} at {baud} baud.{where_}", w.host)));
                let problem = session(w, port, &mut contact, &mut log);
                if let (Some(log), Some(p)) = (&mut log, &problem) {
                    log.note(&format!("Connection ended: {p}"));
                }
                problem
            }
            Err(e) => Some(format!("could not open {}: {e}", w.host)),
        };
        // No problem means we were asked to stop: nothing to report.
        if let Some(problem) = problem {
            contact.failed(w, &problem);
            w.pause(3.0);
        }
    }
    w.update(|s| s.connected = false);
}

/// How often the job's progress is passed to the window while printing.
const PROGRESS_REPORT_EVERY: Duration = Duration::from_millis(250);

/// Streams to the printer until asked to stop (None) or the port fails (why).
fn session(
    w: &Worker,
    mut port: Box<dyn serialport::SerialPort>,
    contact: &mut ContactLog,
    log: &mut Option<SerialLog>,
) -> Option<String> {
    let opened = Instant::now();
    let mut st = Streamer::default();
    // Which firmware is this? (Repetier is driven a little differently.)
    st.send_now("M115");
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut last_poll = Instant::now();
    let mut last_heard = Instant::now();
    let mut started_at: Option<Instant> = None;
    let mut file_name: Option<String> = None;
    // Each update wakes the window, so progress is reported a few times a second rather
    // than after every line, and "connected and idle" only once.
    let mut last_progress_report = Instant::now() - PROGRESS_REPORT_EVERY;
    let mut idle_reported = false;
    loop {
        if w.stopped() {
            if st.printing {
                st.stop();
            }
            // Best effort: send the "heaters off" lines before letting go.
            for c in ["M104 S0", "M140 S0", "M84"] {
                if let Some(log) = log.as_mut() {
                    log.sent(c);
                }
                let _ = port.write_all(format!("{c}\n").as_bytes());
            }
            return None;
        }
        while let Ok(cmd) = w.rx.try_recv() {
            match cmd {
                Command::UploadAndPrint { name, gcode } => {
                    st.start(&gcode, &name);
                    started_at = Some(Instant::now());
                    file_name = Some(name.clone());
                    w.update(|s| {
                        s.note(format!(
                            "Printing {name} over USB. Keep this computer awake and the app open."
                        ));
                        s.status.state = JobState::Printing;
                        s.status.file = Some(name);
                        s.status.total_layers = Some(st.total_layers);
                    });
                }
                Command::Pause => {
                    st.paused = true;
                    w.update(|s| s.status.state = JobState::Paused);
                }
                Command::Resume => {
                    st.paused = false;
                    w.update(|s| s.status.state = JobState::Printing);
                }
                Command::Stop => {
                    st.stop();
                    w.update(|s| {
                        s.status.state = JobState::Stopped;
                        s.note("Print stopped; heaters turned off.");
                    });
                }
                Command::SetNozzle(t) => {
                    st.send_now(&nozzle_gcode(t));
                    w.update(|s| s.status.nozzle_target = Some(t));
                }
                Command::SetBed(t) => {
                    st.send_now(&bed_gcode(t));
                    w.update(|s| s.status.bed_target = Some(t));
                }
                Command::SetFan(p) => {
                    st.send_now(&fan_gcode(p));
                    w.update(|s| s.status.fan_percent = Some(p));
                }
                Command::SetSpeed(p) => {
                    st.send_now(&format!("M220 S{p}"));
                    w.update(|s| s.status.speed_percent = Some(p as f32));
                }
                Command::Jog { axis, mm } => {
                    for line in jog_gcode(axis, mm) {
                        st.send_now(&line);
                    }
                }
                Command::Home => st.send_now("G28"),
                // The app adds (or leaves out) its own levelling step in the file.
                Command::SetCalibration(on) => w.update(|s| s.calibrate = Some(on)),
                Command::CancelObject(_) => w.update(|s| {
                    s.note("This connection can't cancel one object; use the printer's screen.")
                }),
                Command::PrintFile(_) | Command::RefreshFiles | Command::RefreshHistory => {
                    w.update(|s| {
                        s.note("A USB printer has no file list; send a sliced plate instead.")
                    });
                }
            }
        }
        match port.read(&mut chunk) {
            Ok(n) if n > 0 => {
                buf.extend_from_slice(&chunk[..n]);
                last_heard = Instant::now();
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Some(e.to_string()),
        }
        let mut lines = Vec::new();
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if !line.is_empty() {
                if let Some(log) = log.as_mut() {
                    log.received(&line);
                }
                st.on_line(&line);
                lines.push(line);
            }
        }
        if !lines.is_empty() {
            // Connected once the firmware answers, not when the port opens.
            contact.reached(w);
            let mut ended: Option<String> = None;
            let notices: Vec<(String, Notice)> = lines
                .iter()
                .filter_map(|l| notice(l).map(|n| (l.clone(), n)))
                .collect();
            for (line, n) in &notices {
                match n {
                    Notice::Firmware(name) => st.firmware = Firmware::from_name(name),
                    Notice::Restarted => {
                        // Whatever line was in flight is gone with the restart.
                        st.waiting_ok = false;
                        if st.printing {
                            st.stop();
                            ended = Some("The printer restarted, so the print stopped.".into());
                        }
                    }
                    Notice::Fatal if st.printing => {
                        st.stop();
                        ended = Some(format!("The printer stopped with an error: {line}"));
                    }
                    _ => {}
                }
            }
            w.update(|s| {
                for line in &lines {
                    parse_temps(line, &mut s.status);
                }
                for (line, n) in &notices {
                    match n {
                        Notice::Firmware(name) => {
                            if s.status.firmware.as_deref() != Some(name) {
                                s.note(format!(
                                    "The printer runs {name}. Using the {}.",
                                    super::protocol_label(name)
                                ));
                            }
                            s.status.firmware = Some(name.clone());
                            if s.status.model.is_none() {
                                s.status.model = Some(name.clone());
                            }
                        }
                        Notice::Fatal | Notice::Error => s.note(format!("Printer says: {line}")),
                        Notice::Restarted => {}
                    }
                }
                if let Some(why) = &ended {
                    s.status.state = JobState::Stopped;
                    s.note(why.clone());
                }
            });
        }
        // Don't talk while the board is still starting up.
        if opened.elapsed() < BOOT_WAIT {
            continue;
        }
        // A lost "ok" would stall everything; after a long silence, carry on.
        if st.waiting_ok && last_heard.elapsed() > Duration::from_secs(60) {
            st.waiting_ok = false;
            last_heard = Instant::now();
        }
        let poll_due = !st.printing && last_poll.elapsed() > IDLE_POLL;
        // Repetier doesn't report temperatures by itself (no M155): ask while printing.
        if st.printing && st.firmware == Firmware::Repetier && last_poll.elapsed() > IDLE_POLL {
            last_poll = Instant::now();
            st.send_now("M105");
        }
        if let Some(out) = st.next_out(poll_due) {
            if poll_due {
                last_poll = Instant::now();
            }
            if let Some(log) = log.as_mut() {
                log.sent(&out);
            }
            if let Err(e) = port.write_all(format!("{out}\n").as_bytes()) {
                return Some(format!("sending failed: {e}"));
            }
        }
        if st.printing {
            let progress = st.progress();
            let layer = st.layer();
            let elapsed = started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let done = st.finished();
            if done {
                st.printing = false;
            } else if last_progress_report.elapsed() < PROGRESS_REPORT_EVERY {
                continue;
            }
            last_progress_report = Instant::now();
            w.update(|s| {
                s.status.progress = Some(progress);
                s.status.layer = Some(layer);
                s.status.elapsed_s = Some(elapsed);
                s.status.remaining_s = (progress > 0.5)
                    .then_some((elapsed as f32 * (100.0 - progress) / progress) as u64);
                if done {
                    s.status.state = JobState::Finished;
                    s.status.progress = Some(100.0);
                    s.note(format!(
                        "Finished {}.",
                        file_name.clone().unwrap_or_default()
                    ));
                }
            });
        } else if !st.waiting_ok && !idle_reported {
            idle_reported = true;
            w.update(|s| {
                if s.status.state == JobState::Unknown {
                    s.status.state = JobState::Idle;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_without_temperatures_change_nothing() {
        let mut s = PrinterStatus::default();
        assert!(!parse_temps("ok", &mut s));
        assert!(!parse_temps("echo:busy: processing", &mut s));
        assert!(!parse_temps("T:abc B:", &mut s));
        assert_eq!((s.nozzle, s.bed), (None, None));
        // A report without targets keeps the targets already known.
        s.nozzle_target = Some(215.0);
        assert!(parse_temps("T:200.5 B:59.0", &mut s));
        assert_eq!((s.nozzle, s.nozzle_target), (Some(200.5), Some(215.0)));
    }

    #[test]
    fn comments_and_blank_lines_are_not_sent() {
        let (cmds, layers, total) = prepare("; header\n\nG28 ; home\n;LAYER:0\nG1 X1\n   \n");
        assert_eq!(cmds, ["G28", "G1 X1"]);
        assert_eq!(layers, [0, 1]);
        assert_eq!(total, 1);
    }

    #[test]
    fn checksums_match_the_reprap_rule() {
        // XOR of every byte of "N1 G28" = 18 (checked by hand: N=78,1=49,space=32,G=71,2=50,8=56).
        assert_eq!(numbered(1, "G28"), "N1 G28*18");
    }

    #[test]
    fn prepares_commands_and_layers() {
        let (cmds, layers, total) =
            prepare("; header\nG28 ; home\n\n;LAYER:0\nG1 X1\n;LAYER:1\nG1 X2\n");
        assert_eq!(cmds, vec!["G28", "G1 X1", "G1 X2"]);
        assert_eq!(layers, vec![0, 1, 2]);
        assert_eq!(total, 2);
    }

    #[test]
    fn reads_temperature_reports() {
        let mut s = PrinterStatus::default();
        assert!(parse_temps(
            "ok T:209.6 /210.0 B:60.1 /60.0 @:64 B@:0",
            &mut s
        ));
        assert_eq!(
            (s.nozzle, s.nozzle_target, s.bed, s.bed_target),
            (Some(209.6), Some(210.0), Some(60.1), Some(60.0))
        );
        assert!(!parse_temps("echo:busy: processing", &mut s));
    }

    /// Plays a pretend printer: answers "ok" to everything and asks for line 3 again once.
    #[test]
    fn streams_a_job_with_one_resend() {
        let mut st = Streamer::default();
        st.start(";LAYER:0\nG1 X1\nG1 X2\n;LAYER:1\nG1 X3\nG1 X4\n", "job");
        let mut written = Vec::new();
        let mut asked_again = false;
        for _ in 0..50 {
            let Some(out) = st.next_out(false) else { break };
            written.push(out.clone());
            if out.starts_with("N3 ") && !asked_again {
                asked_again = true;
                st.on_line("Error:checksum mismatch, Last Line: 2");
                st.on_line("Resend: 3");
            }
            st.on_line("ok");
            if st.finished() {
                break;
            }
        }
        assert_eq!(written[0], "M110 N0");
        assert_eq!(written[1], "M155 S2");
        let job: Vec<&String> = written.iter().filter(|l| l.starts_with('N')).collect();
        assert_eq!(job.len(), 5, "{written:?}");
        assert!(job[2].starts_with("N3 G1 X3*") && job[3].starts_with("N3 G1 X3*"));
        assert!(job[4].starts_with("N4 G1 X4*"));
        assert!(st.finished());
        assert_eq!(st.progress(), 100.0);
        assert_eq!((st.layer(), st.total_layers), (2, 2));
    }

    #[test]
    fn pause_holds_and_stop_cools_down() {
        let mut st = Streamer::default();
        st.start("G1 X1\nG1 X2\n", "job");
        while st.next_out(false).is_some_and(|l| !l.starts_with('N')) {
            st.on_line("ok");
        }
        st.on_line("ok");
        st.paused = true;
        assert_eq!(st.next_out(false), None);
        st.stop();
        let mut after = Vec::new();
        while let Some(l) = st.next_out(false) {
            after.push(l);
            st.on_line("ok");
        }
        assert_eq!(after, vec!["M104 S0", "M140 S0", "M107", "M84"]);
    }

    #[test]
    fn a_resend_around_stop_does_not_replay_the_job() {
        let mut st = Streamer::default();
        st.start("G1 X1\nG1 X2\nG1 X3\n", "job");
        let mut job_lines = 0;
        while let Some(l) = st.next_out(false) {
            st.on_line("ok");
            job_lines += l.starts_with('N') as usize;
            if job_lines == 2 {
                break;
            }
        }
        st.on_line("Resend: 1");
        st.stop();
        st.on_line("Resend: 1");
        let mut after = Vec::new();
        while let Some(l) = st.next_out(false) {
            after.push(l);
            st.on_line("ok");
        }
        assert!(after.iter().all(|l| !l.contains("G1")), "{after:?}");
        assert_eq!(after, vec!["M104 S0", "M140 S0", "M107", "M84"]);
    }

    /// Runs a whole job against a pretend printer that answers "ok" to every line.
    fn run_job(st: &mut Streamer) -> Vec<String> {
        let mut written = Vec::new();
        for _ in 0..500 {
            let Some(out) = st.next_out(false) else { break };
            written.push(out);
            st.on_line("ok");
            if st.finished() {
                break;
            }
        }
        written
    }

    #[test]
    fn repetier_shows_the_job_and_its_progress_on_the_printer() {
        let mut st = Streamer {
            firmware: Firmware::Repetier,
            ..Streamer::default()
        };
        st.start(
            ";LAYER:0\nG1 X1\nG1 X2\n;LAYER:1\nG1 X3\nG1 X4\n",
            "cube; v2*.gcode",
        );
        let written = run_job(&mut st);
        assert!(st.finished());
        assert_eq!(
            &written[..3],
            ["M110 N0", "M530 S1 L2", "M531 cube v2.gcode"]
        );
        assert!(
            !written.iter().any(|l| l.starts_with("M155")),
            "{written:?}"
        );
        let progress: Vec<&String> = written.iter().filter(|l| l.starts_with("M532")).collect();
        assert_eq!(
            progress,
            [
                "M532 X25.0 L1",
                "M532 X50.0 L1",
                "M532 X75.0 L2",
                "M532 X100.0 L2"
            ]
        );
        // Print mode ends once, after the last job line.
        assert_eq!(written.last().unwrap(), "M530 S0");
        assert_eq!(written.iter().filter(|l| *l == "M530 S0").count(), 1);
        let job: Vec<&String> = written.iter().filter(|l| l.starts_with('N')).collect();
        assert_eq!(job.len(), 4);
    }

    #[test]
    fn stopping_a_repetier_print_leaves_print_mode() {
        let mut st = Streamer {
            firmware: Firmware::Repetier,
            ..Streamer::default()
        };
        st.start("G1 X1\nG1 X2\n", "a");
        st.stop();
        let mut after = Vec::new();
        while let Some(l) = st.next_out(false) {
            after.push(l);
            st.on_line("ok");
        }
        assert_eq!(
            after.iter().filter(|l| *l == "M530 S0").count(),
            1,
            "{after:?}"
        );
        assert!(after.ends_with(&["M104 S0", "M140 S0", "M107", "M84"].map(String::from)));
    }

    #[test]
    fn understands_every_resend_form() {
        for (reply, n) in [
            ("resend: 12", 12),
            ("resend: n12", 12),
            ("resend:n:12", 12),
            ("rs 12", 12),
            ("rs:12", 12),
            ("rs n12", 12),
        ] {
            assert_eq!(resend_line(reply), Some(n), "{reply}");
        }
        assert_eq!(resend_line("rsx"), None);
        assert_eq!(resend_line("ok"), None);
    }

    #[test]
    fn wait_means_the_ok_was_lost() {
        let mut st = Streamer::default();
        st.start("G1 X1\nG1 X2\n", "job");
        assert!(st.next_out(false).is_some());
        assert_eq!(st.next_out(false), None, "waits for ok");
        st.on_line("wait");
        assert!(st.next_out(false).is_some(), "carries on after `wait`");
        // `busy:` keeps it waiting.
        st.on_line("busy: processing");
        assert_eq!(st.next_out(false), None);
    }

    #[test]
    fn reads_firmware_names_and_other_notices() {
        let repetier = "FIRMWARE_NAME:Repetier_1.0.3 COMPILED:Jan 1 2020 FIRMWARE_URL:https://github.com/repetier/Repetier-Firmware/ PROTOCOL_VERSION:1.0 MACHINE_TYPE:Mendel EXTRUDER_COUNT:1 REPETIER_PROTOCOL:3";
        assert_eq!(
            notice(repetier),
            Some(Notice::Firmware("Repetier 1.0.3".into()))
        );
        assert_eq!(Firmware::from_name("Repetier 1.0.3"), Firmware::Repetier);
        let marlin = "FIRMWARE_NAME:Marlin 2.1.2.1 (Jun 12 2024) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0";
        assert_eq!(
            notice(marlin),
            Some(Notice::Firmware("Marlin 2.1.2.1 (Jun 12 2024)".into()))
        );
        assert_eq!(Firmware::from_name("Marlin 2.1.2.1"), Firmware::Marlin);
        assert_eq!(notice("start"), Some(Notice::Restarted));
        assert_eq!(notice("fatal: Heater decoupled"), Some(Notice::Fatal));
        assert_eq!(notice("Error:Wrong checksum"), Some(Notice::Error));
        assert_eq!(notice("!! Thermistor"), Some(Notice::Error));
        assert_eq!(notice("ok T:20 B:20"), None);
    }
}
