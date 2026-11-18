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
                    "print_stats": {"state": "printing", "filename": "cube.gcode", "print_duration": 300.0},
                    "exclude_object": {
                        "objects": [{"name": "LEFT_0"}, {"name": "RIGHT_1"}],
                        "excluded_objects": ["LEFT_0"],
                        "current_object": "RIGHT_1"
                    }
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
    // The objects in the file, and cancelling one.
    let objects = link.state().status.objects;
    assert_eq!(
        objects,
        vec![
            super::PrintObject {
                name: "LEFT_0".into(),
                cancelled: true
            },
            super::PrintObject {
                name: "RIGHT_1".into(),
                cancelled: false
            },
        ]
    );
    link.send(Command::CancelObject("RIGHT_1".into()));
    let (line, _) = rx
        .recv_timeout(Duration::from_secs(8))
        .expect("cancel never arrived");
    assert!(
        line.starts_with("POST /printer/gcode/script?script=EXCLUDE_OBJECT%20NAME%3DRIGHT_1 "),
        "{line}"
    );
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
    let (tx, rx) = mpsc::channel::<(String, String, Vec<u8>)>();
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
    assert_eq!(body, b"G28\n");
    link.send(Command::Pause);
    let (req, _, _) = rx
        .recv_timeout(Duration::from_secs(8))
        .expect("pause never arrived");
    assert_eq!(req, "PUT /api/v1/job/42/pause");
    // A binary G-code file goes up as it is, under its .bgcode name.
    let data = crate::bgcode::encode("G28\n", &crate::gcode::Estimate::default());
    link.send(Command::UploadBinaryAndPrint {
        name: "cube.bgcode".into(),
        data: Arc::new(data.clone()),
    });
    let (req, head, body) = rx
        .recv_timeout(Duration::from_secs(8))
        .expect("binary upload never arrived");
    assert_eq!(req, "PUT /api/v1/files/usb/cube.bgcode");
    let head = head.to_ascii_lowercase();
    assert!(head.contains("print-after-upload: ?1"), "{head}");
    assert!(
        head.contains("content-type: application/octet-stream"),
        "{head}"
    );
    assert_eq!(body, data);
}

/// A pretend Creality network printer at `ip`:9999 that tells every visitor its name.
/// Returns false when the address can't be used on this machine.
pub(crate) fn pretend_creality_named(ip: &str, hostname: &'static str) -> bool {
    let Ok(listener) = TcpListener::bind((ip, super::creality::WS_PORT)) else {
        return false;
    };
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || {
                // Plain "is the port open?" checks connect and leave: not WebSocket.
                let Ok(mut ws) = tungstenite::accept(s) else {
                    return;
                };
                let hello = json!({"hostname": hostname, "model": "Pretend KE"});
                let _ = ws.send(Message::text(hello.to_string()));
                let _ = ws.get_ref().set_read_timeout(Some(Duration::from_secs(3)));
                while ws.read().is_ok() {}
            });
        }
    });
    true
}

/// A saved Creality printer whose address changed: found again by its name, even when a
/// different printer now has its old address. Uses loopback addresses 127.0.0.x only.
#[test]
fn finds_a_creality_printer_that_moved_to_a_new_address() {
    use super::discovery::relocate;
    // The printer now lives at .62; another printer took its old address, .64.
    if !pretend_creality_named("127.0.0.62", "pretend-ke")
        || !pretend_creality_named("127.0.0.64", "other-ke")
    {
        return;
    }
    let saved = |host: &str, name: &str| FoundPrinter {
        host: host.into(),
        name: name.into(),
        model: "Pretend KE".into(),
        api: Some(Api::CrealityLan),
        ..Default::default()
    };
    let network: Vec<String> = (60..=66).map(|d| format!("127.0.0.{d}")).collect();
    // Still where it was saved: nothing to search.
    let here = relocate(&saved("127.0.0.62", "pretend-ke"), &[]).expect("found in place");
    assert_eq!(here.host, "127.0.0.62");
    // Its old address answers no more.
    let moved = relocate(&saved("127.0.0.66", "pretend-ke"), &network).expect("found");
    assert_eq!(moved.host, "127.0.0.62");
    assert_eq!(moved.name, "pretend-ke");
    // Another printer answers at its old address: not taken for it.
    let taken = relocate(&saved("127.0.0.64", "pretend-ke"), &network).expect("found");
    assert_eq!(taken.host, "127.0.0.62");
    // A printer that is nowhere on the network.
    assert!(relocate(&saved("127.0.0.66", "gone-ke"), &network).is_none());
    // A printer saved without a name of its own can't be recognised elsewhere.
    assert!(relocate(&saved("127.0.0.66", "Printer at 127.0.0.66"), &network).is_none());
}

