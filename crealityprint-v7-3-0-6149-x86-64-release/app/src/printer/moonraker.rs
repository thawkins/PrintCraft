//! Moonraker, Klipper's documented web API (port 7125). Many Ender-3 V3 KE owners
//! install it; it is the best-documented way to talk to these printers.

use super::link::{ContactLog, Worker};
use super::{
    bed_gcode, fan_gcode, jog_gcode, json_number, multipart, nozzle_gcode, url_encode, Command,
    FoundPrinter, HistoryEntry, JobState, PrinterStatus, RemoteFile,
};
use serde_json::Value;
use std::time::Duration;

pub const PORT: u16 = 7125;
const POLL_SECONDS: f32 = 1.0;
const QUERY: &str = "/printer/objects/query?print_stats&virtual_sdcard&extruder&heater_bed&display_status&fan&gcode_move";

/// Runs G-code through Moonraker's documented script endpoint.
fn gcode(host: &str, script: &str) -> Result<(), String> {
    post(
        host,
        &format!("/printer/gcode/script?script={}", url_encode(script)),
    )
}

fn list_files(host: &str) -> Result<Vec<RemoteFile>, String> {
    let v = get_json(host, "/server/files/list?root=gcodes")?;
    let list = v
        .get("result")
        .and_then(|r| r.as_array())
        .ok_or("unexpected answer")?;
    let mut files: Vec<RemoteFile> = list
        .iter()
        .filter_map(|f| {
            let path = f
                .get("path")
                .or_else(|| f.get("filename"))?
                .as_str()?
                .to_string();
            Some(RemoteFile {
                name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                size: f.get("size").and_then(json_number).map(|x| x as u64),
                modified: f.get("modified").and_then(json_number).map(|x| x as i64),
                path,
            })
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.modified));
    Ok(files)
}

fn list_history(host: &str) -> Result<Vec<HistoryEntry>, String> {
    let v = get_json(host, "/server/history/list?limit=30&order=desc")?;
    let jobs = v
        .get("result")
        .and_then(|r| r.get("jobs"))
        .and_then(|j| j.as_array())
        .ok_or("unexpected answer")?;
    Ok(jobs
        .iter()
        .map(|j| HistoryEntry {
            name: j
                .get("filename")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            outcome: j
                .get("status")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .replace('_', " "),
            ended: j.get("end_time").and_then(json_number).map(|x| x as i64),
            duration_s: j
                .get("total_duration")
                .and_then(json_number)
                .map(|x| x as u64),
        })
        .collect())
}

fn agent(timeout_s: u64) -> ureq::Agent {
    super::http_agent(Duration::from_secs(3), Duration::from_secs(timeout_s))
}

