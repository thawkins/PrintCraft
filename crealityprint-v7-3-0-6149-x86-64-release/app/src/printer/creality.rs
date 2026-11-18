//! Creality's own network service on stock K1-family / Ender-3 V3 KE firmware.
//!
//! From public community documentation (not from the original slicer; see PRINTER_LINK.md):
//! * `ws://<printer>:9999` sends JSON status updates (only the fields that changed) and
//!   accepts `{"method":"set","params":{...}}` commands.
//! * Files are uploaded with an HTTP multipart POST to `http://<printer>/upload/<name>`
//!   and land in `/usr/data/printer_data/gcodes/`.
//! * A heartbeat keeps the socket open.

use super::link::{ContactLog, Worker};
use super::{
    fan_gcode, jog_gcode, json_number, multipart, Command, FoundPrinter, HistoryEntry, JobState,
    PrinterStatus, RemoteFile,
};
use serde_json::{json, Value};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

pub const WS_PORT: u16 = 9999;
pub const GCODE_DIR: &str = "/usr/data/printer_data/gcodes/";
const HEARTBEAT_EVERY: Duration = Duration::from_secs(10);
/// If nothing at all arrives for this long, the connection is treated as lost.
const SILENCE_LIMIT: Duration = Duration::from_secs(45);
/// Wait between attempts to (re)connect.
const RETRY_SECONDS: f32 = 3.0;

#[cfg(test)]
pub fn start_print_message(file: &str) -> String {
    start_message(&format!("{GCODE_DIR}{file}"), None)
}

/// Starts a print. With `calibrate`, also tells the printer whether to run its own
/// pre-print calibration (`enableSelfTest`, community-reported; unverified on the KE).
pub fn start_message(full_path: &str, calibrate: Option<bool>) -> String {
    let mut params = json!({"opGcodeFile": format!("printprt:{full_path}")});
    if let Some(on) = calibrate {
        params["enableSelfTest"] = json!(if on { 1 } else { 0 });
    }
    json!({"method": "set", "params": params}).to_string()
}

/// Turns the printer's own "calibrate before printing" on or off.
pub fn self_test_message(on: bool) -> String {
    json!({"method": "set", "params": {"enableSelfTest": if on { 1 } else { 0 }}}).to_string()
}

pub fn pause_message(pause: bool) -> String {
    json!({"method": "set", "params": {"pause": if pause { 1 } else { 0 }}}).to_string()
}

pub fn stop_message() -> String {
    json!({"method": "set", "params": {"stop": 1}}).to_string()
}

pub fn heartbeat_message() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    json!({"ModeCode": "heart_beat", "msg": now.to_string()}).to_string()
}

/// Folds one status message into what we know (messages only carry changed fields).
pub fn apply_message(s: &mut PrinterStatus, v: &Value) {
    let f = |key: &str| v.get(key).and_then(json_number);
    if let Some(x) = f("nozzleTemp") {
        s.nozzle = Some(x as f32);
    }
    if let Some(x) = f("targetNozzleTemp") {
        s.nozzle_target = Some(x as f32);
    }
    if let Some(x) = f("bedTemp0") {
        s.bed = Some(x as f32);
    }
    if let Some(x) = f("targetBedTemp0") {
        s.bed_target = Some(x as f32);
    }
    if let Some(x) = f("printProgress") {
        s.progress = Some((x as f32).clamp(0.0, 100.0));
    }
    if let Some(x) = f("printJobTime") {
        s.elapsed_s = Some(x.max(0.0) as u64);
    }
    if let Some(x) = f("printLeftTime") {
        s.remaining_s = Some(x.max(0.0) as u64);
    }
    if let Some(x) = f("layer") {
        s.layer = Some(x as u32);
    }
    if let Some(x) = f("TotalLayer") {
        s.total_layers = Some(x as u32);
    }
    if let Some(name) = v.get("printFileName").and_then(|x| x.as_str()) {
        let base = name.rsplit('/').next().unwrap_or(name);
        s.file = (!base.is_empty()).then(|| base.to_string());
        s.file_path = (!base.is_empty()).then(|| name.to_string());
    }
    if let Some(x) = f("modelFanPct") {
        s.fan_percent = Some(x as f32);
    }
    if let Some(x) = f("curFeedratePct") {
        s.speed_percent = Some(x as f32);
    }
    // The printer's own "calibrate before printing" setting, under either reported name.
    if let Some(x) = f("enableSelfTest").or_else(|| f("withSelfTest")) {
        s.printer_calibration = Some(x != 0.0);
    }
    if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
        s.model = Some(m.to_string());
    }
    if let Some(h) = v.get("hostname").and_then(|x| x.as_str()) {
        s.hostname = Some(h.to_string());
    }
    if let Some(code) = f("state") {
        s.state = match code as i64 {
            0 => JobState::Idle,
            1 => JobState::Printing,
            2 => JobState::Finished,
            3 => JobState::Failed,
            4 => JobState::Stopped,
            5 => JobState::Paused,
            _ => s.state,
        };
    }
}

