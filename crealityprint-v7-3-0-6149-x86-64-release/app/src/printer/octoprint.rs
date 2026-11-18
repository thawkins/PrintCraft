//! OctoPrint: the web host many people run on a Raspberry Pi next to a USB printer. Its
//! REST API is documented by the OctoPrint project.
//!
//! * Sign-in: an API key (OctoPrint's Settings, "Application Keys" or "API"), sent as
//!   `X-Api-Key` on every request. It is never saved.
//! * Status: `GET /api/printer` (temperatures and state flags; 409 while OctoPrint isn't
//!   connected to the printer) and `GET /api/job` (file, progress and times).
//! * Upload and print: `POST /api/files/local`, multipart `file` plus `select=true` and
//!   `print=true`. A stored file: `POST /api/files/local/<path>` with
//!   `{"command": "select", "print": true}`.
//! * Pause / resume / stop: `POST /api/job` with `{"command": "pause", "action": ...}`
//!   or `{"command": "cancel"}`.
//! * Temperatures: `POST /api/printer/tool` and `/api/printer/bed`. Other controls are
//!   G-code through `POST /api/printer/command`.
//! * Files: `GET /api/files?recursive=true`.
//! * Camera: OctoPi's MJPEG stream at `/webcam/?action=stream`, which the camera view
//!   already tries.

use super::link::{ContactLog, Worker};
use super::{
    fan_gcode, jog_gcode, json_number, multipart, url_encode, Command, FoundPrinter, JobState,
    PrinterStatus, RemoteFile,
};
use serde_json::{json, Value};
use std::time::Duration;

pub const PORT: u16 = 80;
const POLL_SECONDS: f32 = 2.0;
/// A whole G-code file over the network to the Pi.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Requests to one OctoPrint server with its API key.
struct Client {
    host: String,
    key: String,
    agent: ureq::Agent,
}

impl Client {
    fn new(host: &str, key: &str) -> Self {
        Self {
            host: host.to_string(),
            key: key.to_string(),
            agent: super::http_agent(Duration::from_secs(4), Duration::from_secs(10)),
        }
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        self.agent
            .request(method, &format!("http://{}{path}", self.host))
            .set("X-Api-Key", &self.key)
    }

    /// The reply's JSON (Null for an empty reply), or what went wrong in words.
    fn answer(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
        match result {
            Ok(resp) => {
                let text = resp.into_string().map_err(|e| e.to_string())?;
                Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
            }
            Err(ureq::Error::Status(401 | 403, _)) => {
                Err("OctoPrint refused the API key. Check it under OctoPrint's settings.".into())
            }
            Err(ureq::Error::Status(409, _)) => {
                Err("OctoPrint isn't connected to the printer.".into())
            }
            Err(ureq::Error::Status(code, _)) => Err(format!("OctoPrint answered {code}.")),
            Err(e) => Err(e.to_string()),
        }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        Self::answer(self.request("GET", path).call())
    }

    fn post_json(&self, path: &str, body: &Value) -> Result<(), String> {
        Self::answer(
            self.request("POST", path)
                .set("Content-Type", "application/json")
                .send_string(&body.to_string()),
        )
        .map(|_| ())
    }

    /// Runs G-code lines on the printer.
    fn gcode(&self, lines: &[String]) -> Result<(), String> {
        self.post_json("/api/printer/command", &json!({ "commands": lines }))
    }

    fn upload_and_print(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let (content_type, body) = multipart(&[
            ("file", Some(name), data),
            ("select", None, b"true"),
            ("print", None, b"true"),
        ]);
        Self::answer(
            self.request("POST", "/api/files/local")
                .timeout(UPLOAD_TIMEOUT)
                .set("Content-Type", &content_type)
                .send_bytes(&body),
        )
        .map(|_| ())
    }
}

