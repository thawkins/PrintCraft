//! A USB connection to a printer that speaks the MakerBot S3G protocol: Sailfish and
//! MakerBot firmware (FlashForge Creator Pro and early Creator Pro 2, Replicator 1/2/2X
//! and their clones). The protocol is binary packets, one in flight at a time; see
//! `crate::x3g` and makerbot.md.
//!
//! **This first stage only reads from the printer.** It asks for the firmware version and
//! variant, copies the EEPROM (the machine's settings: steps per mm, axis lengths, nozzle
//! count and offset, Ditto printing) into `sailfish-eeprom.bin` beside the serial log,
//! and then shows temperatures and the build state every couple of seconds. It never
//! sends a command that moves, heats, writes or restarts anything. Printing and the
//! controls come with the next stages, once the settings read here are known.

use super::link::{ContactLog, Worker};
use super::serial_log::SerialLog;
use super::{Command, JobState};
use crate::x3g::query::{
    self, BuildState, MachineSettings, Query, ToolQuery, EEPROM_CHUNK, EEPROM_SIZE,
};
use crate::x3g::{self, Incoming, PacketReader, Reply};
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
    fn describe(&self) -> String {
        match self {
            AskError::NoAnswer => "the printer didn't answer".into(),
            AskError::Refused(r) => format!("the printer answered \"{}\"", r.describe()),
            AskError::Port(e) => e.clone(),
        }
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
        let packet = x3g::frame(&q.payload());
        let what = q.describe();
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
        s.note(format!(
            "The printer runs {name} (MakerBot protocol). This connection only reads from \
             it for now: temperatures, the build state and its settings."
        ));
        s.status.firmware = Some(name.clone());
        s.status.model = Some(name.clone());
    });

    let settings = match read_settings(w, conn) {
        Ok(Some(s)) => s,
        Ok(None) => return None,
        Err(e) => return Some(e),
    };
    let tools = settings.tool_count.unwrap_or(1).clamp(1, 2);
    let heated_bed = settings.heated_bed.unwrap_or(true);
    match conn.ask(Query::Position).map(|a| query::read_position(&a)) {
        Ok(Some(p)) => conn.note(&format!(
            "Position (steps): X {}, Y {}, Z {}, A {}, B {}.",
            p[0], p[1], p[2], p[3], p[4]
        )),
        Err(AskError::Port(e)) => return Some(e),
        _ => {}
    }

    let mut last_poll: Option<Instant> = None;
    let mut memory = PollMemory::default();
    let mut refused_told = false;
    loop {
        if w.stopped() {
            // Nothing was started from here, so there is nothing to switch off.
            return None;
        }
        while let Ok(cmd) = w.rx.try_recv() {
            match cmd {
                Command::SetCalibration(on) => w.update(|s| s.calibrate = Some(on)),
                Command::RefreshFiles | Command::RefreshHistory => {}
                other => {
                    let what = command_words(&other);
                    conn.note(&format!("Not sent ({what}): read-only connection."));
                    let first = !refused_told;
                    refused_told = true;
                    w.update(|s| {
                        s.note(if first {
                            format!(
                                "Not sent: {what}. The MakerBot-protocol connection only \
                                 reads from the printer for now; use the printer's own \
                                 screen to control it."
                            )
                        } else {
                            format!("Not sent: {what} (read-only connection).")
                        })
                    });
                }
            }
        }
        if last_poll.is_none_or(|t| t.elapsed() >= timing.poll_every) {
            last_poll = Some(Instant::now());
            if let Err(e) = poll(w, conn, tools, heated_bed, &mut memory) {
                return Some(e);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn command_words(c: &Command) -> &'static str {
    match c {
        Command::UploadAndPrint { .. } | Command::PrintFile(_) => "starting a print",
        Command::Pause => "pause",
        Command::Resume => "resume",
        Command::Stop => "stop",
        Command::SetNozzle(_) => "nozzle temperature",
        Command::SetBed(_) => "bed temperature",
        Command::SetFan(_) => "fan",
        Command::SetSpeed(_) => "speed",
        Command::Jog { .. } => "move",
        Command::Home => "home",
        Command::SetCalibration(_) | Command::RefreshFiles | Command::RefreshHistory => "request",
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
    memory: &mut PollMemory,
) -> Result<(), String> {
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
    Ok(())
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

    /// A pretend Sailfish printer: answers questions the way Sailfish's `Host.cc` does.
    struct FakeSailfish {
        reader: PacketReader,
        out: VecDeque<u8>,
        eeprom: Vec<u8>,
        /// Command codes received, in order.
        heard: Arc<Mutex<Vec<u8>>>,
        /// Damage the CRC of this many answers first.
        damage: u32,
        /// Never answer.
        silent: bool,
    }

    impl FakeSailfish {
        fn new(heard: Arc<Mutex<Vec<u8>>>) -> Self {
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
            }
        }

        fn answer(&self, p: &[u8]) -> Vec<u8> {
            let temp = |c: i16| {
                let b = c.to_le_bytes();
                vec![0x81, b[0], b[1]]
            };
            match p[0] {
                0 => vec![0x81, 0xC4, 0x02],
                27 => vec![0x81, 0xC4, 0x02, 0, 0, 0x80, 0, 0, 0],
                10 => match (p[1], p[2]) {
                    (0, 2) => temp(205),
                    (0, 32) => temp(210),
                    (1, 2) => temp(25),
                    (1, 32) => temp(0),
                    (_, 30) => temp(58),
                    (_, 33) => temp(60),
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
                24 => vec![0x81, 1, 0, 42, 0, 0, 0, 0, 0, 0, 0, 0],
                20 => {
                    let mut a = vec![0x81];
                    a.extend(b"cube.x3g\0");
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
                self.heard.lock().unwrap().push(p[0]);
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

    fn wait_for(shared: &Shared, what: impl Fn(&LinkState) -> bool) -> LinkState {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let s = shared.lock().unwrap().clone();
            if what(&s) {
                return s;
            }
            assert!(Instant::now() < until, "timed out; state: {s:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn reads_versions_settings_and_temperatures_without_changing_anything() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeSailfish::new(heard.clone());
        let (w, tx, stop, shared) = worker();
        let t = std::thread::spawn(move || {
            let mut conn = Conn::new(fake, None, FAST.reply_wait);
            session(&w, &mut conn, &mut ContactLog::default(), FAST)
        });
        let s = wait_for(&shared, |s| s.status.nozzle2.is_some());
        assert!(s.connected);
        assert_eq!(s.status.firmware.as_deref(), Some("Sailfish 7.8"));
        assert_eq!(s.status.nozzle, Some(205.0));
        assert_eq!(s.status.nozzle_target, Some(210.0));
        assert_eq!(s.status.nozzle2, Some(25.0));
        assert_eq!(s.status.bed, Some(58.0));
        assert_eq!(s.status.bed_target, Some(60.0));
        assert_eq!(s.status.state, JobState::Printing);
        assert_eq!(s.status.file.as_deref(), Some("cube.x3g"));
        assert_eq!(s.status.elapsed_s, Some(42 * 60));
        let log = s.log.iter().cloned().collect::<Vec<_>>().join("\n");
        assert!(log.contains("\"Creator Pro\": 2 nozzle(s)"), "{log}");
        assert!(log.contains("Ditto printing on"), "{log}");

        tx.send(Command::Home).unwrap();
        tx.send(Command::SetNozzle(200.0)).unwrap();
        let s = wait_for(&shared, |s| {
            s.log
                .iter()
                .any(|l| l.contains("Not sent: nozzle temperature"))
        });
        assert!(s.log.iter().any(|l| l.starts_with("Not sent: home.")));

        stop.store(true, Ordering::Relaxed);
        assert_eq!(t.join().unwrap(), None);
        let heard = heard.lock().unwrap();
        assert!(
            heard.iter().all(|&c| c < 128 && c != 3 && c != 7),
            "only harmless questions may be sent: {heard:?}"
        );
        let eeprom_reads = heard.iter().filter(|&&c| c == 12).count();
        assert_eq!(
            eeprom_reads,
            (EEPROM_SIZE as usize).div_ceil(EEPROM_CHUNK as usize),
            "the whole EEPROM is copied once"
        );
    }

    #[test]
    fn a_damaged_answer_is_asked_for_again() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let mut fake = FakeSailfish::new(heard.clone());
        fake.damage = 2;
        let mut conn = Conn::new(fake, None, FAST.reply_wait);
        let a = conn.ask(Query::Version).unwrap();
        assert_eq!(query::read_version(&a), Some(708));
        assert_eq!(*heard.lock().unwrap(), vec![0, 0, 0]);
    }

    #[test]
    fn an_unsupported_question_is_not_repeated() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeSailfish::new(heard.clone());
        let mut conn = Conn::new(fake, None, FAST.reply_wait);
        assert_eq!(
            conn.ask(Query::Tool {
                tool: 0,
                what: ToolQuery::Temperature
            })
            .map(|a| a.len()),
            Ok(2)
        );
        heard.lock().unwrap().clear();
        let r = conn.ask(Query::Tool {
            tool: 2,
            what: ToolQuery::Target,
        });
        assert_eq!(r, Err(AskError::Refused(Reply::Unsupported)));
        assert_eq!(heard.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_printer_that_never_answers_ends_the_session_with_a_reason() {
        let heard = Arc::new(Mutex::new(Vec::new()));
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