fn open(host: &str) -> Result<WebSocket<TcpStream>, String> {
    let stream = super::link::connect(host, WS_PORT, Duration::from_secs(4))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let (ws, _) = tungstenite::client(format!("ws://{host}:{WS_PORT}/"), stream)
        .map_err(|e| e.to_string())?;
    ws.get_ref()
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|e| e.to_string())?;
    Ok(ws)
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

/// Quick look at a printer during discovery: listens for up to two seconds for its name.
pub fn identify(host: &str) -> Option<FoundPrinter> {
    let mut ws = open(host).ok()?;
    let mut status = PrinterStatus::default();
    let until = Instant::now() + Duration::from_secs(2);
    let _ = ws.send(Message::text(heartbeat_message()));
    while Instant::now() < until && (status.model.is_none() || status.hostname.is_none()) {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                    apply_message(&mut status, &v);
                }
            }
            Ok(_) => {}
            Err(e) if would_block(&e) => {}
            Err(_) => break,
        }
    }
    let _ = ws.close(None);
    Some(FoundPrinter {
        host: host.to_string(),
        name: status
            .hostname
            .clone()
            .unwrap_or_else(|| format!("Printer at {host}")),
        model: status.model.unwrap_or_else(|| "Creality printer".into()),
        api: Some(super::Api::CrealityLan),
        ..Default::default()
    })
}

