//! End-to-end checks against pretend printers that live on this computer only (127.0.0.1).
//! Each test is skipped when its port is already taken on this machine.

use super::link::PrinterLink;
use super::{Api, Command, FoundPrinter, JobState};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tungstenite::Message;

fn printer(api: Api) -> FoundPrinter {
    FoundPrinter {
        host: "127.0.0.1".into(),
        name: "pretend".into(),
        model: String::new(),
        api: Some(api),
        ..Default::default()
    }
}

fn wait_for(link: &PrinterLink, what: impl Fn(&super::link::LinkState) -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < until {
        if what(&link.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn talks_to_a_pretend_creality_printer() {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", super::creality::WS_PORT)) else {
        return;
    };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(s).unwrap();
        let status = json!({"state": 1, "printProgress": 37, "nozzleTemp": "209.5", "targetNozzleTemp": 210,
                            "printLeftTime": 1200, "hostname": "pretend-ke"});
        ws.send(Message::text(status.to_string())).unwrap();
        ws.get_ref()
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        while let Ok(msg) = ws.read() {
            if let Message::Text(t) = msg {
                if t.as_str().contains("pause") {
                    let _ = tx.send(t.as_str().to_string());
                    break;
                }
            }
        }
    });
    let link = PrinterLink::connect(printer(Api::CrealityLan), Arc::new(|| {}));
    assert!(
        wait_for(&link, |s| s.connected && s.status.progress == Some(37.0)),
        "no status arrived"
    );
    let st = link.state().status;
    assert_eq!(st.state, JobState::Printing);
    assert_eq!(st.hostname.as_deref(), Some("pretend-ke"));
    assert_eq!(st.remaining_s, Some(1200));
    link.send(Command::Pause);
    let sent = rx
        .recv_timeout(Duration::from_secs(8))
        .expect("pause never reached the printer");
    assert!(sent.contains("\"pause\":1"), "{sent}");
}

/// Reads one HTTP request: (request line, body).
fn read_request(s: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Some((
        head.lines().next().unwrap_or("").to_string(),
        buf[head_end..].to_vec(),
    ))
}

#[test]
fn talks_to_a_pretend_klipper_printer() {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", super::moonraker::PORT)) else {
        return;
    };
    let (tx, rx) = mpsc::channel::<(String, String)>();
    std::thread::spawn(move || {
        for conn in listener.incoming().take(200) {
            let Ok(mut s) = conn else { continue };
            s.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let Some((line, body)) = read_request(&mut s) else {
                continue;
            };
            let reply = if line.starts_with("GET /printer/objects/query") {
                json!({"result": {"status": {
                    "extruder": {"temperature": 210.0, "target": 210.0},
                    "heater_bed": {"temperature": 60.0, "target": 60.0},
                    "virtual_sdcard": {"progress": 0.5},
                    "print_stats": {"state": "printing", "filename": "cube.gcode", "print_duration": 300.0}
                }}})
            } else {
                let _ = tx.send((line.clone(), String::from_utf8_lossy(&body).to_string()));
                json!({"result": "ok"})
            };
            let text = reply.to_string();
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    let link = PrinterLink::connect(printer(Api::Moonraker), Arc::new(|| {}));
    assert!(
        wait_for(&link, |s| s.connected && s.status.progress == Some(50.0)),
        "no status arrived"
    );
    assert_eq!(link.state().status.remaining_s, Some(300));
    link.send(Command::UploadAndPrint {
        name: "cube.gcode".into(),
        gcode: Arc::new("G28\n".into()),
    });
    let (line, body) = rx
        .recv_timeout(Duration::from_secs(8))
        .expect("upload never arrived");
    assert!(line.starts_with("POST /server/files/upload"), "{line}");
    assert!(body.contains("filename=\"cube.gcode\"") && body.contains("G28"));
    assert!(body.contains("name=\"print\"\r\n\r\ntrue"));
}

/// Reads one HTTP request: (request line, headers, body).
fn read_full_request(s: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let line = head.lines().next().unwrap_or("").to_string();
    Some((line, head, buf[head_end..].to_vec()))
}

#[test]
fn talks_to_a_pretend_prusalink_printer_with_a_password() {
    use super::digest;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<(String, String, String)>();
    std::thread::spawn(move || {
        let challenge = digest::Challenge {
            realm: "Printer API".into(),
            nonce: "n0nce".into(),
            opaque: None,
            qop_auth: false,
        };
        for conn in listener.incoming().take(300) {
            let Ok(mut s) = conn else { continue };
            s.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let Some((line, head, body)) = read_full_request(&mut s) else {
                continue;
            };
            let mut parts = line.split_whitespace();
            let (method, uri) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let expected = digest::authorization(&challenge, "maker", "secret", method, uri, 0, "");
            let signed = head.lines().any(|l| {
                l.split_once(':').is_some_and(|(k, v)| {
                    k.eq_ignore_ascii_case("authorization") && v.trim() == expected
                })
            });
            let (status, text) = if !signed {
                ("401 Unauthorized", String::new())
            } else if uri == "/api/v1/status" {
                let v = json!({"printer": {"state": "PRINTING", "temp_nozzle": 215.0, "target_nozzle": 215,
                                           "temp_bed": 60.0, "target_bed": 60},
                               "job": {"id": 42, "progress": 61.0, "time_remaining": 600, "time_printing": 940}});
                ("200 OK", v.to_string())
            } else if uri == "/api/v1/job" {
                let v =
                    json!({"id": 42, "file": {"name": "CUBE~1.GCO", "display_name": "cube.gcode"}});
                ("200 OK", v.to_string())
            } else if uri == "/api/v1/storage" {
                let v = json!({"storage_list": [{"path": "/usb", "available": true, "read_only": false}]});
                ("200 OK", v.to_string())
            } else {
                let body = String::from_utf8_lossy(&body).to_string();
                let _ = tx.send((format!("{method} {uri}"), head.clone(), body));
                ("204 No Content", String::new())
            };
            let auth = if signed {
                String::new()
            } else {
                "WWW-Authenticate: Digest realm=\"Printer API\", nonce=\"n0nce\"\r\n".to_string()
            };
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    let printer = FoundPrinter {
        host: format!("127.0.0.1:{port}"),
        api: Some(Api::PrusaLink),
        login: Some(("maker".into(), "secret".into())),
        ..Default::default()
    };
    let link = PrinterLink::connect(printer, Arc::new(|| {}));
    assert!(
        wait_for(&link, |s| s.connected && s.status.progress == Some(61.0)),
        "no status: {:?}",
        link.state().log
    );
    assert_eq!(link.state().status.file.as_deref(), Some("cube.gcode"));
    link.send(Command::UploadAndPrint {
        name: "cube.gcode".into(),
        gcode: Arc::new("G28\n".into()),
    });
    let (req, head, body) = rx
        .recv_timeout(Duration::from_secs(8))
        .expect("upload never arrived");
    assert_eq!(req, "PUT /api/v1/files/usb/cube.gcode");
    assert!(
        head.to_ascii_lowercase().contains("print-after-upload: ?1"),
        "{head}"
    );
    assert_eq!(body, "G28\n");
    link.send(Command::Pause);
    let (req, _, _) = rx
        .recv_timeout(Duration::from_secs(8))
        .expect("pause never arrived");
    assert_eq!(req, "PUT /api/v1/job/42/pause");
}
