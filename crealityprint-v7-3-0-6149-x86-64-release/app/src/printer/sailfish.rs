//! A USB connection to a printer that speaks the MakerBot S3G protocol: Sailfish and
//! MakerBot firmware (FlashForge Creator Pro and early Creator Pro 2, Replicator 1/2/2X
//! and their clones). The protocol is binary packets, one in flight at a time; see
//! `crate::x3g` and makerbot.md.
//!
//! On connecting it asks for the firmware version and variant, copies the EEPROM (the
//! machine's settings: steps per mm, axis lengths, nozzle count and offset, Ditto
//! printing) into `sailfish-eeprom.bin` beside the serial log, and from then on shows
//! temperatures and the build state every couple of seconds. The machine definition for
//! printing is the Creator Pro family's, with the EEPROM's values in their place.
//!
//! **Printing.** A sliced plate is translated to S3G (`x3g::translate`). The app first
//! tries to record it onto the SD card (14, the commands, 15) and print it from there
//! (16), so the computer can disconnect. Stock Sailfish is built without that recording
//! (`S3G_CAPTURE_2_SD` is off in every board's `Configuration.hh`) and answers
//! "unsupported", and then the commands are streamed over USB, one packet at a time,
//! with the printer's "buffer full" answer as the pace.
//!
//! **Controls.** Pause and resume use query 8. Stop uses 22 (halt and empty the queue),
//! never 3 or 7, which restart the board; then both nozzles and the bed are switched off.
//! Temperatures and the part fan are queued commands. Jogs (relative moves) and homing
//! are refused while a print runs. The SD card's files are listed with 18 and printed
//! with 16. The print speed can't be changed: S3G has no command for it.

use super::link::{ContactLog, Worker};
use super::serial_log::SerialLog;
use super::{Command, JobState, RemoteFile};
use crate::x3g::command::Action;
use crate::x3g::machine::Machine;
use crate::x3g::query::{
    self, BuildState, MachineSettings, Query, ToolQuery, EEPROM_CHUNK, EEPROM_SIZE,
};
use crate::x3g::{self, translate, Incoming, PacketReader, Reply};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Sailfish's usual speed.
pub const DEFAULT_BAUD: u32 = 115_200;
/// Times a question is sent before the printer counts as not answering.
const TRIES: u32 = 5;
pub const EEPROM_FILE_NAME: &str = "sailfish-eeprom.bin";

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Opening the port restarts many boards; wait before talking.
    pub boot_wait: Duration,
    /// How long to wait for each answer before asking again.
    pub reply_wait: Duration,
    /// How often temperatures and the build state are asked for.
    pub poll_every: Duration,
}

impl Timing {
    pub const REAL: Timing = Timing {
        boot_wait: Duration::from_secs(2),
        reply_wait: Duration::from_millis(500),
        poll_every: Duration::from_secs(2),
    };
}

#[derive(Debug, PartialEq)]
pub enum AskError {
    /// No (undamaged) answer after every try.
    NoAnswer,
    /// The printer answered with an error code.
    Refused(Reply),
    /// The port itself failed (unplugged).
    Port(String),
}

impl AskError {
    pub fn describe(&self) -> String {
        match self {
            AskError::NoAnswer => "the printer didn't answer".into(),
            AskError::Refused(r) => format!("the printer answered \"{}\"", r.describe()),
            AskError::Port(e) => e.clone(),
        }
    }
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe())
    }
}

/// One S3G conversation over a port: a question, then its answer.
pub struct Conn<P> {
    port: P,
    reader: PacketReader,
    pub log: Option<SerialLog>,
    reply_wait: Duration,
}

impl<P: Read + Write> Conn<P> {
    pub fn new(port: P, log: Option<SerialLog>, reply_wait: Duration) -> Self {
        Conn {
            port,
            reader: PacketReader::default(),
            log,
            reply_wait,
        }
    }

    fn note(&mut self, msg: &str) {
        if let Some(log) = self.log.as_mut() {
            log.note(msg);
        }
    }

    /// Asks one question and returns the answer after its reply code. A damaged,
    /// missing or "send again" answer is asked again, up to [`TRIES`] times.
    pub fn ask(&mut self, q: Query) -> Result<Vec<u8>, AskError> {
        self.exchange(&q.payload(), &q.describe(), true)
    }

    /// Sends one command for the printer's queue. A full queue comes back at once as
    /// `Refused(BufferFull)`, so the caller can do other things and send it again.
    ///
    /// When an answer is lost or damaged the command is sent again, as GPX and
    /// ReplicatorG do. If the printer had in fact taken the first copy, it runs twice: for
    /// X, Y and Z (absolute positions) that changes nothing; for the extruders (relative)
    /// it pushes one move's plastic twice. The serial log shows when this happens.
    pub fn send_action(&mut self, a: &Action) -> Result<(), AskError> {
        let what = action_words(a);
        self.exchange(&a.encode(), &what, false).map(|_| ())
    }

    fn exchange(
        &mut self,
        payload: &[u8],
        what: &str,
        wait_when_full: bool,
    ) -> Result<Vec<u8>, AskError> {
        let packet = x3g::frame(payload);
        let what = what.to_string();
        let mut last = AskError::NoAnswer;
        for _ in 0..TRIES {
            // Anything half-received belongs to an earlier, abandoned try.
            self.reader.clear();
            if let Some(log) = self.log.as_mut() {
                log.sent_packet(&packet, &what);
            }
            self.port
                .write_all(&packet)
                .and_then(|_| self.port.flush())
                .map_err(|e| AskError::Port(format!("sending failed: {e}")))?;
            match self.wait_for_packet()? {
                None => {
                    self.note(&format!("No answer to \"{what}\" yet; asking again."));
                    last = AskError::NoAnswer;
                }
                Some(Incoming::Damaged(p)) => {
                    if let Some(log) = self.log.as_mut() {
                        log.received_packet(&p, "damaged: CRC mismatch");
                    }
                    last = AskError::NoAnswer;
                }
                Some(Incoming::Packet(p)) => {
                    let Some(&code) = p.first() else {
                        last = AskError::NoAnswer;
                        continue;
                    };
                    let reply = Reply::from_code(code);
                    if let Some(log) = self.log.as_mut() {
                        log.received_packet(&x3g::frame(&p), &reply.describe());
                    }
                    match reply {
                        Reply::Ok => return Ok(p[1..].to_vec()),
                        Reply::BufferFull if !wait_when_full => {
                            return Err(AskError::Refused(Reply::BufferFull))
                        }
                        r if r.worth_resending() => {
                            last = AskError::Refused(r);
                            if r == Reply::BufferFull {
                                std::thread::sleep(Duration::from_millis(50));
                            }
                        }
                        r => return Err(AskError::Refused(r)),
                    }
                }
            }
        }
        Err(last)
    }