pub fn upload(host: &str, name: &str, data: &[u8]) -> Result<(), String> {
    let (content_type, body) = multipart(&[("file", Some(name), data)]);
    super::http_agent(Duration::from_secs(5), Duration::from_secs(300))
        .post(&format!("http://{host}/upload/{name}"))
        .set("Content-Type", &content_type)
        .send_bytes(&body)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn run(w: &Worker) {
    // A print start that could not be sent because the socket broke (typically after a
    // long upload); it goes out first on the next connection rather than being lost.
    let mut pending_start: Option<String> = None;
    let mut contact = ContactLog::default();
    while !w.stopped() {
        let problem = match open(&w.host) {
            Ok(mut ws) => {
                contact.reached(w);
                let ended = session(w, &mut ws, &mut pending_start);
                let _ = ws.close(None);
                ended
            }
            Err(e) => Some(e),
        };
        // No problem means we were asked to stop: nothing to report.
        if let Some(problem) = problem {
            contact.failed(w, &problem);
            w.pause(RETRY_SECONDS);
        }
    }
    w.update(|s| s.connected = false);
}

/// Talks to the printer until asked to stop (None) or the connection fails (why).
fn session(
    w: &Worker,
    ws: &mut WebSocket<TcpStream>,
    pending_start: &mut Option<String>,
) -> Option<String> {
    const BROKEN: &str = "the connection broke";
    let mut last_beat = Instant::now() - HEARTBEAT_EVERY;
    let mut last_heard = Instant::now();
    if let Some(start) = pending_start.take() {
        if !send_start(w, ws, start, pending_start) {
            return Some(BROKEN.into());
        }
    }
    loop {
        if w.stopped() {
            return None;
        }
        if last_beat.elapsed() >= HEARTBEAT_EVERY {
            if ws.send(Message::text(heartbeat_message())).is_err() {
                return Some(BROKEN.into());
            }
            last_beat = Instant::now();
        }
        if last_heard.elapsed() > SILENCE_LIMIT {
            return Some("the printer stopped answering".into());
        }
        while let Ok(cmd) = w.rx.try_recv() {
            let began = Instant::now();
            let ok = match run_command(w, cmd) {
                Outgoing::Nothing => true,
                Outgoing::Message(text) => ws.send(Message::text(text)).is_ok(),
                Outgoing::Start(text) => send_start(w, ws, text, pending_start),
            };
            if !ok {
                return Some(BROKEN.into());
            }
            // An upload blocks this loop for as long as it takes; the printer was not
            // silent in that time, we just weren't listening.
            if began.elapsed() > HEARTBEAT_EVERY {
                last_heard = Instant::now();
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                last_heard = Instant::now();
                let text = t.as_str();
                if text == "ok" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(text) else {
                    continue;
                };
                if v.get("ModeCode").and_then(|m| m.as_str()) == Some("heart_beat") {
                    let _ = ws.send(Message::text("ok"));
                } else {
                    w.update(|s| {
                        apply_message(&mut s.status, &v);
                        if let Some(files) = parse_files(&v) {
                            s.files = Some(files);
                        }
                        if let Some(history) = parse_history(&v) {
                            s.history = Some(history);
                        }
                    });
                }
            }
            Ok(_) => last_heard = Instant::now(),
            Err(e) if would_block(&e) => {}
            Err(e) => return Some(e.to_string()),
        }
    }
}

/// What a command leaves to send over the socket.
enum Outgoing {
    Nothing,
    Message(String),
    /// Starts a print: must not be lost if the socket breaks.
    Start(String),
}

/// Sends a print start; when the socket is broken it is kept for the next connection.
/// Returns false when the socket broke.
fn send_start(
    w: &Worker,
    ws: &mut WebSocket<TcpStream>,
    text: String,
    pending_start: &mut Option<String>,
) -> bool {
    if ws.send(Message::text(text.as_str())).is_ok() {
        w.update(|s| s.note("Starting the print."));
        true
    } else {
        *pending_start = Some(text);
        false
    }
}

fn run_command(w: &Worker, cmd: Command) -> Outgoing {
    let text = match cmd {
        Command::UploadAndPrint { name, gcode } => {
            w.update(|s| s.busy = Some(format!("Sending {name} to the printer…")));
            let sent = upload(&w.host, &name, gcode.as_bytes());
            w.update(|s| {
                s.busy = None;
                match &sent {
                    Ok(()) => s.note(format!("Sent {name}.")),
                    Err(e) => s.note(format!("Sending {name} failed: {e}")),
                }
            });
            if sent.is_err() {
                return Outgoing::Nothing;
            }
            return Outgoing::Start(start_message(
                &format!("{GCODE_DIR}{name}"),
                calibration_choice(w),
            ));
        }
        Command::PrintFile(path) => print_path_message_with(&path, calibration_choice(w)),
        Command::SetCalibration(on) => {
            w.update(|s| {
                s.calibrate = Some(on);
                s.note(if on {
                    "The printer will calibrate before printing."
                } else {
                    "The printer will skip calibration before printing."
                });
            });
            self_test_message(on)
        }
        Command::Pause => pause_message(true),
        Command::Resume => pause_message(false),
        Command::Stop => stop_message(),
        Command::SetNozzle(t) => {
            json!({"method": "set", "params": {"nozzleTempControl": t.round()}}).to_string()
        }
        Command::SetBed(t) => {
            json!({"method": "set", "params": {"bedTempControl": {"num": 0, "val": t.round()}}})
                .to_string()
        }
        Command::SetFan(p) => gcode_message(&fan_gcode(p)),
        Command::SetSpeed(p) => gcode_message(&format!("M220 S{p}")),
        Command::Jog { axis, mm } => gcode_message(&jog_gcode(axis, mm).join("\n")),
        Command::Home => gcode_message("G28"),
        Command::CancelObject(_) => {
            w.update(|s| {
                s.note("This connection can't cancel one object; use the printer's screen.")
            });
            return Outgoing::Nothing;
        }
        Command::RefreshFiles => {
            json!({"method": "get", "params": {"reqGcodeFile": 1}}).to_string()
        }
        Command::RefreshHistory => {
            json!({"method": "get", "params": {"reqHistory": 1}}).to_string()
        }
    };
    Outgoing::Message(text)
}

/// Runs G-code on the printer through the service's `gcodeCmd` setting.
pub fn gcode_message(gcode: &str) -> String {
    json!({"method": "set", "params": {"gcodeCmd": gcode}}).to_string()
}

/// Starts a file already on the printer: a full path, or a name in the gcodes folder.
#[cfg(test)]
pub fn print_path_message(path: &str) -> String {
    print_path_message_with(path, None)
}

pub fn print_path_message_with(path: &str, calibrate: Option<bool>) -> String {
    if path.starts_with('/') {
        start_message(path, calibrate)
    } else {
        start_message(&format!("{GCODE_DIR}{path}"), calibrate)
    }
}

/// The Enable Calibration choice the app last sent, if any.
fn calibration_choice(w: &Worker) -> Option<bool> {
    w.shared.lock().ok().and_then(|s| s.calibrate)
}

fn text_of<'a>(o: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| o.get(*k).and_then(|x| x.as_str()))
}

