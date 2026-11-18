//! Duet boards running RepRapFirmware (RRF). Two documented HTTP APIs, depending on how
//! the board is set up:
//!
//! * **Standalone** (the board serves the web page itself): the `rr_` requests.
//!   `GET /rr_connect?password=` starts a session (`{"err": 0}`; `err` 1 = wrong password;
//!   RRF 3.5 adds a `sessionKey`, sent back as `X-Session-Key`). Status is the object
//!   model, `GET /rr_model?key=&flags=d99vn` (`result`). Upload is `POST
//!   /rr_upload?name=0:/gcodes/<name>` with the file as the body, and G-code runs through
//!   `GET /rr_gcode?gcode=`. Files: `GET /rr_filelist?dir=0:/gcodes`.
//! * **With a Raspberry Pi (SBC, Duet Software Framework)**: the `/machine` requests.
//!   `GET /machine/connect?password=` gives a `sessionKey` (older versions have no
//!   sign-in and answer 404); status is `GET /machine/status` (the same object model);
//!   upload is `PUT /machine/file/0:/gcodes/<name>`; G-code is `POST /machine/code` with the
//!   code as the body; files: `GET /machine/directory/0:/gcodes`.
//!
//! The password is the one set with M551 ("reprap" when none was set). It is never saved.
//! Prints start with `M32 "<file>"`; pause is M25, resume M24, and stop is M25 then M0
//! (RRF cancels a paused print). One object is cancelled with `M486 P<n>`, the object's
//! number in `job.build.objects` (filled from the M486 labels in the file).

use super::link::{ContactLog, Worker};
use super::{
    fan_gcode, jog_gcode, json_number, nozzle_gcode, url_encode, Command, FoundPrinter, JobState,
    PrintObject, PrinterStatus, RemoteFile,
};
use serde_json::Value;
use std::time::Duration;

pub const PORT: u16 = 80;
const POLL_SECONDS: f32 = 2.0;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// RRF's password when none has been set.
pub const DEFAULT_PASSWORD: &str = "reprap";
/// Where print files go on the board's SD card (or the Pi's virtual SD card).
const GCODES: &str = "0:/gcodes";

/// Which of the two APIs the board answers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Standalone,
    Sbc,
}

struct Client {
    host: String,
    password: String,
    mode: Mode,
    session: Option<String>,
    agent: ureq::Agent,
}

impl Client {
    fn new(host: &str, password: &str) -> Self {
        Self {
            host: host.to_string(),
            password: password.to_string(),
            mode: Mode::Standalone,
            session: None,
            agent: super::http_agent(Duration::from_secs(4), Duration::from_secs(10)),
        }
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        let req = self
            .agent
            .request(method, &format!("http://{}{path}", self.host));
        match &self.session {
            Some(key) => req.set("X-Session-Key", key),
            None => req,
        }
    }

