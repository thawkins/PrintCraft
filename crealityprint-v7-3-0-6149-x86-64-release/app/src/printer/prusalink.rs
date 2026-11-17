//! PrusaLink: the local web API built into Prusa MK4/MK4S, Core One, MINI and XL
//! (and the MK3S with a Raspberry Pi). Prusa publishes it as an OpenAPI description.
//!
//! * Sign-in: HTTP Digest, user "maker" and the password shown on the printer's screen
//!   (Settings → Network → PrusaLink). Older set-ups use an `X-Api-Key` instead: when
//!   the printer refuses without offering Digest, a password that looks like a key is
//!   sent that way. A Digest password is never sent in the clear.
//! * Status: `GET /api/v1/status` (printer temperatures and state, job progress and times).
//! * Job: `GET /api/v1/job` (file name), `PUT /api/v1/job/{id}/pause|resume`,
//!   `DELETE /api/v1/job/{id}` to stop.
//! * Upload and print: `PUT /api/v1/files/{storage}/{name}` with `Print-After-Upload: ?1`.

use super::digest::{self, Challenge};
use super::link::Worker;
use super::{json_number, url_encode, Command, FoundPrinter, JobState, PrinterStatus, RemoteFile};
use serde_json::Value;
use std::time::Duration;

pub const PORT: u16 = 80;
const POLL_SECONDS: f32 = 2.0;
/// A whole G-code file over the printer's (often slow) network link.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);
pub const DEFAULT_USER: &str = "maker";

/// An HTTP client that signs in when asked to.
pub struct Client {
    host: String,
    login: Option<(String, String)>,
    challenge: Option<Challenge>,
    nc: u32,
    /// The printer refused without a Digest challenge: send the password as an API key.
    api_key: bool,
    agent: ureq::Agent,
}

pub enum Reply {
    Json(Value),
    Empty,
}

impl Client {
    pub fn new(host: &str, login: Option<(String, String)>, timeout_s: u64) -> Self {
        Self {
            host: host.to_string(),
            login,
            challenge: None,
            nc: 0,
            api_key: false,
            agent: super::http_agent(Duration::from_secs(4), Duration::from_secs(timeout_s)),
        }
    }

    fn sign(&mut self, req: ureq::Request, method: &str, path: &str) -> ureq::Request {
        let Some((user, pass)) = &self.login else {
            return req;
        };
        if let Some(c) = &self.challenge {
            self.nc += 1;
            let cnonce = format!("{:08x}{:08x}", self.nc, std::process::id());
            let auth = digest::authorization(c, user, pass, method, path, self.nc, &cnonce);
            req.set("Authorization", &auth)
        } else if self.api_key {
            req.set("X-Api-Key", pass)
        } else {
            req
        }
    }