fn number_of(o: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| o.get(*k).and_then(json_number))
}

/// Lists of objects under any key whose name contains `part` (any case).
fn lists_under<'a>(v: &'a Value, part: &str) -> Vec<&'a Vec<Value>> {
    let Some(map) = v.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(k, _)| k.to_ascii_lowercase().contains(part))
        .filter_map(|(_, x)| x.as_array())
        .collect()
}

/// The printer's file list, read loosely: the service's exact field names vary between
/// firmware versions, so any list under a "...GcodeFile..." key with name-like fields counts.
pub fn parse_files(v: &Value) -> Option<Vec<RemoteFile>> {
    let lists = lists_under(v, "gcodefile");
    if lists.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for item in lists.into_iter().flatten() {
        // The file's own name first; "path" may be just its folder.
        let Some(full) = text_of(item, &["name", "filename", "file_name", "path"]) else {
            continue;
        };
        let base = full.rsplit('/').next().unwrap_or(full).to_string();
        let path = match (
            item.get("path").and_then(|p| p.as_str()),
            item.get("name").and_then(|n| n.as_str()),
        ) {
            (Some(dir), Some(name)) if !dir.ends_with(name) => {
                format!("{}/{}", dir.trim_end_matches('/'), name)
            }
            _ => full.to_string(),
        };
        out.push(RemoteFile {
            name: base,
            path,
            size: number_of(item, &["file_size", "size"]).map(|x| x as u64),
            modified: number_of(item, &["create_time", "modified", "mtime", "time"])
                .map(|x| x as i64),
        });
    }
    Some(out)
}