    fn answer(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
        match result {
            Ok(resp) => {
                let text = resp.into_string().map_err(|e| e.to_string())?;
                Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
            }
            Err(ureq::Error::Status(401 | 403, _)) => Err("signed out".into()),
            Err(ureq::Error::Status(code, _)) => Err(format!("The board answered {code}.")),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Signs in, finding out which API the board has.
    fn connect(&mut self) -> Result<(), String> {
        self.session = None;
        let pw = url_encode(&self.password);
        match self
            .agent
            .get(&format!("http://{}/rr_connect?password={pw}", self.host))
            .call()
        {
            Ok(resp) => {
                let v: Value = resp
                    .into_string()
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .ok_or("The board's answer wasn't understood.")?;
                match v.get("err").and_then(json_number).map(|e| e as i64) {
                    Some(0) => {
                        self.mode = Mode::Standalone;
                        self.session = v.get("sessionKey").map(key_text);
                        Ok(())
                    }
                    Some(1) => Err("Wrong Duet password.".into()),
                    Some(2) => Err("The board has no free sessions; close a browser tab.".into()),
                    _ => Err("The board refused the connection.".into()),
                }
            }
            // No rr_ API: a board run by a Raspberry Pi.
            Err(ureq::Error::Status(404, _)) => {
                self.mode = Mode::Sbc;
                match self
                    .agent
                    .get(&format!(
                        "http://{}/machine/connect?password={pw}",
                        self.host
                    ))
                    .call()
                {
                    Ok(resp) => {
                        let v: Value = resp
                            .into_string()
                            .ok()
                            .and_then(|t| serde_json::from_str(&t).ok())
                            .unwrap_or(Value::Null);
                        self.session = v.get("sessionKey").map(key_text);
                        Ok(())
                    }
                    // Older DSF versions have no sign-in.
                    Err(ureq::Error::Status(404, _)) => Ok(()),
                    Err(ureq::Error::Status(403, _)) => Err("Wrong Duet password.".into()),
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// The object model.
    fn status(&self) -> Result<Value, String> {
        match self.mode {
            Mode::Standalone => {
                Self::answer(self.request("GET", "/rr_model?key=&flags=d99vn").call())
                    .map(|v| v.get("result").cloned().unwrap_or(Value::Null))
            }
            Mode::Sbc => Self::answer(self.request("GET", "/machine/status").call()),
        }
    }

    /// Runs G-code (lines separated by newlines).
    fn gcode(&self, code: &str) -> Result<(), String> {
        match self.mode {
            Mode::Standalone => Self::answer(
                self.request("GET", &format!("/rr_gcode?gcode={}", url_encode(code)))
                    .call(),
            ),
            Mode::Sbc => Self::answer(
                self.request("POST", "/machine/code")
                    .set("Content-Type", "text/plain")
                    .send_string(code),
            ),
        }
        .map(|_| ())
    }

    /// Puts `data` on the card as `path` (`0:/gcodes/<name>`).
    fn upload(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let v = match self.mode {
            Mode::Standalone => Self::answer(
                self.request("POST", &format!("/rr_upload?name={}", url_encode(path)))
                    .timeout(UPLOAD_TIMEOUT)
                    .set("Content-Type", "application/octet-stream")
                    .send_bytes(data),
            )?,
            Mode::Sbc => Self::answer(
                self.request("PUT", &format!("/machine/file/{}", url_encode(path)))
                    .timeout(UPLOAD_TIMEOUT)
                    .set("Content-Type", "application/octet-stream")
                    .send_bytes(data),
            )?,
        };
        match v.get("err").and_then(json_number) {
            Some(e) if e != 0.0 => Err("The board couldn't store the file.".into()),
            _ => Ok(()),
        }
    }

    fn files(&self) -> Result<Vec<RemoteFile>, String> {
        let v = match self.mode {
            Mode::Standalone => Self::answer(
                self.request("GET", &format!("/rr_filelist?dir={}", url_encode(GCODES)))
                    .call(),
            )?
            .get("files")
            .cloned()
            .unwrap_or(Value::Null),
            Mode::Sbc => Self::answer(
                self.request("GET", &format!("/machine/directory/{}", url_encode(GCODES)))
                    .call(),
            )?,
        };
        Ok(parse_files(&v))
    }
}

/// A session key as text (RRF sends a number, DSF a string).
fn key_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A directory listing (`[{"type": "f", "name", "size", "date"}]`): the files in the
/// print folder, newest first.
pub fn parse_files(v: &Value) -> Vec<RemoteFile> {
    let mut out: Vec<RemoteFile> = v
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|f| f.get("type").and_then(|t| t.as_str()) == Some("f"))
        .filter_map(|f| {
            let name = f.get("name")?.as_str()?.to_string();
            Some(RemoteFile {
                path: format!("{GCODES}/{name}"),
                size: f.get("size").and_then(json_number).map(|x| x as u64),
                modified: f
                    .get("date")
                    .and_then(|d| d.as_str())
                    .and_then(seconds_since_1970),
                name,
            })
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.modified));
    out
}

/// `2024-03-01T10:20:30` (the board's local time, taken as is) as seconds since 1970.
fn seconds_since_1970(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let d: Vec<i64> = date
        .split('-')
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    let t: Vec<i64> = time
        .split(':')
        .map(|x| x.split('.').next()?.parse().ok())
        .collect::<Option<_>>()?;
    let (&[y, m, day], &[hh, mm, ss]) = (d.as_slice(), t.as_slice()) else {
        return None;
    };
    // Days from the civil date (the well-known algorithm by Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

/// Item `i` of the list `v`, if both are there.
fn at(v: Option<&Value>, i: usize) -> Option<&Value> {
    v.and_then(|x| x.as_array()).and_then(|a| a.get(i))
}

/// Folds the object model into what we know.
pub fn apply_model(s: &mut PrinterStatus, m: &Value) {
    let num = |v: Option<&Value>| v.and_then(json_number);
    let path = |p: &[&str]| p.iter().try_fold(m, |v, k| v.get(*k));
    if let Some(status) = path(&["state", "status"]).and_then(|x| x.as_str()) {
        s.state = match status {
            "printing" | "processing" | "simulating" | "resuming" => JobState::Printing,
            "paused" | "pausing" => JobState::Paused,
            "cancelling" => JobState::Stopped,
            "halted" => JobState::Failed,
            _ => JobState::Idle,
        };
    }
    let heaters = path(&["heat", "heaters"]);
    let heater = |i: Option<f64>| i.and_then(|i| at(heaters, i as usize));
    let bed = heater(num(at(path(&["heat", "bedHeaters"]), 0)));
    let nozzle = heater(
        num(at(
            at(path(&["tools"]), 0).and_then(|t| t.get("heaters")),
            0,
        ))
        .or(Some(1.0)),
    );
    if let Some(h) = bed {
        s.bed = num(h.get("current")).map(|x| x as f32);
        s.bed_target = num(h.get("active")).map(|x| x as f32);
    }
    if let Some(h) = nozzle {
        s.nozzle = num(h.get("current")).map(|x| x as f32);
        s.nozzle_target = num(h.get("active")).map(|x| x as f32);
    }
    if let Some(f) = num(at(path(&["fans"]), 0).and_then(|f| f.get("requestedValue"))) {
        s.fan_percent = Some((f as f32 * 100.0).clamp(0.0, 100.0));
    }
    if let Some(f) = num(path(&["move", "speedFactor"])) {
        s.speed_percent = Some(f as f32 * 100.0);
    }
    let Some(job) = m.get("job") else { return };
    let file = job
        .get("file")
        .and_then(|f| f.get("fileName"))
        .and_then(|x| x.as_str())
        .filter(|x| !x.is_empty());
    s.file = file.map(|f| f.rsplit('/').next().unwrap_or(f).to_string());
    s.file_path = file.map(str::to_string);
    let size = num(job.get("file").and_then(|f| f.get("size"))).filter(|x| *x > 0.0);
    s.progress = match (num(job.get("filePosition")), size, file) {
        (Some(p), Some(n), Some(_)) => Some((p / n * 100.0).clamp(0.0, 100.0) as f32),
        _ => None,
    };
    s.elapsed_s = num(job.get("duration")).map(|x| x as u64);
    let left = job.get("timesLeft");
    s.remaining_s = ["slicer", "file", "filament"]
        .iter()
        .find_map(|k| num(left.and_then(|l| l.get(*k))))
        .map(|x| x as u64);
    s.layer = num(job.get("layer")).map(|x| x as u32);
    s.total_layers = num(job.get("file").and_then(|f| f.get("numLayers")))
        .filter(|x| *x > 0.0)
        .map(|x| x as u32);
    s.objects = job
        .get("build")
        .and_then(|b| b.get("objects"))
        .and_then(|o| o.as_array())
        .map(|list| {
            list.iter()
                .enumerate()
                .map(|(i, o)| PrintObject {
                    name: o
                        .get("name")
                        .and_then(|n| n.as_str())
                        .filter(|n| !n.is_empty())
                        .map_or_else(|| format!("Object {i}"), str::to_string),
                    cancelled: o
                        .get("cancelled")
                        .and_then(|c| c.as_bool())
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
}

/// Does this address look like a Duet? A standalone board answers `rr_connect` with an
/// `err` number; one run by a Pi has no `rr_` requests but answers `/machine/status`
/// with an object model.
pub fn identify(host: &str) -> Option<FoundPrinter> {
    let agent = super::http_agent(Duration::from_millis(800), Duration::from_secs(3));
    let json = |path: &str| -> Option<Value> {
        let resp = agent.get(&format!("http://{host}{path}")).call().ok()?;
        serde_json::from_str(&resp.into_string().ok()?).ok()
    };
    let pw = url_encode(DEFAULT_PASSWORD);
    let standalone = json(&format!("/rr_connect?password={pw}")).filter(|v| v.get("err").is_some());
    let board = match standalone {
        Some(v) => {
            // Leave no session behind.
            let _ = agent.get(&format!("http://{host}/rr_disconnect")).call(); // no-error-ok: only tidies up
            Some(
                v.get("boardType")
                    .and_then(|b| b.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        }
        // Run by a Pi: its sign-in hands out a session key (or refuses the password),
        // or, on versions without sign-in, the status answers.
        None => {
            let signed_in = match agent
                .get(&format!("http://{host}/machine/connect?password={pw}"))
                .call()
            {
                Ok(resp) => resp
                    .into_string()
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .is_some_and(|v| v.get("sessionKey").is_some()),
                Err(ureq::Error::Status(403, _)) => true,
                Err(_) => false,
            };
            (signed_in
                || json("/machine/status")
                    .is_some_and(|v| v.get("state").is_some() || v.get("boards").is_some()))
            .then(String::new)
        }
    }?;
    Some(FoundPrinter {
        host: host.to_string(),
        name: format!("Duet at {host}"),
        model: if board.is_empty() {
            "Duet (RepRapFirmware)".into()
        } else {
            format!("Duet {board} (RepRapFirmware)")
        },
        api: Some(super::Api::Duet),
        ..Default::default()
    })
}

/// One command from the app; `objects` is the running print's object list.
fn run_command(
    w: &Worker,
    c: &Client,
    cmd: Command,
    objects: &[PrintObject],
) -> Result<(), String> {
    match cmd {
        Command::UploadAndPrint { name, gcode } => w.sending(&name, || {
            let path = format!("{GCODES}/{name}");
            c.upload(&path, gcode.as_bytes())?;
            c.gcode(&format!("M32 \"{path}\""))
        }),
        Command::UploadBinaryAndPrint { .. } => {
            Err("A Duet takes plain G-code, not binary (.bgcode).".into())
        }
        Command::PrintFile(path) => c.gcode(&format!("M32 \"{path}\"")),
        Command::Pause => c.gcode("M25"),
        Command::Resume => c.gcode("M24"),
        // RRF cancels a print once it is paused: the two run in order.
        Command::Stop => c.gcode("M25\nM0"),
        Command::SetNozzle(t) => c.gcode(&nozzle_gcode(t)),
        Command::SetBed(t) => c.gcode(&super::bed_gcode(t)),
        Command::SetFan(p) => c.gcode(&fan_gcode(p)),
        Command::SetSpeed(p) => c.gcode(&format!("M220 S{p}")),
        Command::Jog { axis, mm } => c.gcode(&jog_gcode(axis, mm).join("\n")),
        Command::Home => c.gcode("G28"),
        Command::RefreshFiles => c.files().map(|f| w.update(|s| s.files = Some(f))),
        // RRF keeps no print history through its API.
        Command::RefreshHistory => Ok(()),
        // The app adds (or leaves out) its own levelling step in the file.
        Command::SetCalibration(on) => {
            w.update(|s| s.calibrate = Some(on));
            Ok(())
        }
        Command::CancelObject(name) => match objects.iter().position(|o| o.name == name) {
            Some(i) => c.gcode(&format!("M486 P{i}")),
            None => Err(format!("The print has no object called {name}.")),
        },
    }
}

pub fn run(w: &Worker) {
    let password = w
        .printer
        .login
        .as_ref()
        .map(|l| l.1.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| DEFAULT_PASSWORD.to_string());
    let mut client = Client::new(&w.host, &password);
    let mut contact = ContactLog::default();
    let mut signed_in = false;
    let mut objects: Vec<PrintObject> = Vec::new();
    while !w.stopped() {
        if !signed_in {
            match client.connect() {
                Ok(()) => signed_in = true,
                Err(e) => {
                    contact.failed(w, &e);
                    w.pause(5.0);
                    continue;
                }
            }
        }
        while let Ok(cmd) = w.rx.try_recv() {
            if let Err(e) = run_command(w, &client, cmd, &objects) {
                w.update(|s| s.note(format!("The printer refused: {e}")));
            }
        }
        match client.status() {
            Ok(model) => {
                contact.reached(w);
                w.update(|s| {
                    apply_model(&mut s.status, &model);
                    objects = s.status.objects.clone();
                });
            }
            // The session ran out: sign in again.
            Err(e) if e == "signed out" => signed_in = false,
            Err(e) => contact.failed(w, &e),
        }
        w.pause(POLL_SECONDS);
    }
    if signed_in && client.mode == Mode::Standalone {
        let _ = client.request("GET", "/rr_disconnect").call(); // no-error-ok: only tidies up
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_duet_status_words_map_to_job_states() {
        for (word, want) in [
            ("processing", JobState::Printing),
            ("printing", JobState::Printing),
            ("simulating", JobState::Printing),
            ("paused", JobState::Paused),
            ("pausing", JobState::Paused),
            ("cancelling", JobState::Stopped),
            ("halted", JobState::Failed),
            ("idle", JobState::Idle),
            ("busy", JobState::Idle),
        ] {
            let mut s = PrinterStatus::default();
            apply_model(&mut s, &json!({"state": {"status": word}}));
            assert_eq!(s.state, want, "{word}");
        }
    }

    #[test]
    fn test_duet_file_dates_and_listing() {
        assert_eq!(seconds_since_1970("1970-01-01T00:00:00"), Some(0));
        assert_eq!(
            seconds_since_1970("2024-03-01T10:20:30"),
            Some(1_709_288_430)
        );
        assert_eq!(seconds_since_1970("not a date"), None);
        let files = parse_files(&json!([
            {"type": "f", "name": "old.gcode", "size": 10, "date": "2023-01-01T00:00:00"},
            {"type": "d", "name": "folder"},
            {"type": "f", "name": "new.gcode", "size": 20, "date": "2024-01-01T00:00:00"}
        ]));
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["0:/gcodes/new.gcode", "0:/gcodes/old.gcode"]);
    }
}