    /// The next packet, or None when the wait is over.
    fn wait_for_packet(&mut self) -> Result<Option<Incoming>, AskError> {
        let deadline = Instant::now() + self.reply_wait;
        let mut chunk = [0u8; 256];
        loop {
            if let Some(p) = self.reader.next_packet() {
                return Ok(Some(p));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            match self.port.read(&mut chunk) {
                Ok(n) => self.reader.push(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(e) => return Err(AskError::Port(e.to_string())),
            }
        }
    }

    /// Reads and drops whatever the board says for `how_long` (its start-up chatter).
    fn drain(&mut self, how_long: Duration) -> Result<(), String> {
        let until = Instant::now() + how_long;
        let mut chunk = [0u8; 256];
        let mut dropped = Vec::new();
        while Instant::now() < until {
            match self.port.read(&mut chunk) {
                Ok(n) => dropped.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if !dropped.is_empty() {
            self.note(&format!(
                "Ignored {} bytes the board sent while starting: {}",
                dropped.len(),
                x3g::hex(&dropped)
            ));
        }
        self.reader.clear();
        Ok(())
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
                let log = SerialLog::open_default(&w.host, baud);
                let where_ = log
                    .as_ref()
                    .map(|l| format!(" Recording what is said in {}.", l.path.display()))
                    .unwrap_or_default();
                w.update(|s| s.note(format!("Opened {} at {baud} baud.{where_}", w.host)));
                let mut conn = Conn::new(port, log, Timing::REAL.reply_wait);
                let problem = session(w, &mut conn, &mut contact, Timing::REAL);
                if let Some(p) = &problem {
                    conn.note(&format!("Connection ended: {p}"));
                }
                problem
            }
            Err(e) => Some(format!("could not open {}: {e}", w.host)),
        };
        if let Some(problem) = problem {
            contact.failed(w, &problem);
            w.pause(3.0);
        }
    }
    w.update(|s| s.connected = false);
}

/// The firmware's name for the badge and the log: "Sailfish 7.8".
fn firmware_name(version: Option<u16>, advanced: Option<query::AdvancedVersion>) -> String {
    match (advanced, version) {
        (Some(a), _) => format!(
            "{} {}",
            query::variant_name(a.variant),
            query::version_text(a.firmware)
        ),
        (None, Some(v)) if v > 0 => {
            format!("MakerBot-protocol firmware {}", query::version_text(v))
        }
        _ => "MakerBot-protocol firmware (version not reported)".into(),
    }
}

/// Talks to the printer until asked to stop (None) or the connection fails (why).
pub fn session<P: Read + Write>(
    w: &Worker,
    conn: &mut Conn<P>,
    contact: &mut ContactLog,
    timing: Timing,
) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < timing.boot_wait {
        if w.stopped() {
            return None;
        }
        if let Err(e) = conn.drain(Duration::from_millis(100)) {
            return Some(e);
        }
    }

    let version = match conn.ask(Query::Version) {
        Ok(a) => query::read_version(&a),
        Err(AskError::Port(e)) => return Some(e),
        Err(e) => {
            return Some(format!(
                "{} to the MakerBot protocol's version question. Check that this is a \
                 Sailfish or MakerBot printer and that the speed matches the printer's \
                 (Sailfish usually uses {DEFAULT_BAUD} baud).",
                e.describe()
            ))
        }
    };
    contact.reached(w);
    let advanced = match conn.ask(Query::AdvancedVersion) {
        Ok(a) => query::read_advanced_version(&a),
        Err(AskError::Port(e)) => return Some(e),
        Err(_) => None,
    };
    let name = firmware_name(version, advanced);
    conn.note(&format!("The printer runs {name}."));
    w.update(|s| {
        s.note(format!("The printer runs {name} (MakerBot protocol)."));
        s.status.firmware = Some(name.clone());
        s.status.model = Some(name.clone());
    });

    let settings = match read_settings(w, conn) {
        Ok(Some(s)) => s,
        Ok(None) => return None,
        Err(e) => return Some(e),
    };
    match conn.ask(Query::Position).map(|a| query::read_position(&a)) {
        Ok(Some(p)) => conn.note(&format!(
            "Position (steps): X {}, Y {}, Z {}, A {}, B {}.",
            p[0], p[1], p[2], p[3], p[4]
        )),
        Err(AskError::Port(e)) => return Some(e),
        _ => {}
    }

    let mut p = PrinterSide {
        machine: Machine::creator_pro().with_settings(&settings),
        tools: settings.tool_count.unwrap_or(1).clamp(1, 2),
        heated_bed: settings.heated_bed.unwrap_or(true),
        stream: None,
        building: false,
        paused: false,
        stopped_here: false,
        memory: PollMemory::default(),
    };
    let mut last_poll: Option<Instant> = None;
    loop {
        if w.stopped() {
            // A print sent from here stops with the connection; one running from the SD
            // card carries on by itself.
            if p.stream.is_some() {
                let _ = p.stop(w, conn);
            }
            return None;
        }
        while let Ok(cmd) = w.rx.try_recv() {
            if let Err(e) = p.command(w, conn, cmd) {
                return Some(e);
            }
        }
        if let Err(e) = p.stream_some(w, conn) {
            return Some(e);
        }
        if last_poll.is_none_or(|t| t.elapsed() >= timing.poll_every) {
            last_poll = Some(Instant::now());
            match poll(
                w,
                conn,
                p.tools,
                p.heated_bed,
                p.stopped_here,
                &mut p.memory,
            ) {
                Ok(building) => p.building = building || p.stream.is_some(),
                Err(e) => return Some(e),
            }
        }
        if p.stream.is_none() || p.paused {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A print being streamed: the commands, and how far it has got.
struct Stream {
    name: String,
    actions: Vec<Action>,
    next: usize,
    started: Instant,
    total_layers: u32,
    last_report: Instant,
}

/// What the session knows about the printer and the print it is running.
struct PrinterSide {
    machine: Machine,
    tools: u8,
    heated_bed: bool,
    stream: Option<Stream>,
    /// A build is running (streamed from here, or from the SD card).
    building: bool,
    /// Paused from here (query 8 toggles, so this tracks which way it last went).
    paused: bool,
    /// Stopped from here. Extended stop (22) leaves Sailfish's build state alone and the
    /// build end that follows marks it "finished normally" (`Host.cc`), so the app keeps
    /// showing Stopped until the next print starts.
    stopped_here: bool,
    memory: PollMemory,
}

/// How often streaming progress is passed to the window.
const PROGRESS_REPORT_EVERY: Duration = Duration::from_millis(250);
/// The most commands sent in one go before commands and polling get a turn.
const STREAM_BATCH: usize = 32;

impl PrinterSide {
    /// Carries out one of the window's commands. Err only when the port fails.
    fn command<P: Read + Write>(
        &mut self,
        w: &Worker,
        conn: &mut Conn<P>,
        cmd: Command,
    ) -> Result<(), String> {
        match cmd {
            Command::UploadAndPrint { name, gcode } => self.start_print(w, conn, &name, &gcode),
            Command::PrintFile(file) => {
                if self.building {
                    w.update(|s| s.note("A print is already running."));
                    return Ok(());
                }
                match port_ok(conn.ask(Query::PlayFile { name: file.clone() }))? {
                    Ok(a) if a.first() == Some(&0) => {
                        self.building = true;
                        self.stopped_here = false;
                        w.update(|s| {
                            s.note(format!("Printing {file} from the printer's SD card."));
                            s.status.state = JobState::Printing;
                            s.status.file = Some(file);
                        });
                    }
                    Ok(a) => {
                        let why = query::sd_problem(a.first().copied().unwrap_or(0xFF));
                        w.update(|s| s.note(format!("Couldn't print {file}: {why}.")));
                    }
                    Err(e) => w.update(|s| s.note(format!("Couldn't print {file}: {e}."))),
                }
                Ok(())
            }
            Command::RefreshFiles => self.list_files(w, conn),
            Command::RefreshHistory => Ok(()),
            Command::SetCalibration(on) => {
                w.update(|s| s.calibrate = Some(on));
                Ok(())
            }
            Command::Pause | Command::Resume => {
                let pause = matches!(cmd, Command::Pause);
                if !self.building || pause == self.paused {
                    return Ok(());
                }
                // Query 8 toggles: send it only to change to the state asked for.
                match port_ok(conn.ask(Query::PauseResume))? {
                    Ok(_) => {
                        self.paused = pause;
                        w.update(|s| {
                            s.status.state = if pause {
                                JobState::Paused
                            } else {
                                JobState::Printing
                            }
                        });
                    }
                    Err(e) => w.update(|s| s.note(format!("The printer refused: {e}."))),
                }
                Ok(())
            }
            Command::Stop => {
                if !self.building && self.stream.is_none() {
                    return Ok(());
                }
                self.stop(w, conn)?;
                w.update(|s| {
                    s.status.state = JobState::Stopped;
                    s.note("Print stopped; heaters turned off.");
                });
                Ok(())
            }
            Command::SetNozzle(t) => {
                let celsius = t.round() as i16;
                self.queue(w, conn, Action::SetNozzleTemp { tool: 0, celsius })?;
                w.update(|s| s.status.nozzle_target = Some(t));
                Ok(())
            }
            Command::SetBed(t) => {
                if !self.heated_bed {
                    w.update(|s| s.note("This printer has no heated bed."));
                    return Ok(());
                }
                let celsius = t.round() as i16;
                self.queue(w, conn, Action::SetBedTemp { celsius })?;
                w.update(|s| s.status.bed_target = Some(t));
                Ok(())
            }
            Command::SetFan(pct) => {
                self.queue(
                    w,
                    conn,
                    Action::Fan {
                        tool: 0,
                        on: pct > 0.0,
                    },
                )?;
                w.update(|s| s.status.fan_percent = Some(if pct > 0.0 { 100.0 } else { 0.0 }));
                Ok(())
            }
            Command::SetSpeed(_) => {
                w.update(|s| {
                    s.note("MakerBot-protocol printers can't change the print speed from the app.")
                });
                Ok(())
            }
            Command::Jog { axis, mm } => {
                if self.building {
                    w.update(|s| s.note("Moving the axes is off while a print runs."));
                    return Ok(());
                }
                let feed = if axis.eq_ignore_ascii_case(&'Z') {
                    super::JOG_FEED_Z
                } else {
                    super::JOG_FEED_XY
                } as f64;
                if let Some(a) = translate::jog(&self.machine, axis, mm as f64, feed) {
                    self.queue(w, conn, a)?;
                }
                Ok(())
            }
            Command::Home => {
                if self.building {
                    w.update(|s| s.note("Homing is off while a print runs."));
                    return Ok(());
                }
                for a in translate::home_actions(&self.machine) {
                    self.queue(w, conn, a)?;
                }
                Ok(())
            }
        }
    }

    /// Queues one command outside a print, waiting while the printer's queue is full.
    fn queue<P: Read + Write>(
        &mut self,
        w: &Worker,
        conn: &mut Conn<P>,
        a: Action,
    ) -> Result<(), String> {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            match conn.send_action(&a) {
                Ok(()) => return Ok(()),
                Err(AskError::Refused(Reply::BufferFull)) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(AskError::Port(e)) => return Err(e),
                Err(e) => {
                    w.update(|s| s.note(format!("The printer refused: {}.", e.describe())));
                    return Ok(());
                }
            }
        }
    }

    /// Translates a sliced plate and prints it: recorded onto the SD card and printed from
    /// there when the firmware can, otherwise streamed over USB.
    fn start_print<P: Read + Write>(
        &mut self,
        w: &Worker,
        conn: &mut Conn<P>,
        name: &str,
        gcode: &str,
    ) -> Result<(), String> {
        if self.building || self.stream.is_some() {
            w.update(|s| s.note("A print is already running."));
            return Ok(());
        }
        let translated = translate::Options::from_gcode(gcode, name)
            .and_then(|o| translate::translate(gcode, &self.machine, &o).map_err(|e| e.to_string()))
            .map(|t| {
                (
                    t,
                    translate::Options::from_gcode(gcode, name).map(|o| o.total_layers),
                )
            });
        let (t, layers) = match translated {
            Ok((t, layers)) => (t, layers.unwrap_or(0)),
            Err(e) => {
                w.update(|s| s.note(format!("Couldn't print {name}: {e}")));
                return Ok(());
            }
        };
        for note in &t.warnings {
            let note = note.clone();
            w.update(|s| s.note(note));
        }
        self.stopped_here = false;
        match self.upload(w, conn, name, &t.actions)? {
            Upload::Printing(file) => {
                self.building = true;
                self.paused = false;
                w.update(|s| {
                    s.note(format!(
                        "Sent {name} to the printer's SD card as {file}; it is printing from \
                         the card, so this computer may disconnect."
                    ));
                    s.status.state = JobState::Printing;
                    s.status.file = Some(name.to_string());
                    s.status.total_layers = Some(layers);
                });
            }
            Upload::NotPossible(why) => {
                self.stream = Some(Stream {
                    name: name.to_string(),
                    actions: t.actions,
                    next: 0,
                    started: Instant::now(),
                    total_layers: layers,
                    last_report: Instant::now() - PROGRESS_REPORT_EVERY,
                });
                self.building = true;
                self.paused = false;
                w.update(|s| {
                    s.note(format!(
                        "Printing {name} over USB ({why}). Keep this computer awake and the \
                         app open until it finishes."
                    ));
                    s.status.state = JobState::Printing;
                    s.status.file = Some(name.to_string());
                    s.status.total_layers = Some(layers);
                    s.status.progress = Some(0.0);
                });
            }
            Upload::Stopped => {}
        }
        Ok(())
    }

    /// Records the print onto the SD card (commands 14 and 15) and starts it (16). Only
    /// firmware built with `S3G_CAPTURE_2_SD` can; stock Sailfish answers "unsupported".
    fn upload<P: Read + Write>(
        &mut self,
        w: &Worker,
        conn: &mut Conn<P>,
        name: &str,
        actions: &[Action],
    ) -> Result<Upload, String> {
        let file = query::sd_file_name(name);
        match port_ok(conn.ask(Query::CaptureToFile { name: file.clone() }))? {
            Ok(a) if a.first() == Some(&0) => {}
            Ok(a) => {
                let why = query::sd_problem(a.first().copied().unwrap_or(0xFF));
                return Ok(Upload::NotPossible(format!("not via the SD card: {why}")));
            }
            Err(AskError::Refused(Reply::Unsupported)) => {
                return Ok(Upload::NotPossible(
                    "this firmware can't receive files over USB".into(),
                ))
            }
            Err(e) => {
                return Ok(Upload::NotPossible(format!(
                    "not via the SD card: {}",
                    e.describe()
                )))
            }
        }
        let total = actions.len().max(1);
        let mut last = Instant::now() - PROGRESS_REPORT_EVERY;
        for (i, a) in actions.iter().enumerate() {
            if w.stopped() {
                let _ = conn.ask(Query::EndCapture);
                return Ok(Upload::Stopped);
            }
            loop {
                match conn.send_action(a) {
                    Ok(()) => break,
                    Err(AskError::Refused(Reply::BufferFull)) => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(AskError::Port(e)) => return Err(e),
                    Err(e) => {
                        let _ = conn.ask(Query::EndCapture);
                        return Ok(Upload::NotPossible(format!(
                            "recording onto the SD card failed: {}",
                            e.describe()
                        )));
                    }
                }
            }
            if last.elapsed() >= PROGRESS_REPORT_EVERY {
                last = Instant::now();
                let pct = i * 100 / total;
                w.update(|s| {
                    s.busy = Some(format!("Sending {name} to the printer's SD card… {pct}%"))
                });
            }
        }
        w.update(|s| s.busy = None);
        let written = match port_ok(conn.ask(Query::EndCapture))? {
            Ok(a) => query::read_u32(&a, 0).unwrap_or(0),
            Err(e) => {
                return Ok(Upload::NotPossible(format!(
                    "the SD card recording didn't finish: {}",
                    e.describe()
                )))
            }
        };
        conn.note(&format!("Recorded {written} bytes into {file}."));
        match port_ok(conn.ask(Query::PlayFile { name: file.clone() }))? {
            Ok(a) if a.first() == Some(&0) => Ok(Upload::Printing(file)),
            Ok(a) => {
                let why = query::sd_problem(a.first().copied().unwrap_or(0xFF));
                w.update(|s| s.note(format!("{file} is on the SD card but didn't start: {why}.")));
                Ok(Upload::Stopped)
            }
            Err(e) => {
                w.update(|s| {
                    s.note(format!(
                        "{file} is on the SD card but didn't start: {}.",
                        e.describe()
                    ))
                });
                Ok(Upload::Stopped)
            }
        }
    }

    /// Sends the next commands of a streamed print, until the printer's queue is full,
    /// a batch has gone, or the print is done.
    fn stream_some<P: Read + Write>(
        &mut self,
        w: &Worker,
        conn: &mut Conn<P>,
    ) -> Result<(), String> {
        if self.paused {
            return Ok(());
        }
        let Some(st) = self.stream.as_mut() else {
            return Ok(());
        };
        for _ in 0..STREAM_BATCH {
            let Some(a) = st.actions.get(st.next) else {
                break;
            };
            match conn.send_action(a) {
                Ok(()) => st.next += 1,
                // The printer is busy; carry on after commands and polling.
                Err(AskError::Refused(Reply::BufferFull)) => {
                    std::thread::sleep(Duration::from_millis(10));
                    break;
                }
                Err(AskError::Port(e)) => return Err(e),
                Err(AskError::Refused(Reply::Cancelled)) => {
                    self.stream = None;
                    self.building = false;
                    w.update(|s| {
                        s.status.state = JobState::Stopped;
                        s.note("The print was cancelled on the printer.");
                    });
                    return Ok(());
                }
                Err(e) => {
                    let why = e.describe();
                    self.stop(w, conn)?;
                    w.update(|s| {
                        s.status.state = JobState::Failed;
                        s.note(format!("The print stopped: {why}. Heaters turned off."));
                    });
                    return Ok(());
                }
            }
        }
        let Some(st) = self.stream.as_mut() else {
            return Ok(());
        };
        let done = st.next >= st.actions.len();
        if done || st.last_report.elapsed() >= PROGRESS_REPORT_EVERY {
            st.last_report = Instant::now();
            let progress = 100.0 * st.next as f32 / st.actions.len().max(1) as f32;
            let layer = (progress / 100.0 * st.total_layers as f32).ceil() as u32;
            let elapsed = st.started.elapsed().as_secs();
            let name = st.name.clone();
            w.update(|s| {
                s.status.progress = Some(progress);
                s.status.layer = Some(layer.min(st.total_layers));
                s.status.elapsed_s = Some(elapsed);
                s.status.remaining_s = (progress > 0.5)
                    .then_some((elapsed as f32 * (100.0 - progress) / progress) as u64);
                if done {
                    s.note(format!(
                        "Sent all of {name}; the printer is finishing what it has queued."
                    ));
                }
            });
        }
        if done {
            // The printer's own build state (polled) says when it has really finished.
            self.stream = None;
        }
        Ok(())
    }

    /// Stops whatever is printing: the motors halt and the printer's queue is emptied
    /// (query 22), then both nozzles and the bed are switched off.
    fn stop<P: Read + Write>(&mut self, w: &Worker, conn: &mut Conn<P>) -> Result<(), String> {
        self.stream = None;
        self.building = false;
        self.paused = false;
        self.stopped_here = true;
        port_ok(conn.ask(Query::ExtendedStop))?.ok();
        let mut off = vec![];
        for tool in 0..self.tools {
            off.push(Action::SetNozzleTemp { tool, celsius: 0 });
        }
        if self.heated_bed {
            off.push(Action::SetBedTemp { celsius: 0 });
        }
        off.push(Action::BuildEnd);
        for a in off {
            self.queue(w, conn, a)?;
        }
        Ok(())
    }

    /// Reads the SD card's file list (query 18) into the window's file list.
    fn list_files<P: Read + Write>(
        &mut self,
        w: &Worker,
        conn: &mut Conn<P>,
    ) -> Result<(), String> {
        let mut files = Vec::new();
        let mut restart = true;
        // A card can't hold this many in its root folder; the limit just ends a bad loop.
        for _ in 0..512 {
            let answer = port_ok(conn.ask(Query::NextFileName { restart }))?;
            restart = false;
            match answer
                .map_err(|e| e.describe())
                .map(|a| query::read_next_file_name(&a))
            {
                Ok(Ok(Some(name))) => {
                    if name.to_ascii_lowercase().ends_with(".x3g")
                        || name.to_ascii_lowercase().ends_with(".s3g")
                    {
                        files.push(RemoteFile {
                            path: name.clone(),
                            name,
                            ..Default::default()
                        });
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(code)) => {
                    let why = query::sd_problem(code);
                    w.update(|s| s.note(format!("Couldn't read the SD card: {why}.")));
                    return Ok(());
                }
                Err(e) => {
                    w.update(|s| s.note(format!("Couldn't read the SD card: {e}.")));
                    return Ok(());
                }
            }
        }
        w.update(|s| s.files = Some(files));
        Ok(())
    }
}

/// How an upload to the SD card went.
enum Upload {
    /// On the card and printing, under this file name.
    Printing(String),
    /// The card can't take it; print over USB instead (why).
    NotPossible(String),
    /// Asked to stop, or the file wouldn't start: nothing more to do.
    Stopped,
}

/// Separates a failed port (the session's end) from the printer's own answer.
fn port_ok<T>(r: Result<T, AskError>) -> Result<Result<T, AskError>, String> {
    match r {
        Err(AskError::Port(e)) => Err(e),
        other => Ok(other),
    }
}

/// A few words for the serial log about a queued command.
fn action_words(a: &Action) -> String {
    match a {
        Action::Move { steps, .. } => format!(
            "move X {} Y {} Z {} A {} B {}",
            steps[0], steps[1], steps[2], steps[3], steps[4]
        ),
        Action::Home { to_max, axes, .. } => {
            format!(
                "home axes {axes:05b} to {}",
                if *to_max { "max" } else { "min" }
            )
        }
        Action::SetNozzleTemp { tool, celsius } => format!("tool {tool}: nozzle to {celsius} °C"),
        Action::SetBedTemp { celsius } => format!("bed to {celsius} °C"),
        Action::WaitForTool { tool, .. } => format!("wait for tool {tool} to heat"),
        Action::WaitForBed { .. } => "wait for the bed to heat".into(),
        Action::Fan { on, .. } => format!("part fan {}", if *on { "on" } else { "off" }),
        Action::BuildPercent { percent } => format!("progress {percent}%"),
        Action::BuildStart { name } => format!("build start: {name}"),
        Action::BuildEnd => "build end".into(),
        Action::RecallHome { .. } => "take the stored home position".into(),
        other => format!("{other:?}"),
    }
}

/// Copies the whole EEPROM, saves it beside the serial log, and reads the machine's
/// settings from it. Ok(None) when asked to stop meanwhile.
fn read_settings<P: Read + Write>(
    w: &Worker,
    conn: &mut Conn<P>,
) -> Result<Option<MachineSettings>, String> {
    w.update(|s| s.busy = Some("Reading the printer's settings…".into()));
    let mut eeprom = Vec::with_capacity(EEPROM_SIZE as usize);
    let mut offset = 0u16;
    let mut unreadable = None;
    while offset < EEPROM_SIZE {
        if w.stopped() {
            w.update(|s| s.busy = None);
            return Ok(None);
        }
        // Worked out in u16: the remaining size (up to 4096) doesn't fit a u8.
        let len = (EEPROM_SIZE - offset).min(EEPROM_CHUNK as u16) as u8;
        match conn.ask(Query::ReadEeprom { offset, len }) {
            Ok(bytes) if bytes.len() == len as usize => eeprom.extend_from_slice(&bytes),
            Ok(_) | Err(AskError::NoAnswer | AskError::Refused(_)) => {
                unreadable = Some(offset);
                break;
            }
            Err(AskError::Port(e)) => {
                w.update(|s| s.busy = None);
                return Err(e);
            }
        }
        offset += len as u16;
    }
    let saved = conn.log.as_ref().and_then(|log| {
        let path = log.path.with_file_name(EEPROM_FILE_NAME);
        std::fs::write(&path, &eeprom).ok().map(|_| path)
    });
    let settings = MachineSettings::from_eeprom(&eeprom);
    let mut lines = settings.summary();
    if let Some(at) = unreadable {
        lines.push(format!(
            "The settings could only be read up to 0x{at:04X}; anything after is unknown."
        ));
    }
    if let Some(path) = &saved {
        lines.push(format!("A copy of the settings is in {}.", path.display()));
    }
    for line in &lines {
        conn.note(line);
    }
    w.update(|s| {
        s.busy = None;
        for line in lines {
            s.note(line);
        }
    });
    Ok(Some(settings))
}

/// What polling remembers between rounds, so changes are logged once.
#[derive(Debug, Default)]
struct PollMemory {
    /// Faults already told to the person.
    warned: Vec<&'static str>,
    /// The motherboard status bits last seen.
    board: Option<u8>,
}

/// Asks for temperatures and the build state, and passes them to the window. Only a
/// failed port is an error; a question the firmware doesn't support leaves its value out.
fn poll<P: Read + Write>(
    w: &Worker,
    conn: &mut Conn<P>,
    tools: u8,
    heated_bed: bool,
    stopped_here: bool,
    memory: &mut PollMemory,
) -> Result<bool, String> {
    let mut ask_temp = |tool: u8, what: ToolQuery| -> Result<Option<f32>, String> {
        match conn.ask(Query::Tool { tool, what }) {
            Ok(a) => Ok(query::read_temperature(&a)),
            Err(AskError::Port(e)) => Err(e),
            Err(_) => Ok(None),
        }
    };
    let nozzle = ask_temp(0, ToolQuery::Temperature)?;
    let nozzle_target = ask_temp(0, ToolQuery::Target)?;
    let (nozzle2, nozzle2_target) = if tools > 1 {
        (
            ask_temp(1, ToolQuery::Temperature)?,
            ask_temp(1, ToolQuery::Target)?,
        )
    } else {
        (None, None)
    };
    let (bed, bed_target) = if heated_bed {
        (
            ask_temp(0, ToolQuery::BedTemperature)?,
            ask_temp(0, ToolQuery::BedTarget)?,
        )
    } else {
        (None, None)
    };
    let board = match conn.ask(Query::BoardStatus) {
        Ok(a) => a.first().copied(),
        Err(AskError::Port(e)) => return Err(e),
        Err(_) => None,
    };
    let stats = match conn.ask(Query::BuildStatistics) {
        Ok(a) => query::read_build_statistics(&a),
        Err(AskError::Port(e)) => return Err(e),
        Err(_) => None,
    };
    let building = stats.is_some_and(|s| {
        matches!(
            s.state,
            BuildState::Running | BuildState::Paused | BuildState::Sleeping
        )
    });
    let build_name = if building {
        match conn.ask(Query::BuildName) {
            Ok(a) => Some(query::read_text(&a)).filter(|n| !n.is_empty()),
            Err(AskError::Port(e)) => return Err(e),
            Err(_) => None,
        }
    } else {
        None
    };
    if board.is_some() && board != memory.board {
        memory.board = board;
        let words = query::board_status_words(board.unwrap_or(0));
        conn.note(&format!(
            "Motherboard status: {}.",
            if words.is_empty() {
                "nothing to report".to_string()
            } else {
                words.join(", ")
            }
        ));
    }
    // Faults are told once each, not on every poll.
    let mut faults = Vec::new();
    for (bit, words) in [
        (0x40u8, "The printer shut its heaters down (heater fault)."),
        (0x80u8, "The printer reports a power error."),
    ] {
        if board.is_some_and(|b| b & bit != 0) && !memory.warned.contains(&words) {
            memory.warned.push(words);
            faults.push(words);
        }
    }
    w.update(|s| {
        let st = &mut s.status;
        st.nozzle = nozzle;
        st.nozzle_target = nozzle_target;
        st.nozzle2 = nozzle2;
        st.nozzle2_target = nozzle2_target;
        st.bed = bed;
        st.bed_target = bed_target;
        if let Some(stats) = stats {
            st.state = match stats.state {
                BuildState::None | BuildState::Finished if stopped_here => JobState::Stopped,
                BuildState::None => JobState::Idle,
                BuildState::Running => JobState::Printing,
                BuildState::Finished => JobState::Finished,
                BuildState::Paused | BuildState::Sleeping => JobState::Paused,
                BuildState::Cancelled => JobState::Stopped,
            };
            st.elapsed_s =
                building.then_some(stats.hours as u64 * 3600 + stats.minutes as u64 * 60);
        } else if st.state == JobState::Unknown {
            st.state = JobState::Idle;
        }
        if building {
            st.file = build_name.clone();
        }
        for f in &faults {
            s.note(*f);
        }
    });
    Ok(building)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::link::{LinkState, Shared};
    use crate::printer::FoundPrinter;
    use crate::x3g::frame;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    /// A pretend Sailfish printer: answers questions the way Sailfish's `Host.cc` does,
    /// queues commands, and keeps a build state from the build start/end commands.
    struct FakeSailfish {
        reader: PacketReader,
        out: VecDeque<u8>,
        eeprom: Vec<u8>,
        /// Every packet received (payload), in order.
        heard: Arc<Mutex<Vec<Vec<u8>>>>,
        /// Damage the CRC of this many answers first.
        damage: u32,
        /// Never answer.
        silent: bool,
        /// Answer "buffer full" to every this-many-th command (0: never).
        full_every: usize,
        actions_seen: usize,
        /// Built with S3G_CAPTURE_2_SD (recording onto the SD card).
        can_record: bool,
        recording: bool,
        recorded: u32,
        /// 0 no build, 1 running, 2 finished, 3 paused.
        build: u8,
        files: Vec<&'static str>,
        file_index: usize,
        /// Targets set by commands: tool 0, tool 1, bed.
        targets: [i16; 3],
    }

    impl FakeSailfish {
        fn new(heard: Arc<Mutex<Vec<Vec<u8>>>>) -> Self {
            let mut eeprom = vec![0xFF; EEPROM_SIZE as usize];
            eeprom[0x22..0x22 + 12].copy_from_slice(b"Creator Pro\0");
            eeprom[0x42] = 2;
            eeprom[0x4C] = 1;
            eeprom[0x01A4..0x01A8].copy_from_slice(&94_117_647u32.to_le_bytes());
            eeprom[0x0FFF] = 1;
            FakeSailfish {
                reader: PacketReader::default(),
                out: VecDeque::new(),
                eeprom,
                heard,
                damage: 0,
                silent: false,
                full_every: 0,
                actions_seen: 0,
                can_record: false,
                recording: false,
                recorded: 0,
                build: 0,
                files: vec!["CUBE.X3G", "NOTES.TXT", "GEAR.X3G"],
                file_index: 0,
                targets: [210, 0, 60],
            }
        }

        fn answer(&mut self, p: &[u8]) -> Vec<u8> {
            let temp = |c: i16| {
                let b = c.to_le_bytes();
                vec![0x81, b[0], b[1]]
            };
            if p[0] >= 128 {
                self.actions_seen += 1;
                if self.full_every > 0 && self.actions_seen.is_multiple_of(self.full_every) {
                    return vec![0x82];
                }
                if self.recording {
                    self.recorded += p.len() as u32;
                    return vec![0x81];
                }
                match p[0] {
                    153 => self.build = 1,
                    154 => self.build = 2,
                    136 if p[2] == 3 => {
                        self.targets[p[1].min(1) as usize] = i16::from_le_bytes([p[4], p[5]])
                    }
                    136 if p[2] == 31 => self.targets[2] = i16::from_le_bytes([p[4], p[5]]),
                    _ => {}
                }
                return vec![0x81];
            }
            match p[0] {
                0 => vec![0x81, 0xC4, 0x02],
                27 => vec![0x81, 0xC4, 0x02, 0, 0, 0x80, 0, 0, 0],
                10 => match (p[1], p[2]) {
                    (0, 2) => temp(205),
                    (0, 32) => temp(self.targets[0]),
                    (1, 2) => temp(25),
                    (1, 32) => temp(self.targets[1]),
                    (_, 30) => temp(58),
                    (_, 33) => temp(self.targets[2]),
                    _ => vec![0x85],
                },
                12 => {
                    let at = u16::from_le_bytes([p[1], p[2]]) as usize;
                    let mut a = vec![0x81];
                    a.extend_from_slice(&self.eeprom[at..at + p[3] as usize]);
                    a
                }
                21 => {
                    let mut a = vec![0x81];
                    for v in [1i32, 2, 3, 4, 5] {
                        a.extend(v.to_le_bytes());
                    }
                    a.extend([0, 0]);
                    a
                }
                23 => vec![0x81, 0],
                24 => vec![0x81, self.build, 0, 42, 0, 0, 0, 0, 0, 0, 0, 0],
                20 => {
                    let mut a = vec![0x81];
                    a.extend(b"cube\0");
                    a
                }
                8 => {
                    self.build = if self.build == 3 { 1 } else { 3 };
                    vec![0x81]
                }
                22 => {
                    self.build = 0;
                    vec![0x81, 0]
                }
                14 if !self.can_record => vec![0x85],
                14 => {
                    self.recording = true;
                    vec![0x81, 0]
                }
                15 => {
                    self.recording = false;
                    let mut a = vec![0x81];
                    a.extend(self.recorded.to_le_bytes());
                    a
                }
                16 => {
                    self.build = 1;
                    vec![0x81, 0]
                }
                18 => {
                    if p[1] == 1 {
                        self.file_index = 0;
                    }
                    let mut a = vec![0x81, 0];
                    if let Some(f) = self.files.get(self.file_index) {
                        a.extend(f.as_bytes());
                        self.file_index += 1;
                    }
                    a.push(0);
                    a
                }
                _ => vec![0x85],
            }
        }
    }

    impl Write for FakeSailfish {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.reader.push(buf);
            while let Some(Incoming::Packet(p)) = self.reader.next_packet() {
                self.heard.lock().unwrap().push(p.clone());
                if self.silent {
                    continue;
                }
                let mut reply = frame(&self.answer(&p));
                if self.damage > 0 {
                    self.damage -= 1;
                    let last = reply.len() - 1;
                    reply[last] ^= 0x55;
                }
                self.out.extend(reply);
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Read for FakeSailfish {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.out.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let n = buf.len().min(self.out.len());
            for b in buf.iter_mut().take(n) {
                *b = self.out.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    const FAST: Timing = Timing {
        boot_wait: Duration::ZERO,
        reply_wait: Duration::from_millis(100),
        poll_every: Duration::from_millis(10),
    };

    type Heard = Arc<Mutex<Vec<Vec<u8>>>>;

    fn worker() -> (Worker, Sender<Command>, Arc<AtomicBool>, Shared) {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let shared: Shared = Arc::new(Mutex::new(LinkState::default()));
        let w = Worker {
            host: "/dev/ttyACM9".into(),
            printer: FoundPrinter::default(),
            shared: shared.clone(),
            rx,
            stop: stop.clone(),
            wake: Arc::new(|| {}),
        };
        (w, tx, stop, shared)
    }

    /// Runs a session against a pretend printer on its own thread.
    fn start(
        setup: impl FnOnce(&mut FakeSailfish),
    ) -> (
        Sender<Command>,
        Arc<AtomicBool>,
        Shared,
        Heard,
        std::thread::JoinHandle<Option<String>>,
    ) {
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let mut fake = FakeSailfish::new(heard.clone());
        setup(&mut fake);
        let (w, tx, stop, shared) = worker();
        let t = std::thread::spawn(move || {
            let mut conn = Conn::new(fake, None, FAST.reply_wait);
            session(&w, &mut conn, &mut ContactLog::default(), FAST)
        });
        (tx, stop, shared, heard, t)
    }

    fn wait_for(shared: &Shared, what: impl Fn(&LinkState) -> bool) -> LinkState {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let s = shared.lock().unwrap().clone();
            if what(&s) {
                return s;
            }
            assert!(Instant::now() < until, "timed out; state: {s:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn logged(s: &LinkState, text: &str) -> bool {
        s.log.iter().any(|l| l.contains(text))
    }

    /// A small sliced-looking job: the plate header, heating, homing and a few moves.
    fn job() -> &'static str {
        "; printer: FlashForge Creator Pro (Sailfish)\n; plate: 227.0 x 148.0 mm\n\
         ;LAYER_COUNT:2\nM140 S60\nM104 S210\nG90\nM82\nG28\nM190 S60\nM109 S210\n\
         G92 E0\n;LAYER:0\nG1 Z0.2 F600\nG1 X100 Y70 F3000\nG1 X120 Y70 E1.0 F1500\n\
         ;LAYER:1\nG1 Z0.4 F600\nG1 X100 Y70 E2.0 F1500\nM104 S0\nM140 S0\nM84\n"
    }

    fn actions_heard(heard: &Heard) -> Vec<Vec<u8>> {
        heard
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p[0] >= 128)
            .cloned()
            .collect()
    }

    #[test]
    fn reads_versions_settings_and_temperatures_on_connecting() {
        let (_tx, stop, shared, heard, t) = start(|_| {});
        let s = wait_for(&shared, |s| s.status.nozzle2.is_some());
        assert!(s.connected);
        assert_eq!(s.status.firmware.as_deref(), Some("Sailfish 7.8"));
        assert_eq!(s.status.nozzle, Some(205.0));
        assert_eq!(s.status.nozzle_target, Some(210.0));
        assert_eq!(s.status.nozzle2, Some(25.0));
        assert_eq!(s.status.bed, Some(58.0));
        assert_eq!(s.status.bed_target, Some(60.0));
        assert_eq!(s.status.state, JobState::Idle);
        assert!(logged(&s, "\"Creator Pro\": 2 nozzle(s)"), "{:?}", s.log);
        assert!(logged(&s, "Ditto printing on"));
        stop.store(true, Ordering::Relaxed);
        assert_eq!(t.join().unwrap(), None);
        let heard = heard.lock().unwrap();
        assert!(
            heard.iter().all(|p| p[0] < 128 && p[0] != 3 && p[0] != 7),
            "connecting must not change anything"
        );
        let eeprom_reads = heard.iter().filter(|p| p[0] == 12).count();
        assert_eq!(
            eeprom_reads,
            (EEPROM_SIZE as usize).div_ceil(EEPROM_CHUNK as usize),
            "the whole EEPROM is copied once"
        );
    }

    #[test]
    fn streams_a_print_over_usb_when_the_card_cannot_record() {
        // Stock Sailfish: no recording onto the SD card; and a busy printer that says
        // "buffer full" to every fifth command.
        let (tx, stop, shared, heard, t) = start(|f| f.full_every = 5);
        wait_for(&shared, |s| s.status.state == JobState::Idle);
        tx.send(Command::UploadAndPrint {
            name: "cube".into(),
            gcode: Arc::new(job().into()),
        })
        .unwrap();
        let s = wait_for(&shared, |s| s.status.state == JobState::Finished);
        assert!(logged(
            &s,
            "Printing cube over USB (this firmware can't receive files over USB)"
        ));
        assert_eq!(s.status.progress, Some(100.0));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        // Every command of the translated job arrived once, in order, despite the full
        // buffer answers (which are sent again).
        let o = translate::Options::from_gcode(job(), "cube").unwrap();
        let expected: Vec<Vec<u8>> = translate::translate(job(), &Machine::creator_pro(), &o)
            .unwrap()
            .actions
            .iter()
            .map(|a| a.encode())
            .collect();
        let mut got = actions_heard(&heard);
        got.dedup(); // a "buffer full" answer means the same command is sent again
        assert_eq!(got.len(), expected.len());
        // Steps per mm come from the EEPROM (X only there; the same value), so compare
        // the commands' kinds and order.
        let kinds = |v: &Vec<Vec<u8>>| v.iter().map(|p| p[0]).collect::<Vec<u8>>();
        assert_eq!(kinds(&got), kinds(&expected));
        assert!(
            heard.lock().unwrap().iter().any(|p| p[0] == 14),
            "recording was tried"
        );
        assert!(heard.lock().unwrap().iter().all(|p| p[0] != 3 && p[0] != 7));
    }

    #[test]
    fn records_onto_the_sd_card_and_prints_from_it_when_the_firmware_can() {
        let (tx, stop, shared, heard, t) = start(|f| f.can_record = true);
        wait_for(&shared, |s| s.status.state == JobState::Idle);
        tx.send(Command::UploadAndPrint {
            name: "XYZ cube".into(),
            gcode: Arc::new(job().into()),
        })
        .unwrap();
        let s = wait_for(&shared, |s| logged(s, "printing from the card"));
        assert!(logged(&s, "as XYZcube.x3g"), "{:?}", s.log);
        assert_eq!(s.status.state, JobState::Printing);
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let codes: Vec<u8> = heard.lock().unwrap().iter().map(|p| p[0]).collect();
        let start = codes.iter().position(|&c| c == 14).unwrap();
        let end = codes.iter().position(|&c| c == 15).unwrap();
        let play = codes.iter().position(|&c| c == 16).unwrap();
        assert!(
            start < end && end < play,
            "record, finish, then print: {codes:?}"
        );
        assert!(
            codes[start + 1..end].contains(&155),
            "the moves were recorded"
        );
        assert!(
            !codes[play..].contains(&22),
            "disconnecting leaves an SD print running"
        );
    }

    #[test]
    fn stop_halts_empties_the_queue_and_turns_the_heaters_off() {
        let (tx, stop, shared, heard, t) = start(|f| f.full_every = 2);
        wait_for(&shared, |s| s.status.state == JobState::Idle);
        let mut long = job().to_string();
        for i in 0..3000 {
            long.push_str(&format!(
                "G1 X{} Y70 E{}\n",
                100 + i % 20,
                3.0 + i as f64 * 0.01
            ));
        }
        tx.send(Command::UploadAndPrint {
            name: "long".into(),
            gcode: Arc::new(long),
        })
        .unwrap();
        wait_for(&shared, |s| s.status.progress.is_some_and(|p| p > 1.0));
        tx.send(Command::Pause).unwrap();
        wait_for(&shared, |s| s.status.state == JobState::Paused);
        tx.send(Command::Resume).unwrap();
        wait_for(&shared, |s| s.status.state == JobState::Printing);
        tx.send(Command::Stop).unwrap();
        let s = wait_for(&shared, |s| logged(s, "Print stopped; heaters turned off."));
        assert_eq!(s.status.state, JobState::Stopped);
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let heard = heard.lock().unwrap();
        let stop_at = heard
            .iter()
            .position(|p| p[0] == 22)
            .expect("extended stop sent");
        assert_eq!(heard[stop_at], vec![22, 0b11]);
        let after: Vec<&Vec<u8>> = heard[stop_at..].iter().filter(|p| p[0] >= 128).collect();
        assert!(after.contains(&&vec![136, 0, 3, 2, 0, 0]), "tool 0 off");
        assert!(after.contains(&&vec![136, 1, 3, 2, 0, 0]), "tool 1 off");
        assert!(after.contains(&&vec![136, 0, 31, 2, 0, 0]), "bed off");
        assert!(
            !after.iter().any(|p| p[0] == 155),
            "no move is sent after stopping"
        );
        assert_eq!(
            heard.iter().filter(|p| p[0] == 8).count(),
            2,
            "pause, then resume"
        );
        assert!(heard.iter().all(|p| p[0] != 3 && p[0] != 7));
    }

    #[test]
    fn controls_heat_fan_jog_and_home_but_not_during_a_print() {
        let (tx, stop, shared, heard, t) = start(|_| {});
        wait_for(&shared, |s| s.status.state == JobState::Idle);
        tx.send(Command::SetNozzle(200.0)).unwrap();
        tx.send(Command::SetBed(55.0)).unwrap();
        tx.send(Command::SetFan(40.0)).unwrap();
        tx.send(Command::Jog {
            axis: 'Z',
            mm: -1.0,
        })
        .unwrap();
        tx.send(Command::Home).unwrap();
        tx.send(Command::SetSpeed(120)).unwrap();
        let s = wait_for(&shared, |s| logged(s, "can't change the print speed"));
        assert_eq!(s.status.nozzle_target, Some(200.0));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let acts = actions_heard(&heard);
        assert_eq!(acts[0], vec![136, 0, 3, 2, 200, 0], "nozzle");
        assert_eq!(acts[1], vec![136, 0, 31, 2, 55, 0], "bed");
        assert_eq!(
            acts[2],
            vec![136, 0, 13, 1, 1],
            "part fan on (the extra output)"
        );
        assert_eq!(acts[3][0], 155, "jog");
        assert_eq!(i32::from_le_bytes(acts[3][9..13].try_into().unwrap()), -400);
        assert_eq!(
            acts[3][25],
            crate::x3g::command::ALL_AXES,
            "jog is relative"
        );
        assert_eq!(acts[4][0], 132, "home X/Y to max");
        assert_eq!(acts[5][0], 131, "home Z to min");
        assert_eq!(acts[6], vec![144, 0b111], "take the stored home");

        // While a print runs from the card, jogging and homing are refused.
        let (tx, stop, shared, heard, t) = start(|_| {});
        wait_for(&shared, |s| s.status.state == JobState::Idle);
        tx.send(Command::PrintFile("CUBE.X3G".into())).unwrap();
        wait_for(&shared, |s| s.status.state == JobState::Printing);
        tx.send(Command::Jog {
            axis: 'X',
            mm: 10.0,
        })
        .unwrap();
        tx.send(Command::Home).unwrap();
        wait_for(&shared, |s| logged(s, "Homing is off while a print runs."));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        assert!(actions_heard(&heard).is_empty(), "nothing moved");
        assert!(heard.lock().unwrap().contains(&b"\x10CUBE.X3G\0".to_vec()));
    }

    #[test]
    fn lists_the_print_files_on_the_sd_card() {
        let (tx, stop, shared, _heard, t) = start(|_| {});
        wait_for(&shared, |s| s.status.state == JobState::Idle);
        tx.send(Command::RefreshFiles).unwrap();
        let s = wait_for(&shared, |s| s.files.is_some());
        let names: Vec<String> = s.files.unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["CUBE.X3G", "GEAR.X3G"], "only print files");
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
    }

    #[test]
    fn a_print_without_a_plate_size_is_refused_with_a_reason() {
        let (tx, stop, shared, heard, t) = start(|_| {});
        wait_for(&shared, |s| s.status.state == JobState::Idle);
        tx.send(Command::UploadAndPrint {
            name: "old".into(),
            gcode: Arc::new("G28\nG1 X10 F600\n".into()),
        })
        .unwrap();
        let s = wait_for(&shared, |s| logged(s, "Couldn't print old"));
        assert!(logged(&s, "plate size"));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        assert!(actions_heard(&heard).is_empty());
    }

    #[test]
    fn a_damaged_answer_is_asked_for_again() {
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let mut fake = FakeSailfish::new(heard.clone());
        fake.damage = 2;
        let mut conn = Conn::new(fake, None, FAST.reply_wait);
        let a = conn.ask(Query::Version).unwrap();
        assert_eq!(query::read_version(&a), Some(708));
        assert_eq!(heard.lock().unwrap().len(), 3);
    }

    #[test]
    fn a_full_queue_comes_back_at_once_for_commands() {
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let mut fake = FakeSailfish::new(heard.clone());
        fake.full_every = 1;
        let mut conn = Conn::new(fake, None, FAST.reply_wait);
        assert_eq!(
            conn.send_action(&Action::BuildEnd),
            Err(AskError::Refused(Reply::BufferFull))
        );
        assert_eq!(heard.lock().unwrap().len(), 1, "not sent again by itself");
    }

    #[test]
    fn an_unsupported_question_is_not_repeated() {
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeSailfish::new(heard.clone());
        let mut conn = Conn::new(fake, None, FAST.reply_wait);
        let r = conn.ask(Query::Tool {
            tool: 2,
            what: ToolQuery::Target,
        });
        assert_eq!(r, Err(AskError::Refused(Reply::Unsupported)));
        assert_eq!(heard.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_printer_that_never_answers_ends_the_session_with_a_reason() {
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let mut fake = FakeSailfish::new(heard.clone());
        fake.silent = true;
        let (w, _tx, _stop, shared) = worker();
        let mut conn = Conn::new(fake, None, Duration::from_millis(20));
        let why = session(&w, &mut conn, &mut ContactLog::default(), FAST).unwrap();
        assert!(why.contains("didn't answer"), "{why}");
        assert!(why.contains("115200 baud"), "{why}");
        assert_eq!(heard.lock().unwrap().len(), TRIES as usize);
        assert!(!shared.lock().unwrap().connected);
    }

    #[test]
    fn names_the_firmware_from_its_versions() {
        let adv = query::AdvancedVersion {
            firmware: 708,
            internal: 0,
            variant: 0x80,
        };
        assert_eq!(firmware_name(Some(708), Some(adv)), "Sailfish 7.8");
        assert_eq!(
            firmware_name(Some(705), None),
            "MakerBot-protocol firmware 7.5"
        );
        assert_eq!(
            firmware_name(None, None),
            "MakerBot-protocol firmware (version not reported)"
        );
    }
}