    /// Sends a request, answering one sign-in challenge if the printer asks.
    pub fn call(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<Reply, String> {
        self.call_within(method, path, headers, body, None)
    }

    /// `call` with its own time limit, for requests slower than the client's usual one
    /// (uploads). Uses the same sign-in, so a Digest nonce is never reused.
    pub fn call_within(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
        timeout: Option<Duration>,
    ) -> Result<Reply, String> {
        for attempt in 0..2 {
            let url = format!("http://{}{}", self.host, path);
            let mut req = self.agent.request(method, &url);
            if let Some(t) = timeout {
                req = req.timeout(t);
            }
            for (k, v) in headers {
                req = req.set(k, v);
            }
            req = self.sign(req, method, path);
            let result = match body {
                Some(b) => req.send_bytes(b),
                None => req.call(),
            };
            match result {
                Ok(resp) => {
                    let text = resp.into_string().map_err(|e| e.to_string())?;
                    return Ok(if text.trim().is_empty() {
                        Reply::Empty
                    } else {
                        serde_json::from_str(&text)
                            .map(Reply::Json)
                            .unwrap_or(Reply::Empty)
                    });
                }
                Err(ureq::Error::Status(401, resp)) if attempt == 0 => {
                    let header = resp.header("WWW-Authenticate").unwrap_or("").to_string();
                    if self.login.is_none() {
                        return Err("The printer needs its PrusaLink password.".into());
                    }
                    self.challenge = digest::parse_challenge(&header);
                    self.nc = 0;
                    if self.challenge.is_none() {
                        // No Digest offered: an older PrusaLink may take the key instead.
                        let key = self.login.as_ref().map_or("", |l| l.1.as_str());
                        if self.api_key || !looks_like_api_key(key) {
                            return Err("The printer refused the password.".into());
                        }
                        self.api_key = true;
                    }
                }
                Err(ureq::Error::Status(401, _)) => return Err("Wrong PrusaLink password.".into()),
                Err(ureq::Error::Status(code, _)) => {
                    return Err(format!("The printer answered {code}."))
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Wrong PrusaLink password.".into())
    }
}

/// Older PrusaLink API keys are one long word.
fn looks_like_api_key(pass: &str) -> bool {
    !pass.is_empty() && !pass.contains(' ') && pass.len() >= 16
}

/// Does this address look like a PrusaLink printer? A PrusaLink answers /api/version,
/// or refuses it with its own sign-in realm.
pub fn identify(host: &str) -> Option<FoundPrinter> {
    let agent = super::http_agent(Duration::from_millis(800), Duration::from_secs(3));
    let found = |name: String| FoundPrinter {
        host: host.to_string(),
        name,
        model: "Prusa printer (PrusaLink)".into(),
        api: Some(super::Api::PrusaLink),
        ..Default::default()
    };
    match agent.get(&format!("http://{host}/api/version")).call() {
        Ok(resp) => {
            let v: Value = serde_json::from_str(&resp.into_string().ok()?).ok()?;
            let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let looks_prusa = text.contains("PrusaLink") || v.get("printer").is_some();
            looks_prusa.then(|| {
                found(
                    v.get("hostname")
                        .and_then(|h| h.as_str())
                        .unwrap_or("Prusa printer")
                        .to_string(),
                )
            })
        }
        Err(ureq::Error::Status(401, resp)) => {
            let realm = resp.header("WWW-Authenticate").unwrap_or("").to_string();
            (realm.contains("Printer API") || realm.contains("PrusaLink"))
                .then(|| found(format!("Prusa printer at {host}")))
        }
        Err(_) => None,
    }
}

pub fn apply_status(s: &mut PrinterStatus, v: &Value) {
    let n = |obj: &str, key: &str| v.get(obj).and_then(|o| o.get(key)).and_then(json_number);
    if let Some(x) = n("printer", "temp_nozzle") {
        s.nozzle = Some(x as f32);
    }
    if let Some(x) = n("printer", "target_nozzle") {
        s.nozzle_target = Some(x as f32);
    }
    if let Some(x) = n("printer", "temp_bed") {
        s.bed = Some(x as f32);
    }
    if let Some(x) = n("printer", "target_bed") {
        s.bed_target = Some(x as f32);
    }
    let has_job = v.get("job").is_some_and(|j| !j.is_null());
    s.progress = n("job", "progress").map(|p| p as f32).filter(|_| has_job);
    s.elapsed_s = n("job", "time_printing")
        .map(|t| t as u64)
        .filter(|_| has_job);
    s.remaining_s = n("job", "time_remaining")
        .map(|t| t as u64)
        .filter(|_| has_job);
    if let Some(state) = v
        .get("printer")
        .and_then(|p| p.get("state"))
        .and_then(|x| x.as_str())
    {
        s.state = match state.to_ascii_uppercase().as_str() {
            "IDLE" | "READY" | "BUSY" => JobState::Idle,
            "PRINTING" => JobState::Printing,
            "PAUSED" => JobState::Paused,
            "FINISHED" => JobState::Finished,
            "STOPPED" => JobState::Stopped,
            "ERROR" | "ATTENTION" => JobState::Failed,
            _ => s.state,
        };
    }
}

/// A storage listing (`GET /api/v1/files/<storage>`): print files with their real names.
pub fn parse_files(v: &Value, storage: &str) -> Vec<RemoteFile> {
    let children = v
        .get("children")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    children
        .iter()
        .filter(|c| {
            c.get("type")
                .and_then(|t| t.as_str())
                .is_none_or(|t| t == "PRINT_FILE")
        })
        .filter_map(|c| {
            let name = c.get("name")?.as_str()?.to_string();
            let shown = c
                .get("display_name")
                .and_then(|d| d.as_str())
                .unwrap_or(&name)
                .to_string();
            Some(RemoteFile {
                name: shown,
                path: format!("/{storage}/{name}"),
                size: c.get("size").and_then(json_number).map(|x| x as u64),
                modified: c.get("m_timestamp").and_then(json_number).map(|x| x as i64),
            })
        })
        .collect()
}

fn job_id(v: &Value) -> Option<u64> {
    v.get("job")
        .and_then(|j| j.get("id"))
        .and_then(json_number)
        .map(|x| x as u64)
}

/// Picks the storage to upload to: the first one the printer says is available
/// (`usb` on Buddy printers, `local` on a Raspberry Pi set-up).
fn storage(client: &mut Client) -> String {
    if let Ok(Reply::Json(v)) = client.call("GET", "/api/v1/storage", &[], None) {
        if let Some(list) = v.get("storage_list").and_then(|l| l.as_array()) {
            for s in list {
                let available = s.get("available").and_then(|a| a.as_bool()).unwrap_or(true);
                let read_only = s
                    .get("read_only")
                    .and_then(|a| a.as_bool())
                    .unwrap_or(false);
                if let Some(p) = s
                    .get("path")
                    .and_then(|p| p.as_str())
                    .filter(|_| available && !read_only)
                {
                    return p.trim_matches('/').to_string();
                }
            }
        }
    }
    "usb".into()
}

pub fn run(w: &Worker) {
    let login = w
        .printer
        .login
        .clone()
        .or_else(|| Some((DEFAULT_USER.into(), String::new())));
    let mut client = Client::new(&w.host, login, 10);
    let mut last_id: Option<u64> = None;
    let mut was_connected = false;
    let mut last_error = String::new();
    while !w.stopped() {
        while let Ok(cmd) = w.rx.try_recv() {
            let result = match cmd {
                Command::UploadAndPrint { name, gcode } => w.sending(&name, || {
                    let store = storage(&mut client);
                    client
                        .call_within(
                            "PUT",
                            &format!("/api/v1/files/{store}/{name}"),
                            &[
                                ("Content-Type", "text/x.gcode"),
                                ("Print-After-Upload", "?1"),
                                ("Overwrite", "?1"),
                            ],
                            Some(gcode.as_bytes()),
                            Some(UPLOAD_TIMEOUT),
                        )
                        .map(|_| ())
                }),
                Command::PrintFile(path) => {
                    let path = if path.starts_with('/') {
                        path
                    } else {
                        format!("/{}/{path}", storage(&mut client))
                    };
                    client
                        .call(
                            "POST",
                            &format!("/api/v1/files{}", url_encode(&path)),
                            &[],
                            None,
                        )
                        .map(|_| ())
                }
                Command::RefreshFiles => {
                    let store = storage(&mut client);
                    match client.call("GET", &format!("/api/v1/files/{store}"), &[], None) {
                        Ok(Reply::Json(v)) => {
                            let files = parse_files(&v, &store);
                            w.update(|s| s.files = Some(files));
                            Ok(())
                        }
                        Ok(Reply::Empty) => {
                            w.update(|s| s.files = Some(Vec::new()));
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                Command::RefreshHistory => Ok(()),
                // The app adds (or leaves out) its own levelling step in the file.
                Command::SetCalibration(on) => {
                    w.update(|s| s.calibrate = Some(on));
                    Ok(())
                }
                Command::SetNozzle(_)
                | Command::SetBed(_)
                | Command::SetFan(_)
                | Command::SetSpeed(_)
                | Command::Jog { .. }
                | Command::Home => {
                    Err("PrusaLink doesn't offer this; use the printer's screen.".to_string())
                }
                Command::Pause | Command::Resume | Command::Stop => match last_id {
                    None => Err("No print is running.".to_string()),
                    Some(id) => {
                        let (method, path) = match cmd {
                            Command::Pause => ("PUT", format!("/api/v1/job/{id}/pause")),
                            Command::Resume => ("PUT", format!("/api/v1/job/{id}/resume")),
                            _ => ("DELETE", format!("/api/v1/job/{id}")),
                        };
                        client.call(method, &path, &[], None).map(|_| ())
                    }
                },
            };
            if let Err(e) = result {
                w.update(|s| s.note(format!("The printer refused: {e}")));
            }
        }
        match client.call("GET", "/api/v1/status", &[], None) {
            Ok(Reply::Json(v)) => {
                last_id = job_id(&v);
                let file =
                    last_id.and_then(|_| match client.call("GET", "/api/v1/job", &[], None) {
                        Ok(Reply::Json(j)) => j.get("file").map(|f| {
                            let s = |k: &str| f.get(k).and_then(|x| x.as_str()).unwrap_or("");
                            let shown = if s("display_name").is_empty() {
                                s("name")
                            } else {
                                s("display_name")
                            };
                            let path = format!("{}/{}", s("path").trim_end_matches('/'), s("name"));
                            (shown.to_string(), path)
                        }),
                        _ => None,
                    });
                let (file, file_path) = match file {
                    Some((a, b)) => (Some(a), Some(b)),
                    None => (None, None),
                };
                if !was_connected {
                    w.update(|s| s.note("Connected."));
                }
                was_connected = true;
                last_error.clear();
                w.update(|s| {
                    s.connected = true;
                    apply_status(&mut s.status, &v);
                    s.status.file = file;
                    s.status.file_path = file_path;
                });
            }
            Ok(Reply::Empty) => {}
            Err(e) => {
                if e != last_error {
                    w.update(|s| s.note(e.clone()));
                    last_error = e;
                }
                was_connected = false;
                w.update(|s| s.connected = false);
            }
        }
        w.pause(POLL_SECONDS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A pretend printer: answers each request with `respond(request head)`, and sends
    /// every request head back down the channel.
    fn pretend_printer(
        respond: fn(&str) -> &'static str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for conn in listener.incoming().take(10) {
                let Ok(mut s) = conn else { continue };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let reply = respond(&head);
                let _ = tx.send(head);
                let _ = s.write_all(reply.as_bytes());
            }
        });
        (host, rx)
    }

    const LONG_PASSWORD: &str = "abcdefghijklmnopqrstuvwxyz";

    #[test]
    fn a_digest_password_is_never_sent_as_an_api_key() {
        let (host, rx) = pretend_printer(|_| {
            "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"Printer API\", nonce=\"n\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        });
        let mut c = Client::new(&host, Some(("maker".into(), LONG_PASSWORD.into())), 5);
        let _ = c.call("GET", "/api/v1/status", &[], None);
        let heads: Vec<String> = rx.try_iter().collect();
        assert!(!heads.is_empty());
        for h in &heads {
            assert!(
                !h.contains(LONG_PASSWORD),
                "password sent in the clear:\n{h}"
            );
        }
    }

    #[test]
    fn an_api_key_is_sent_only_when_the_printer_offers_no_digest() {
        let (host, rx) = pretend_printer(|head| {
            if head
                .to_ascii_lowercase()
                .contains("x-api-key: abcdefghijklmnopqrstuvwxyz")
            {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            } else {
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            }
        });
        let mut c = Client::new(&host, Some(("maker".into(), LONG_PASSWORD.into())), 5);
        assert!(c.call("GET", "/api/version", &[], None).is_ok());
        let heads: Vec<String> = rx.try_iter().collect();
        assert!(!heads[0].contains(LONG_PASSWORD), "first try is unsigned");
    }

    #[test]
    fn reads_prusalink_status() {
        let mut s = PrinterStatus::default();
        apply_status(
            &mut s,
            &json!({"printer": {"state": "PRINTING", "temp_nozzle": 214.8, "target_nozzle": 215, "temp_bed": 60.1, "target_bed": 60},
                    "job": {"id": 42, "progress": 37.0, "time_remaining": 1800, "time_printing": 900}}),
        );
        assert_eq!(s.state, JobState::Printing);
        assert_eq!(s.progress, Some(37.0));
        assert_eq!((s.elapsed_s, s.remaining_s), (Some(900), Some(1800)));
        apply_status(&mut s, &json!({"printer": {"state": "IDLE"}}));
        assert_eq!(s.state, JobState::Idle);
        assert_eq!(s.progress, None);
    }
}