fn get_json(host: &str, path: &str) -> Result<Value, String> {
    let resp = agent(5)
        .get(&format!("http://{host}:{PORT}{path}"))
        .call()
        .map_err(|e| e.to_string())?;
    let text = resp.into_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn post(host: &str, path: &str) -> Result<(), String> {
    agent(10)
        .post(&format!("http://{host}:{PORT}{path}"))
        .call()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Folds a `/printer/objects/query` result (`result.status`) into what we know.
pub fn apply_status(s: &mut PrinterStatus, status: &Value) {
    let n = |obj: &str, key: &str| {
        status
            .get(obj)
            .and_then(|o| o.get(key))
            .and_then(json_number)
    };
    if let Some(x) = n("extruder", "temperature") {
        s.nozzle = Some(x as f32);
    }
    if let Some(x) = n("extruder", "target") {
        s.nozzle_target = Some(x as f32);
    }
    if let Some(x) = n("heater_bed", "temperature") {
        s.bed = Some(x as f32);
    }
    if let Some(x) = n("heater_bed", "target") {
        s.bed_target = Some(x as f32);
    }
    let progress = n("display_status", "progress").or_else(|| n("virtual_sdcard", "progress"));
    if let Some(p) = progress {
        s.progress = Some((p as f32 * 100.0).clamp(0.0, 100.0));
    }
    if let Some(d) = n("print_stats", "print_duration") {
        s.elapsed_s = Some(d as u64);
        // Same simple projection most Klipper front ends use.
        s.remaining_s = progress
            .filter(|p| *p > 0.01)
            .map(|p| (d / p - d).max(0.0) as u64);
    }
    if let Some(x) = n("fan", "speed") {
        s.fan_percent = Some((x as f32 * 100.0).clamp(0.0, 100.0));
    }
    if let Some(x) = n("gcode_move", "speed_factor") {
        s.speed_percent = Some(x as f32 * 100.0);
    }
    if let Some(ps) = status.get("print_stats") {
        if let Some(f) = ps.get("filename").and_then(|x| x.as_str()) {
            s.file = (!f.is_empty()).then(|| f.to_string());
            s.file_path = s.file.clone();
        }
        if let Some(info) = ps.get("info") {
            s.layer = info
                .get("current_layer")
                .and_then(json_number)
                .map(|x| x as u32);
            s.total_layers = info
                .get("total_layer")
                .and_then(json_number)
                .map(|x| x as u32);
        }
        if let Some(state) = ps.get("state").and_then(|x| x.as_str()) {
            s.state = match state {
                "standby" => JobState::Idle,
                "printing" => JobState::Printing,
                "paused" => JobState::Paused,
                "complete" => JobState::Finished,
                "error" => JobState::Failed,
                "cancelled" => JobState::Stopped,
                _ => s.state,
            };
        }
    }
}

pub fn identify(host: &str) -> Option<FoundPrinter> {
    let info = get_json(host, "/printer/info").ok()?;
    let r = info.get("result")?;
    let hostname = r
        .get("hostname")
        .and_then(|x| x.as_str())
        .unwrap_or(host)
        .to_string();
    Some(FoundPrinter {
        host: host.to_string(),
        name: hostname,
        model: "Klipper printer".into(),
        api: Some(super::Api::Moonraker),
        ..Default::default()
    })
}

pub fn upload_and_print(host: &str, name: &str, data: &[u8]) -> Result<(), String> {
    let (content_type, body) = multipart(&[("file", Some(name), data), ("print", None, b"true")]);
    agent(300)
        .post(&format!("http://{host}:{PORT}/server/files/upload"))
        .set("Content-Type", &content_type)
        .send_bytes(&body)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub fn run(w: &Worker) {
    let mut contact = ContactLog::default();
    while !w.stopped() {
        while let Ok(cmd) = w.rx.try_recv() {
            let result = match cmd {
                Command::UploadAndPrint { name, gcode } => {
                    w.sending(&name, || upload_and_print(&w.host, &name, gcode.as_bytes()))
                }
                Command::PrintFile(path) => post(
                    &w.host,
                    &format!("/printer/print/start?filename={}", url_encode(&path)),
                ),
                Command::Pause => post(&w.host, "/printer/print/pause"),
                Command::Resume => post(&w.host, "/printer/print/resume"),
                Command::Stop => post(&w.host, "/printer/print/cancel"),
                Command::SetNozzle(t) => gcode(&w.host, &nozzle_gcode(t)),
                Command::SetBed(t) => gcode(&w.host, &bed_gcode(t)),
                Command::SetFan(p) => gcode(&w.host, &fan_gcode(p)),
                Command::SetSpeed(p) => gcode(&w.host, &format!("M220 S{p}")),
                Command::Jog { axis, mm } => gcode(&w.host, &jog_gcode(axis, mm).join("\n")),
                Command::Home => gcode(&w.host, "G28"),
                Command::RefreshFiles => {
                    list_files(&w.host).map(|f| w.update(|s| s.files = Some(f)))
                }
                Command::RefreshHistory => {
                    list_history(&w.host).map(|h| w.update(|s| s.history = Some(h)))
                }
                // The app adds (or leaves out) its own levelling step in the file.
                Command::SetCalibration(on) => {
                    w.update(|s| s.calibrate = Some(on));
                    Ok(())
                }
            };
            if let Err(e) = result {
                w.update(|s| s.note(format!("The printer refused: {e}")));
            }
        }
        match get_json(&w.host, QUERY) {
            Ok(v) => {
                contact.reached(w);
                w.update(|s| {
                    if let Some(st) = v.get("result").and_then(|r| r.get("status")) {
                        apply_status(&mut s.status, st);
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
    use serde_json::json;

    #[test]
    fn maps_every_klipper_print_state() {
        for (klipper, ours) in [
            ("standby", JobState::Idle),
            ("printing", JobState::Printing),
            ("paused", JobState::Paused),
            ("complete", JobState::Finished),
            ("error", JobState::Failed),
            ("cancelled", JobState::Stopped),
        ] {
            let mut s = PrinterStatus::default();
            apply_status(&mut s, &json!({"print_stats": {"state": klipper}}));
            assert_eq!(s.state, ours, "{klipper}");
        }
        let mut s = PrinterStatus {
            state: JobState::Printing,
            ..PrinterStatus::default()
        };
        apply_status(&mut s, &json!({"print_stats": {"state": "something new"}}));
        assert_eq!(
            s.state,
            JobState::Printing,
            "an unknown state keeps the last one"
        );
    }

    #[test]
    fn reads_fan_speed_factor_and_an_empty_file_name() {
        let mut s = PrinterStatus::default();
        apply_status(
            &mut s,
            &json!({"fan": {"speed": 0.5}, "gcode_move": {"speed_factor": 1.25},
                    "print_stats": {"filename": ""}}),
        );
        assert_eq!(s.fan_percent, Some(50.0));
        assert_eq!(s.speed_percent, Some(125.0));
        assert_eq!(s.file, None);
    }

    #[test]
    fn reads_klipper_status() {
        let mut s = PrinterStatus::default();
        apply_status(
            &mut s,
            &json!({
                "extruder": {"temperature": 209.8, "target": 210.0},
                "heater_bed": {"temperature": 60.1, "target": 60.0},
                "virtual_sdcard": {"progress": 0.25},
                "print_stats": {"state": "printing", "filename": "cube.gcode", "print_duration": 600.0,
                                 "info": {"current_layer": 10, "total_layer": 100}}
            }),
        );
        assert_eq!(s.state, JobState::Printing);
        assert_eq!(s.progress, Some(25.0));
        assert_eq!(s.remaining_s, Some(1800));
        assert_eq!(s.layer, Some(10));
    }
}