/// The printer's own print history, read as loosely as the file list.
pub fn parse_history(v: &Value) -> Option<Vec<HistoryEntry>> {
    let lists = lists_under(v, "history");
    if lists.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for item in lists.into_iter().flatten() {
        let Some(name) = text_of(item, &["filename", "file_name", "name"]) else {
            continue;
        };
        let outcome = match item
            .get("printfinish")
            .or_else(|| item.get("status"))
            .or_else(|| item.get("state"))
        {
            Some(Value::Number(n)) if n.as_i64() == Some(1) => "Finished".to_string(),
            Some(Value::Number(_)) => "Not finished".to_string(),
            Some(Value::Bool(true)) => "Finished".to_string(),
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        let start = number_of(item, &["starttime", "start_time"]);
        let used = number_of(item, &["usagetime", "print_time", "total_duration"]);
        out.push(HistoryEntry {
            name: name.rsplit('/').next().unwrap_or(name).to_string(),
            outcome,
            ended: start.map(|s| (s + used.unwrap_or(0.0)) as i64),
            duration_s: used.map(|u| u as u64),
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_messages_merge() {
        let mut s = PrinterStatus::default();
        apply_message(
            &mut s,
            &json!({"nozzleTemp": "205.31", "targetNozzleTemp": 210, "bedTemp0": "59.9", "state": 1,
                    "printProgress": 42, "printJobTime": 600, "printLeftTime": 900,
                    "printFileName": "/usr/data/printer_data/gcodes/cube.gcode", "layer": 12, "TotalLayer": 100}),
        );
        apply_message(&mut s, &json!({"printProgress": 43}));
        assert_eq!(s.state, JobState::Printing);
        assert_eq!(s.progress, Some(43.0));
        assert!((s.nozzle.unwrap() - 205.31).abs() < 1e-3);
        assert_eq!(s.file.as_deref(), Some("cube.gcode"));
        assert_eq!((s.layer, s.total_layers), (Some(12), Some(100)));
        assert_eq!(s.remaining_s, Some(900));
    }

    #[test]
    fn start_message_points_at_gcode_folder() {
        let m: Value = serde_json::from_str(&start_print_message("cube.gcode")).unwrap();
        assert_eq!(
            m["params"]["opGcodeFile"],
            "printprt:/usr/data/printer_data/gcodes/cube.gcode"
        );
        let p: Value = serde_json::from_str(&pause_message(true)).unwrap();
        assert_eq!(p["params"]["pause"], 1);
    }

    #[test]
    fn reads_file_lists_and_history_loosely() {
        let files = parse_files(&json!({"retGcodeFileInfo2": [
            {"name": "spacer.gcode", "path": "/usr/data/printer_data/gcodes", "file_size": 812345, "create_time": 1700000000}
        ]}))
        .unwrap();
        assert_eq!(files[0].name, "spacer.gcode");
        assert_eq!(files[0].path, "/usr/data/printer_data/gcodes/spacer.gcode");
        assert_eq!(files[0].size, Some(812345));
        assert!(
            parse_files(&json!({"nozzleTemp": "200"})).is_none(),
            "status messages aren't file lists"
        );
        let h = parse_history(&json!({"historyList": [
            {"filename": "/usr/data/printer_data/gcodes/hook.gcode", "printfinish": 1, "starttime": 1000, "usagetime": 600}
        ]}))
        .unwrap();
        assert_eq!(
            (
                h[0].name.as_str(),
                h[0].outcome.as_str(),
                h[0].ended,
                h[0].duration_s
            ),
            ("hook.gcode", "Finished", Some(1600), Some(600))
        );
    }

    #[test]
    fn enable_calibration_reaches_the_printer() {
        let off: Value = serde_json::from_str(&self_test_message(false)).unwrap();
        assert_eq!(off["params"]["enableSelfTest"], 0);
        let on: Value = serde_json::from_str(&self_test_message(true)).unwrap();
        assert_eq!(on["params"]["enableSelfTest"], 1);
        // The start command carries the choice too.
        let start: Value = serde_json::from_str(&start_message(
            "/usr/data/printer_data/gcodes/a.gcode",
            Some(false),
        ))
        .unwrap();
        assert_eq!(start["params"]["enableSelfTest"], 0);
        assert_eq!(
            start["params"]["opGcodeFile"],
            "printprt:/usr/data/printer_data/gcodes/a.gcode"
        );
        let plain: Value = serde_json::from_str(&start_print_message("a.gcode")).unwrap();
        assert!(
            plain["params"].get("enableSelfTest").is_none(),
            "nothing sent unless chosen"
        );
        let stored: Value =
            serde_json::from_str(&print_path_message_with("b.gcode", Some(true))).unwrap();
        assert_eq!(
            stored["params"]["opGcodeFile"],
            "printprt:/usr/data/printer_data/gcodes/b.gcode"
        );
        assert_eq!(stored["params"]["enableSelfTest"], 1);
        // The printer's own setting is read back under either name.
        let mut s = PrinterStatus::default();
        apply_message(&mut s, &json!({"withSelfTest": 0}));
        assert_eq!(s.printer_calibration, Some(false));
        apply_message(&mut s, &json!({"enableSelfTest": 1}));
        assert_eq!(s.printer_calibration, Some(true));
    }

    #[test]
    fn controls_and_reprint_messages() {
        let v: Value =
            serde_json::from_str(&print_path_message("/usr/data/printer_data/gcodes/a.gcode"))
                .unwrap();
        assert_eq!(
            v["params"]["opGcodeFile"],
            "printprt:/usr/data/printer_data/gcodes/a.gcode"
        );
        let v: Value = serde_json::from_str(&gcode_message("M220 S125")).unwrap();
        assert_eq!(v["params"]["gcodeCmd"], "M220 S125");
        let mut s = PrinterStatus::default();
        apply_message(&mut s, &json!({"modelFanPct": 48, "curFeedratePct": 100}));
        assert_eq!((s.fan_percent, s.speed_percent), (Some(48.0), Some(100.0)));
    }
}