/// A pretend printer running Repetier-Firmware on a pseudo serial port (Linux/macOS):
/// it names itself to M115, asks for one line again with `rs:2` and then only says
/// `wait` (no "ok"), as the RepRap wiki describes, and answers "ok" to everything else.
#[cfg(unix)]
#[test]
fn prints_on_a_pretend_repetier_printer_over_usb() {
    use serialport::{SerialPort, TTYPort};
    let Ok((mut board, port)) = TTYPort::pair() else {
        return;
    };
    let Some(path) = port.name() else { return };
    // The app opens the port by its name, like a real USB printer.
    drop(port);
    let (tx, rx) = mpsc::channel::<Vec<String>>();
    std::thread::spawn(move || {
        let mut got: Vec<String> = Vec::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 256];
        let mut asked_again = false;
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            match board.read(&mut chunk) {
                Ok(n) if n > 0 => buf.extend_from_slice(&chunk[..n]),
                // Timeouts, and errors while the app hasn't opened its end yet.
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
            while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                let raw: Vec<u8> = buf.drain(..=i).collect();
                let line = String::from_utf8_lossy(&raw).trim().to_string();
                let reply = if line == "M115" {
                    got.push(line);
                    "FIRMWARE_NAME:Repetier_1.0.3 FIRMWARE_URL:https://github.com/repetier/Repetier-Firmware/ PROTOCOL_VERSION:1.0 MACHINE_TYPE:Mendel EXTRUDER_COUNT:1 REPETIER_PROTOCOL:3\nok\n".to_string()
                } else if let Some(body) = line.strip_prefix('N') {
                    let (numbered, cs) = line.rsplit_once('*').unwrap();
                    let sum = numbered.bytes().fold(0u8, |a, b| a ^ b);
                    assert_eq!(cs.parse::<u8>().unwrap(), sum, "bad checksum: {line}");
                    let (n, cmd) = body.split_once(' ').unwrap();
                    let cmd = cmd.rsplit_once('*').unwrap().0.to_string();
                    if n == "2" && !asked_again {
                        asked_again = true;
                        "rs:2\nwait\n".to_string()
                    } else {
                        got.push(cmd);
                        "ok\n".to_string()
                    }
                } else {
                    let done = line == "M530 S0";
                    got.push(line.clone());
                    if done {
                        // Report what arrived, but stay connected and keep answering,
                        // like a real printer: closing this end at once could lose the
                        // last "ok" before the app reads it.
                        let _ = tx.send(got.clone());
                    }
                    if line == "M105" {
                        "ok T:209.0 /210.0 B:60.0 /60.0 @:0\n".to_string()
                    } else {
                        "ok\n".to_string()
                    }
                };
                let _ = board.write_all(reply.as_bytes());
            }
        }
    });
    let link = PrinterLink::connect(
        FoundPrinter {
            host: path,
            name: "pretend repetier".into(),
            api: Some(Api::Usb),
            baud: Some(115_200),
            ..Default::default()
        },
        Arc::new(|| {}),
    );
    assert!(
        wait_for(&link, |s| s.status.firmware.as_deref()
            == Some("Repetier 1.0.3")),
        "the firmware never named itself"
    );
    link.send(Command::UploadAndPrint {
        name: "cube.gcode".into(),
        gcode: Arc::new(";LAYER:0\nG28\nG1 X1\n;LAYER:1\nG1 X2 ; last\n".into()),
    });
    let got = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("the print never finished");
    let pos = |cmd: &str| {
        got.iter()
            .position(|l| l == cmd)
            .unwrap_or_else(|| panic!("{cmd} missing from {got:?}"))
    };
    assert!(pos("M530 S1 L2") < pos("M531 cube.gcode"));
    assert!(pos("M531 cube.gcode") < pos("G28"));
    // Line 2 arrived after it was asked for again, then the job carried on.
    assert!(pos("G28") < pos("G1 X1") && pos("G1 X1") < pos("G1 X2"));
    assert!(pos("G1 X2") < pos("M532 X100.0 L2"));
    assert!(!got.iter().any(|l| l.starts_with("M155")), "{got:?}");
    assert!(
        wait_for(&link, |s| s.status.state == JobState::Finished),
        "not reported finished"
    );
}