/// Does this address look like an OctoPrint server? It answers `/api/version` with
/// "OctoPrint" in its text, or, refusing that without a key, shows "OctoPrint" on its
/// page (the sign-in page).
pub fn identify(host: &str) -> Option<FoundPrinter> {
    let agent = super::http_agent(Duration::from_millis(800), Duration::from_secs(3));
    let is_octoprint = match agent.get(&format!("http://{host}/api/version")).call() {
        Ok(resp) => resp
            .into_string()
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get("text")?.as_str().map(|t| t.contains("OctoPrint")))
            .unwrap_or(false),
        Err(ureq::Error::Status(401 | 403, _)) => agent
            .get(&format!("http://{host}/"))
            .call()
            .ok()
            .and_then(|r| r.into_string().ok())
            .is_some_and(|page| page.contains("OctoPrint")),
        Err(_) => false,
    };
    is_octoprint.then(|| FoundPrinter {
        host: host.to_string(),
        name: format!("OctoPrint at {host}"),
        model: "OctoPrint".into(),
        api: Some(super::Api::OctoPrint),
        ..Default::default()
    })
}

/// Folds `GET /api/printer` into what we know.
pub fn apply_printer(s: &mut PrinterStatus, v: &Value) {
    let t = |heater: &str, key: &str| {
        v.get("temperature")
            .and_then(|x| x.get(heater))
            .and_then(|x| x.get(key))
            .and_then(json_number)
            .map(|x| x as f32)
    };
    s.nozzle = t("tool0", "actual").or(s.nozzle);
    s.nozzle_target = t("tool0", "target").or(s.nozzle_target);
    s.bed = t("bed", "actual").or(s.bed);
    s.bed_target = t("bed", "target").or(s.bed_target);
    if let Some(flags) = v.get("state").and_then(|x| x.get("flags")) {
        let on = |k: &str| flags.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        s.state = if on("error") || on("closedOrError") {
            JobState::Failed
        } else if on("cancelling") {
            JobState::Stopped
        } else if on("paused") || on("pausing") {
            JobState::Paused
        } else if on("printing") {
            JobState::Printing
        } else if on("operational") || on("ready") {
            JobState::Idle
        } else {
            s.state
        };
    }
}

