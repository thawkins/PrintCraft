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

use super::link::Worker;
use super::{fan_gcode, jog_gcode, Command, JobState, PrinterStatus};
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
}

impl Streamer {
    pub fn start(&mut self, gcode: &str) {
        let (cmds, layers, total) = prepare(gcode);
        self.job = cmds;
        self.job_layers = layers;
        self.total_layers = total;
        self.next = 0;
        self.printing = true;
        self.paused = false;
        self.resend_from = None;
        self.sent = vec![String::new()];
        // Start numbering again at 1.
        self.urgent.push("M110 N0".into());
        // Ask the firmware to report temperatures by itself every 2 s.
        self.urgent.push("M155 S2".into());
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
    }

    /// The next line to write, if it's time to write one.
    pub fn next_out(&mut self, idle_poll_due: bool) -> Option<String> {
        if self.waiting_ok {
            return None;
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
        if let Some(n) = lower
            .strip_prefix("resend:")
            .or_else(|| lower.strip_prefix("rs "))
            .map(|r| r.trim().trim_start_matches('n'))
            .and_then(|r| r.split_whitespace().next())
            .and_then(|r| r.parse::<usize>().ok())
        {
            self.resend_from = Some(n);
        } else if lower.starts_with("ok") {
            self.waiting_ok = false;
        }
    }
}

pub fn run(w: &Worker) {
    let baud = w.printer.baud.unwrap_or(DEFAULT_BAUD);
    while !w.stopped() {
        match serialport::new(&w.host, baud)
            .timeout(Duration::from_millis(50))
            .open()
        {
            Ok(port) => {
                w.update(|s| s.note(format!("Opened {} at {baud} baud.", w.host)));
                session(w, port);
            }
            Err(e) => w.update(|s| s.note(format!("Could not open {}: {e}", w.host))),
        }
        w.update(|s| s.connected = false);
        if !w.stopped() {
            w.pause(3.0);
        }
    }
}

fn session(w: &Worker, mut port: Box<dyn serialport::SerialPort>) {
    let opened = Instant::now();
    let mut st = Streamer::default();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut last_poll = Instant::now();
    let mut last_heard = Instant::now();
    let mut started_at: Option<Instant> = None;
    let mut file_name: Option<String> = None;
    loop {
        if w.stopped() {
            if st.printing {
                st.stop();
            }
            // Best effort: send the "heaters off" lines before letting go.
            for c in ["M104 S0", "M140 S0", "M84"] {
                let _ = port.write_all(format!("{c}\n").as_bytes());
            }
            return;
        }
        while let Ok(cmd) = w.rx.try_recv() {
            match cmd {
                Command::UploadAndPrint { name, gcode } => {
                    st.start(&gcode);
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
                    st.send_now(&format!("M104 S{t:.0}"));
                    w.update(|s| s.status.nozzle_target = Some(t));
                }
                Command::SetBed(t) => {
                    st.send_now(&format!("M140 S{t:.0}"));
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
            Err(e) => {
                w.update(|s| s.note(format!("USB connection lost: {e}")));
                return;
            }
        }
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if line.is_empty() {
                continue;
            }
            st.on_line(&line);
            w.update(|s| {
                s.connected = true;
                parse_temps(&line, &mut s.status);
                if line.starts_with("Error") || line.starts_with("!!") {
                    s.note(format!("Printer says: {line}"));
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
        if let Some(out) = st.next_out(poll_due) {
            if poll_due {
                last_poll = Instant::now();
            }
            if port.write_all(format!("{out}\n").as_bytes()).is_err() {
                w.update(|s| s.note("USB connection lost while sending."));
                return;
            }
        }
        if st.printing {
            let progress = st.progress();
            let layer = st.layer();
            let elapsed = started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            let done = st.finished();
            if done {
                st.printing = false;
            }
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
        } else if !st.waiting_ok {
            w.update(|s| {
                s.connected = true;
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
        st.start(";LAYER:0\nG1 X1\nG1 X2\n;LAYER:1\nG1 X3\nG1 X4\n");
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
        st.start("G1 X1\nG1 X2\n");
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
        st.start("G1 X1\nG1 X2\nG1 X3\n");
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
}