/// Folds `GET /api/job` into what we know.
pub fn apply_job(s: &mut PrinterStatus, v: &Value) {
    let file = v.get("job").and_then(|j| j.get("file"));
    let text = |k: &str| {
        file.and_then(|f| f.get(k))
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    s.file = text("display").or_else(|| text("name"));
    s.file_path = text("path");
    let p = |k: &str| {
        v.get("progress")
            .and_then(|x| x.get(k))
            .and_then(json_number)
    };
    s.progress = p("completion").map(|x| x as f32);
    s.elapsed_s = p("printTime").map(|x| x as u64);
    s.remaining_s = p("printTimeLeft").map(|x| x as u64);
}

/// A `GET /api/files?recursive=true` listing: G-code files in every folder, newest first.
pub fn parse_files(v: &Value) -> Vec<RemoteFile> {
    fn walk(list: &[Value], out: &mut Vec<RemoteFile>) {
        for f in list {
            match f.get("type").and_then(|t| t.as_str()) {
                Some("folder") => {
                    if let Some(children) = f.get("children").and_then(|c| c.as_array()) {
                        walk(children, out);
                    }
                }
                Some("machinecode") => {
                    let s = |k: &str| f.get(k).and_then(|x| x.as_str()).unwrap_or("");
                    let path = if s("path").is_empty() {
                        s("name")
                    } else {
                        s("path")
                    };
                    let shown = if s("display").is_empty() {
                        s("name")
                    } else {
                        s("display")
                    };
                    out.push(RemoteFile {
                        name: shown.to_string(),
                        path: path.to_string(),
                        size: f.get("size").and_then(json_number).map(|x| x as u64),
                        modified: f.get("date").and_then(json_number).map(|x| x as i64),
                    });
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    if let Some(list) = v.get("files").and_then(|f| f.as_array()) {
        walk(list, &mut out);
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.modified));
    out
}

/// One command from the app.
fn run_command(w: &Worker, c: &Client, cmd: Command) -> Result<(), String> {
    match cmd {
        Command::UploadAndPrint { name, gcode } => {
            w.sending(&name, || c.upload_and_print(&name, gcode.as_bytes()))
        }
        Command::UploadBinaryAndPrint { .. } => {
            Err("OctoPrint takes plain G-code, not binary (.bgcode).".into())
        }
        Command::PrintFile(path) => c.post_json(
            &format!("/api/files/local/{}", url_encode(&path)),
            &json!({"command": "select", "print": true}),
        ),
        Command::Pause => c.post_json("/api/job", &json!({"command": "pause", "action": "pause"})),
        Command::Resume => {
            c.post_json("/api/job", &json!({"command": "pause", "action": "resume"}))
        }
        Command::Stop => c.post_json("/api/job", &json!({"command": "cancel"})),
        Command::SetNozzle(t) => c.post_json(
            "/api/printer/tool",
            &json!({"command": "target", "targets": {"tool0": t.round() as i64}}),
        ),
        Command::SetBed(t) => c.post_json(
            "/api/printer/bed",
            &json!({"command": "target", "target": t.round() as i64}),
        ),
        Command::SetFan(p) => c.gcode(&[fan_gcode(p)]),
        Command::SetSpeed(p) => c.gcode(&[format!("M220 S{p}")]),
        Command::Jog { axis, mm } => c.gcode(&jog_gcode(axis, mm)),
        Command::Home => c.gcode(&["G28".to_string()]),
        Command::RefreshFiles => c
            .get("/api/files?recursive=true")
            .map(|v| w.update(|s| s.files = Some(parse_files(&v)))),
        // OctoPrint keeps no print history through its API.
        Command::RefreshHistory => Ok(()),
        // The app adds (or leaves out) its own levelling step in the file.
        Command::SetCalibration(on) => {
            w.update(|s| s.calibrate = Some(on));
            Ok(())
        }
        Command::CancelObject(_) => {
            Err("OctoPrint can't cancel one object here; use its web page.".into())
        }
    }
}

pub fn run(w: &Worker) {
    let key = w.printer.login.as_ref().map(|l| l.1.trim().to_string());
    let Some(key) = key.filter(|k| !k.is_empty()) else {
        w.update(|s| {
            s.note("OctoPrint needs its API key: enter it next to the address, then connect again.")
        });
        return;
    };
    let client = Client::new(&w.host, &key);
    let mut contact = ContactLog::default();
    while !w.stopped() {
        while let Ok(cmd) = w.rx.try_recv() {
            if let Err(e) = run_command(w, &client, cmd) {
                w.update(|s| s.note(format!("The printer refused: {e}")));
            }
        }
        match client.get("/api/printer") {
            Ok(printer) => {
                let job = client.get("/api/job").ok();
                contact.reached(w);
                w.update(|s| {
                    apply_printer(&mut s.status, &printer);
                    if let Some(j) = &job {
                        apply_job(&mut s.status, j);
                    }
                });
            }
            Err(e) => contact.failed(w, &e),
        }
        w.pause(POLL_SECONDS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn test_octoprint_state_flags_map_to_job_states() {
        for (flag, want) in [
            ("printing", JobState::Printing),
            ("paused", JobState::Paused),
            ("pausing", JobState::Paused),
            ("cancelling", JobState::Stopped),
            ("error", JobState::Failed),
            ("closedOrError", JobState::Failed),
            ("operational", JobState::Idle),
        ] {
            let mut s = PrinterStatus::default();
            apply_printer(&mut s, &json!({"state": {"flags": {flag: true}}}));
            assert_eq!(s.state, want, "{flag}");
        }
        // Printing and paused together (as OctoPrint reports a pause): paused.
        let mut s = PrinterStatus::default();
        apply_printer(
            &mut s,
            &json!({"state": {"flags": {"printing": true, "paused": true, "operational": true}}}),
        );
        assert_eq!(s.state, JobState::Paused);
        // No job: nothing left over from the last one.
        apply_job(
            &mut s,
            &json!({"job": {"file": {"name": null}}, "progress": {}}),
        );
        assert_eq!((s.file, s.progress, s.remaining_s), (None, None, None));
    }

    #[test]
    fn test_octoprint_is_recognised_by_its_sign_in_page() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for conn in listener.incoming().take(4) {
                let Ok(mut s) = conn else { continue };
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let reply = if head.starts_with("GET /api/version") {
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else {
                    let page = "<html><head><title>OctoPrint Login</title></head></html>";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                        page.len()
                    )
                };
                let _ = s.write_all(reply.as_bytes());
            }
        });
        let found = identify(&host).expect("not recognised");
        assert_eq!(found.api, Some(super::super::Api::OctoPrint));
    }
}
