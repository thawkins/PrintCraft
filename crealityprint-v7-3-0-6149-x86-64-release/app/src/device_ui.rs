//! The "Printer" page, laid out like the printer device screen the person shared:
//! a tab with the printer's name and address, the camera and the current print on the
//! left, print settings (temperatures, model fan, printing speed) and tabs for XYZ
//! control, files on the printer, records and timelapse on the right. Also finds and
//! connects printers, and asks "Print again?" when a print finishes.

use crate::printer::calibration::with_calibration;
use crate::printer::camera::{CameraFeed, CameraState};
use crate::printer::discovery::{self, Scan};
use crate::printer::known::KnownPrinters;
use crate::printer::layer_info::with_layer_info;
use crate::printer::link::{LinkState, PrinterLink};
use crate::printer::{prusalink, usb, Api, HistoryEntry};
use crate::printer::{Command, FoundPrinter, JobState, PrinterStatus, Wake};
use crate::theme;
use crate::widgets::card;
use eframe::egui::{self, Color32, RichText, Sense, Ui};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// A sliced plate ready to send.
#[derive(Clone)]
pub struct PrintJob {
    pub name: String,
    pub gcode: Arc<String>,
    pub layers: usize,
    /// The picture of the part written into the print file.
    pub picture: Option<Arc<image::RgbaImage>>,
}

enum Pending {
    Print(PrintJob),
    PrintFile { name: String, path: String },
    Stop,
}

/// What "Print again" would print.
#[derive(Clone)]
enum Again {
    /// The plate sliced and sent from this app (sent again).
    Job(PrintJob),
    /// A file already on the printer.
    File { name: String, path: String },
}

impl Again {
    fn name(&self) -> &str {
        match self {
            Self::Job(j) => &j.name,
            Self::File { name, .. } => name,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Xyz,
    Files,
    Records,
    Timelapse,
}

/// The speed buttons. The screen shows no number for Silent; 50 % is an estimate.
pub const SPEED_MODES: [(&str, u32); 4] = [
    ("Silent", 50),
    ("Stable 50%", 50),
    ("Standard 100%", 100),
    ("Ultrafast 125%", 125),
];
const JOG_STEPS: [f32; 3] = [1.0, 10.0, 100.0];

pub struct DevicePage {
    /// Measure the plate before each print sent from here (on by default).
    calibrate: bool,
    scan: Option<Scan>,
    scan_error: Option<String>,
    found: Vec<FoundPrinter>,
    scanned_once: bool,
    manual_host: String,
    link: Option<PrinterLink>,
    camera: Option<CameraFeed>,
    camera_texture: Option<egui::TextureHandle>,
    camera_frame: u64,
    /// The picture of the last job sent from here: (file name, texture).
    job_picture: Option<(String, egui::TextureHandle)>,
    camera_big: bool,
    pending: Option<Pending>,
    /// PrusaLink password (the user name is always "maker").
    prusa_password: String,
    usb_ports: Vec<String>,
    usb_port: String,
    usb_baud: u32,
    /// The last plate sent from here, kept so it can be printed again.
    last_job: Option<PrintJob>,
    /// Name and printer path of the file seen printing most recently.
    seen_file: Option<(String, Option<String>)>,
    prev_state: JobState,
    again: Option<Again>,
    /// Prints this app saw finish, stop or fail (newest first).
    records: Vec<HistoryEntry>,
    nozzle_edit: Option<f32>,
    bed_edit: Option<f32>,
    fan_edit: Option<f32>,
    last_fan: f32,
    speed_mode: Option<usize>,
    jog_step: f32,
    tab: Tab,
    asked_files: bool,
    asked_history: bool,
    /// Printers found or used before, kept between runs.
    known: KnownPrinters,
    /// The current connection has been added to the known list.
    remembered_link: bool,
    /// A printer from an opened project, being looked for before connecting.
    locating: Option<(FoundPrinter, std::sync::mpsc::Receiver<bool>)>,
    /// A message shown in a small window until the person closes it.
    alert: Option<(String, String)>,
}

impl Default for DevicePage {
    fn default() -> Self {
        Self {
            calibrate: true,
            scan: None,
            scan_error: None,
            found: Vec::new(),
            scanned_once: false,
            manual_host: String::new(),
            link: None,
            camera: None,
            camera_texture: None,
            camera_frame: 0,
            job_picture: None,
            camera_big: false,
            pending: None,
            prusa_password: String::new(),
            usb_ports: Vec::new(),
            usb_port: String::new(),
            usb_baud: usb::DEFAULT_BAUD,
            last_job: None,
            seen_file: None,
            prev_state: JobState::Unknown,
            again: None,
            records: Vec::new(),
            nozzle_edit: None,
            bed_edit: None,
            fan_edit: None,
            last_fan: 100.0,
            speed_mode: None,
            jog_step: 10.0,
            tab: Tab::Xyz,
            asked_files: false,
            asked_history: false,
            // Nothing is read or written unless the app asks for the saved list.
            known: KnownPrinters::load(None),
            remembered_link: false,
            locating: None,
            alert: None,
        }
    }
}

impl DevicePage {
    /// A Device page that loads and saves the known printers in the user's settings folder.
    /// With `read_only` (check pictures) the list is shown but never written.
    pub fn with_saved_printers(read_only: bool) -> Self {
        let path = KnownPrinters::default_path();
        Self {
            known: if read_only {
                KnownPrinters::load_read_only(path)
            } else {
                KnownPrinters::load(path)
            },
            ..Self::default()
        }
    }
}

/// The "Enable Calibration" switch. Returns the response so callers can react to a change.
fn calibration_checkbox(ui: &mut Ui, on: &mut bool, self_levelling: bool) -> egui::Response {
    let hint = if self_levelling {
        "On: the printer runs its own calibration (bed levelling) before each print.\nOff: the printer is asked to skip it and start printing straight away."
    } else {
        "On: the app adds a calibration (bed levelling) step after the nozzle and plate heat up, so the first layer goes down evenly. Adds a minute or two.\nOff: no calibration step."
    };
    ui.checkbox(on, "Enable Calibration").on_hover_text(hint)
}

/// Printers whose own start routine already levels the bed. The person's Ender-3 V3 KE
/// (Creality network service) measured twice when the app added its own step.
fn levels_itself(api: Option<Api>) -> bool {
    api == Some(Api::CrealityLan)
}

impl DevicePage {
    /// The printer device currently selected (for saving in a project), without its
    /// password, under the name and connection kind it reported.
    pub fn current_device(&self) -> Option<FoundPrinter> {
        let link = self.link.as_ref()?;
        let (hostname, api) = link.with_state(|s| (s.status.hostname.clone(), s.api));
        let mut p = link.printer.clone();
        p.login = None;
        if let Some(h) = hostname.filter(|h| !h.is_empty()) {
            p.name = h;
        }
        p.api = api.or(p.api);
        Some(p)
    }

    /// Selects the printer saved in a project: looks for it first (network or USB) and
    /// connects only if it is there. If it isn't, no printer is selected and an alert says so.
    pub fn select_saved_device(&mut self, printer: FoundPrinter, ctx: &egui::Context) {
        self.disconnect();
        let (tx, rx) = std::sync::mpsc::channel();
        let p = printer.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(device_is_present(&p));
            c.request_repaint();
        });
        self.locating = Some((printer, rx));
    }

    /// No printer selected (a project without one, or "New project").
    pub fn deselect(&mut self) {
        self.locating = None;
        self.disconnect();
    }

    /// Shows a message in a small window.
    pub fn show_alert(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.alert = Some((title.into(), body.into()));
    }

    fn poll_locating(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.locating else {
            return;
        };
        match rx.try_recv() {
            Ok(found) => {
                let (printer, _) = self.locating.take().expect("locating");
                if found {
                    self.connect(printer, ctx);
                } else {
                    let place = if printer.api == Some(Api::Usb) {
                        "on USB port"
                    } else {
                        "at"
                    };
                    self.show_alert(
                        "Printer not found",
                        format!(
                            "The printer saved in this project, {} ({place} {}), could not be located.\n\nNo printer is selected. Check that it is switched on and connected, then pick it on the Device page.",
                            printer.name, printer.host
                        ),
                    );
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200))
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.locating = None,
        }
    }

    /// The name of a printer being looked for, if any.
    pub fn locating_name(&self) -> Option<&str> {
        self.locating.as_ref().map(|(p, _)| p.name.as_str())
    }

    fn alert_dialog(&mut self, ctx: &egui::Context) {
        let Some((title, body)) = &self.alert else {
            return;
        };
        let mut close = false;
        egui::Modal::new(egui::Id::new("printcraft_alert")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(title.as_str());
            ui.add_space(6.0);
            ui.label(body.as_str());
            ui.add_space(10.0);
            close = ui
                .add(
                    egui::Button::new(RichText::new("OK").color(Color32::WHITE))
                        .fill(theme::ACCENT_DARK)
                        .min_size(egui::vec2(80.0, 30.0)),
                )
                .clicked();
        });
        if close {
            self.alert = None;
        }
    }

    pub fn is_connected(&self) -> bool {
        self.link
            .as_ref()
            .is_some_and(|l| l.with_state(|s| s.connected))
    }

    /// Short text for the status bar, e.g. "Printing 42 %".
    pub fn summary(&self) -> Option<String> {
        let link = self.link.as_ref()?;
        let name = &link.printer.name;
        Some(link.with_state(|s| {
            if !s.connected {
                return format!("{name}: not connected");
            }
            match (s.status.state, s.status.progress) {
                (JobState::Printing, Some(p)) => format!("{name}: printing {p:.0} %"),
                (state, _) => format!("{name}: {}", state.label().to_lowercase()),
            }
        }))
    }

    /// Asks the person to confirm before anything is sent to the printer.
    pub fn ask_to_print(&mut self, job: PrintJob) {
        self.pending = Some(Pending::Print(job));
    }

    /// Connects to a typed-in address; the kind of printer is worked out on the way.
    pub fn connect_to(&mut self, host: String, ctx: &egui::Context) {
        self.manual_host = host.clone();
        let printer = FoundPrinter {
            name: format!("Printer at {host}"),
            host,
            ..Default::default()
        };
        self.connect(printer, ctx);
    }

    fn connect_usb(&mut self, ctx: &egui::Context) {
        let printer = FoundPrinter {
            name: format!("USB printer ({})", self.usb_port),
            model: "USB".into(),
            host: self.usb_port.clone(),
            api: Some(Api::Usb),
            baud: Some(self.usb_baud),
            ..Default::default()
        };
        self.connect(printer, ctx);
    }

    fn connect(&mut self, mut printer: FoundPrinter, ctx: &egui::Context) {
        if printer.api != Some(Api::Usb) && !self.prusa_password.trim().is_empty() {
            printer.login = Some((
                prusalink::DEFAULT_USER.into(),
                self.prusa_password.trim().to_string(),
            ));
        }
        let wake = waker(ctx);
        // A USB printer has no network camera.
        self.camera = (printer.api != Some(Api::Usb))
            .then(|| CameraFeed::start(printer.host.clone(), wake.clone()));
        self.camera_texture = None;
        self.camera_frame = 0;
        self.prev_state = JobState::Unknown;
        self.asked_files = false;
        self.asked_history = false;
        self.remembered_link = false;
        self.link = Some(PrinterLink::connect(printer, wake));
    }

    fn disconnect(&mut self) {
        self.link = None;
        self.camera = None;
        self.camera_texture = None;
        self.camera_big = false;
    }

    fn send(&self, cmd: Command) {
        if let Some(link) = &self.link {
            link.send(cmd);
        }
    }

    fn api(&self) -> Option<Api> {
        let link = self.link.as_ref()?;
        link.with_state(|s| s.api).or(link.printer.api)
    }

    /// Sends a sliced plate, adding calibration and layer information as the printer needs.
    fn send_job(&mut self, job: PrintJob) {
        let klipper = self.api().is_some_and(Api::is_klipper);
        let mut text = (*job.gcode).clone();
        if self.calibrate && !levels_itself(self.api()) {
            text = with_calibration(&text, klipper);
        }
        if klipper {
            text = with_layer_info(&text);
            // Klipper's M109/M190 also wait for cooling; wait only while heating instead.
            text = crate::heat::heat_up_only(&text);
        }
        self.last_job = Some(job.clone());
        // Tell the printer the Enable Calibration choice before it starts.
        self.send(Command::SetCalibration(self.calibrate));
        self.send(Command::UploadAndPrint {
            name: job.name,
            gcode: Arc::new(text),
        });
    }

    /// Prints a file already on the printer, with the Enable Calibration choice.
    fn start_stored_file(&self, path: String) {
        self.send(Command::SetCalibration(self.calibrate));
        self.send(Command::PrintFile(path));
    }

    /// What "Print again" would print right now, if anything.
    fn again_target(&self) -> Option<Again> {
        let seen = self.seen_file.clone();
        if let Some(job) = &self.last_job {
            let matches = seen.as_ref().is_none_or(|(name, _)| {
                name.ends_with(&job.name) || job.name.ends_with(name.as_str())
            });
            if matches {
                return Some(Again::Job(job.clone()));
            }
        }
        let files_ok = self.api().is_some_and(|a| a.abilities().files);
        match seen {
            Some((name, Some(path))) if files_ok => Some(Again::File { name, path }),
            _ => self.last_job.clone().map(Again::Job),
        }
    }

    /// Runs every frame whatever page is showing: notices a print finishing (and asks
    /// "Print again?"), keeps the records, and shows the confirmation dialogs.
    pub fn overlay(&mut self, ctx: &egui::Context) {
        if let Some(s) = self.link.as_ref().map(|l| l.state()) {
            self.observe(&s);
            self.remember_connected(&s);
        }
        self.poll_locating(ctx);
        self.alert_dialog(ctx);
        self.confirm_dialog(ctx);
        self.again_dialog(ctx);
        self.camera_window(ctx);
    }

    /// Follows the printer's state: remembers what is printing, records prints that end,
    /// and asks "Print again?" when one finishes.
    fn observe(&mut self, s: &LinkState) {
        let state = s.status.state;
        if state.is_active() {
            if let Some(f) = s.status.file.clone() {
                self.seen_file = Some((f, s.status.file_path.clone()));
            }
        }
        if self.prev_state.is_active()
            && matches!(
                state,
                JobState::Finished | JobState::Stopped | JobState::Failed
            )
        {
            let name = self
                .seen_file
                .as_ref()
                .map(|f| f.0.clone())
                .unwrap_or_else(|| "print".into());
            self.records.insert(
                0,
                HistoryEntry {
                    name,
                    outcome: state.label().to_string(),
                    ended: Some(chrono::Utc::now().timestamp()),
                    duration_s: s.status.elapsed_s,
                },
            );
            if state == JobState::Finished {
                self.again = self.again_target();
            }
        }
        if state != JobState::Unknown {
            self.prev_state = state;
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, job: Option<PrintJob>) {
        let ctx = ui.ctx().clone();
        self.poll_scan(&ctx);
        self.update_camera_texture(&ctx);
        let Some(state) = self.link.as_ref().map(|l| l.state()) else {
            ui.columns(2, |cols| {
                egui::ScrollArea::vertical()
                    .id_salt("find_left")
                    .show(&mut cols[0], |ui| self.find_card(ui));
                egui::ScrollArea::vertical()
                    .id_salt("find_right")
                    .show(&mut cols[1], |ui| self.usb_card(ui));
            });
            return;
        };
        // Keep the progress and clocks fresh while connected.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        self.header(ui, &state);
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            egui::ScrollArea::vertical()
                .id_salt("device_left")
                .show(&mut cols[0], |ui| {
                    self.camera_card(ui);
                    self.current_card(ui, &state);
                    self.send_card(ui, &state, job.clone());
                    log_card(ui, &state);
                });
            egui::ScrollArea::vertical()
                .id_salt("device_right")
                .show(&mut cols[1], |ui| {
                    self.settings_card(ui, &state);
                    self.tabs_card(ui, &state);
                });
        });
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(scan) = &self.scan else { return };
        let found = match scan.rx.try_recv() {
            Ok(found) => found,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            // The search thread ended without an answer: stop spinning, nothing found.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        // Every printer found is kept in the known list for next time.
        let now = chrono::Utc::now().timestamp();
        let mut changed = false;
        for p in &found {
            changed |= self.known.remember(p, now);
        }
        if changed {
            let _ = self.known.save();
        }
        self.found = found;
        self.scan = None;
        self.scanned_once = true;
    }

    /// Once a connection works, keeps the printer in the known list under the name it
    /// reports and the kind of connection that worked.
    fn remember_connected(&mut self, s: &LinkState) {
        if self.remembered_link || !s.connected {
            return;
        }
        let Some(link) = &self.link else { return };
        let mut p = link.printer.clone();
        if let Some(h) = s.status.hostname.clone().filter(|h| !h.is_empty()) {
            p.name = h;
        }
        if let Some(m) = s.status.model.clone().filter(|m| !m.is_empty()) {
            p.model = m;
        }
        if p.model.is_empty() {
            p.model = s.api.map(|a| a.label().to_string()).unwrap_or_default();
        }
        p.api = s.api.or(p.api);
        self.remembered_link = true;
        if self.known.remember(&p, chrono::Utc::now().timestamp()) {
            let _ = self.known.save();
        }
    }

    // ----- not connected yet -----------------------------------------------------------------

    fn known_card(&mut self, ui: &mut Ui) {
        if self.known.list.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        let mut connect = None;
        let mut forget = None;
        card(ui, "Known printers", |ui| {
            ui.label(
                RichText::new("Printers found or used before. Pick one to connect again.")
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(4.0);
            for k in &self.known.list {
                let p = &k.printer;
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .corner_radius(6)
                    .inner_margin(8)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&p.name).strong());
                                let kind = p.api.map(|a| a.label()).unwrap_or("kind not known yet");
                                let mut details = vec![p.host.clone(), kind.to_string()];
                                if !p.model.is_empty() && p.model != kind {
                                    details.insert(0, p.model.clone());
                                }
                                if k.last_seen > 0 {
                                    details.push(format!("last used {}", date_time(k.last_seen)));
                                }
                                ui.label(
                                    RichText::new(details.join(" · "))
                                        .color(theme::TEXT_DIM)
                                        .small(),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button("Forget")
                                        .on_hover_text("Remove it from this list")
                                        .clicked()
                                    {
                                        forget = Some(p.host.clone());
                                    }
                                    if ui.button("Connect").clicked() {
                                        connect = Some(p.clone());
                                    }
                                },
                            );
                        });
                    });
            }
        });
        if let Some(host) = forget {
            self.known.forget(&host);
            let _ = self.known.save();
        }
        if let Some(p) = connect {
            if p.api == Some(Api::Usb) {
                self.usb_port = p.host.clone();
                self.usb_baud = p.baud.unwrap_or(usb::DEFAULT_BAUD);
            }
            self.connect(p, &ctx);
        }
    }

    /// The known-printers card, then the "Connect a printer" card: a Wi-Fi scan, the
    /// printers it found, and a box for typing an address.
    fn find_card(&mut self, ui: &mut Ui) {
        self.known_card(ui);
        let ctx = ui.ctx().clone();
        card(ui, "Connect a printer", |ui| {
            ui.label(
                RichText::new(
                    "Your printer and this computer need to be on the same Wi-Fi network.",
                )
                .color(theme::TEXT_DIM),
            );
            ui.add_space(4.0);
            self.wifi_scan_section(ui);
            let chosen = self.found_printers_list(ui);
            ui.add_space(8.0);
            let typed = self.manual_address_entry(ui);
            if let Some(p) = chosen {
                self.connect(p, &ctx);
            } else if let Some(host) = typed {
                self.connect_to(host, &ctx);
            }
        });
    }

    /// The "Find printers on Wi-Fi" button, or the scan's progress while it runs, then
    /// any scan error and the "No printers answered" note.
    fn wifi_scan_section(&mut self, ui: &mut Ui) {
        match &self.scan {
            Some(scan) => {
                let n = scan.checked.load(Ordering::Relaxed);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "Looking on {}… {n} of 254 addresses checked",
                        scan.network
                    ));
                });
                ui.add(egui::ProgressBar::new(n as f32 / 254.0));
            }
            None => {
                let b = egui::Button::new(
                    RichText::new("Find printers on Wi-Fi")
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(theme::ACCENT_DARK);
                if ui.add(b).clicked() {
                    match discovery::start() {
                        Ok(s) => {
                            self.scan = Some(s);
                            self.scan_error = None;
                        }
                        Err(e) => self.scan_error = Some(e),
                    }
                }
            }
        }
        if let Some(e) = &self.scan_error {
            ui.label(RichText::new(e).color(theme::WARNING));
        }
        if self.scanned_once && self.found.is_empty() && self.scan.is_none() {
            ui.label(
                RichText::new("No printers answered. Check that the printer is on and connected to Wi-Fi, or type its address below.")
                    .color(theme::WARNING),
            );
        }
    }

    /// One row per printer the scan found, each with a Connect button; returns the one
    /// whose Connect was clicked.
    fn found_printers_list(&self, ui: &mut Ui) -> Option<FoundPrinter> {
        let mut chosen = None;
        for p in &self.found {
            egui::Frame::new()
                .fill(theme::PANEL)
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&p.name).strong());
                            ui.label(
                                RichText::new(format!("{} · {}", p.model, p.host))
                                    .color(theme::TEXT_DIM)
                                    .small(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Connect").clicked() {
                                chosen = Some(p.clone());
                            }
                        });
                    });
                });
        }
        chosen
    }

    /// The typed-address box with its Connect button, and the PrusaLink password box;
    /// returns the address when Connect was clicked.
    fn manual_address_entry(&mut self, ui: &mut Ui) -> Option<String> {
        let mut typed = None;
        ui.label("Or type the printer's address (shown in its network settings):");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.manual_host)
                    .hint_text("192.168.1.50")
                    .desired_width(170.0),
            );
            let host = self.manual_host.trim().to_string();
            if ui
                .add_enabled(!host.is_empty(), egui::Button::new("Connect"))
                .clicked()
            {
                typed = Some(host);
            }
        });
        ui.horizontal(|ui| {
            ui.label("PrusaLink password");
            ui.add(
                egui::TextEdit::singleline(&mut self.prusa_password)
                    .password(true)
                    .hint_text("Prusa printers only")
                    .desired_width(150.0),
            )
            .on_hover_text(
                "Shown on a Prusa printer's screen under Settings → Network → PrusaLink",
            );
        });
        typed
    }

    fn usb_card(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let mut go = false;
        card(ui, "Printer on a USB cable", |ui| {
            ui.label(
                RichText::new("For printers running Marlin or Prusa firmware. The app sends the print line by line, so keep this computer awake until it finishes.")
                    .color(theme::TEXT_DIM),
            );
            if self.usb_ports.is_empty() && ui.ctx().cumulative_frame_nr() % 300 == 1 {
                self.usb_ports = usb::list_ports();
            }
            ui.horizontal(|ui| {
                let shown = if self.usb_port.is_empty() {
                    "Choose a port".to_string()
                } else {
                    self.usb_port.clone()
                };
                egui::ComboBox::from_id_salt("usb_port")
                    .selected_text(shown)
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        if self.usb_ports.is_empty() {
                            ui.label("No USB printers found");
                        }
                        for p in &self.usb_ports {
                            ui.selectable_value(&mut self.usb_port, p.clone(), p);
                        }
                    });
                if ui.button("Refresh").clicked() {
                    self.usb_ports = usb::list_ports();
                    if !self.usb_ports.contains(&self.usb_port) {
                        self.usb_port = self.usb_ports.first().cloned().unwrap_or_default();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Speed");
                egui::ComboBox::from_id_salt("usb_baud")
                    .selected_text(format!("{} baud", self.usb_baud))
                    .show_ui(ui, |ui| {
                        for b in usb::BAUD_RATES {
                            ui.selectable_value(&mut self.usb_baud, b, format!("{b} baud"));
                        }
                    });
                go = ui
                    .add_enabled(!self.usb_port.is_empty(), egui::Button::new("Connect"))
                    .clicked();
            });
        });
        if go {
            self.connect_usb(&ctx);
        }
    }

    // ----- connected ---------------------------------------------------------------------------

    /// The printer's tab: name_address, connection dot and a close button.
    fn header(&mut self, ui: &mut Ui, s: &LinkState) {
        let Some(link) = &self.link else { return };
        // "name_address" like the printer screen; just the address until the printer says its name.
        let label = match (&s.status.hostname, link.printer.api) {
            (_, Some(Api::Usb)) => link.printer.name.clone(),
            (Some(h), _) if !h.is_empty() => format!("{h}_{}", link.printer.host),
            _ if link.printer.name.starts_with("Printer at ") => link.printer.host.clone(),
            _ => format!("{}_{}", link.printer.name, link.printer.host),
        };
        let mut close = false;
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(theme::CARD)
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let color = if s.connected {
                            theme::ACCENT
                        } else {
                            theme::WARNING
                        };
                        let (r, painter) =
                            ui.allocate_painter(egui::vec2(10.0, 10.0), Sense::hover());
                        painter.circle_filled(r.rect.center(), 4.0, color);
                        ui.label(RichText::new(label).size(15.0));
                        ui.add_space(12.0);
                        close = close_button(ui).on_hover_text("Disconnect").clicked();
                    });
                });
            let api = s.api.map(|a| a.label()).unwrap_or("Connecting…");
            let model = s
                .status
                .model
                .as_deref()
                .filter(|m| !m.is_empty())
                .map(|m| format!(" · model {m}"))
                .unwrap_or_default();
            ui.label(RichText::new(format!("{api}{model}")).color(theme::TEXT_DIM));
        });
        if close {
            self.disconnect();
        }
    }

    fn update_camera_texture(&mut self, ctx: &egui::Context) {
        let Some(cam) = &self.camera else { return };
        // Only a new picture is copied out and converted (the camera may be idle).
        let seen = self.camera_frame;
        let latest = cam.frame.lock().ok().and_then(|f| {
            f.as_ref().filter(|(n, _)| *n != seen).map(|(n, rgba)| {
                let size = [rgba.width() as usize, rgba.height() as usize];
                (
                    *n,
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                )
            })
        });
        if let Some((n, img)) = latest {
            self.camera_frame = n;
            match &mut self.camera_texture {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => {
                    self.camera_texture =
                        Some(ctx.load_texture("printer-camera", img, egui::TextureOptions::LINEAR))
                }
            }
        }
    }

    fn restart_camera(&mut self, ctx: &egui::Context) {
        if let Some(link) = &self.link {
            if link.printer.api != Some(Api::Usb) {
                self.camera = Some(CameraFeed::start(link.printer.host.clone(), waker(ctx)));
                self.camera_texture = None;
                self.camera_frame = 0;
            }
        }
    }

    fn camera_card(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let mut refresh = false;
        egui::Frame::new().fill(theme::CARD).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Camera").size(15.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.camera_texture.is_some() && ui.small_button("Enlarge").on_hover_text("Show the camera bigger").clicked() {
                        self.camera_big = true;
                    }
                    if self.camera.is_some() && ui.small_button("Refresh").on_hover_text("Reconnect the video").clicked() {
                        refresh = true;
                    }
                });
            });
            ui.add_space(4.0);
            let Some(cam) = &self.camera else {
                ui.label(RichText::new("A printer on a USB cable has no network camera.").color(theme::TEXT_DIM));
                return;
            };
            match (cam.state(), &self.camera_texture) {
                (CameraState::Streaming(_), Some(tex)) => {
                    let size = tex.size_vec2();
                    let w = ui.available_width();
                    let scale = (w / size.x).min(380.0 / size.y);
                    let r = ui.add(egui::Image::new((tex.id(), size * scale)).corner_radius(6).sense(Sense::click()));
                    if r.on_hover_text("Click to enlarge").clicked() {
                        self.camera_big = true;
                    }
                }
                (CameraState::Streaming(url), None) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Waiting for the first picture from {url}"));
                    });
                }
                (CameraState::Looking, _) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking for a camera…");
                    });
                }
                (CameraState::NotFound, _) => {
                    ui.label(
                        RichText::new("No camera video found. If a camera is fitted, check it's turned on in the printer's settings, then press Refresh.")
                            .color(theme::TEXT_DIM),
                    );
                }
            }
        });
        ui.add_space(8.0);
        if refresh {
            self.restart_camera(&ctx);
        }
    }

    fn camera_window(&mut self, ctx: &egui::Context) {
        if !self.camera_big {
            return;
        }
        let Some(tex) = &self.camera_texture else {
            self.camera_big = false;
            return;
        };
        let mut open = true;
        let screen = ctx.content_rect().size();
        egui::Window::new("Camera")
            .open(&mut open)
            .collapsible(false)
            .default_size(screen * 0.8)
            .show(ctx, |ui| {
                let size = tex.size_vec2();
                let avail = ui.available_size();
                let scale = (avail.x / size.x).min((avail.y - 4.0).max(100.0) / size.y);
                ui.image((tex.id(), size * scale));
            });
        self.camera_big = open;
    }

    /// What the printer is doing now: the part's picture, the job's details, and the
    /// Print again / Pause or Resume / Stop buttons.
    fn current_card(&mut self, ui: &mut Ui, s: &LinkState) {
        let mut cmd = None;
        let mut ask_again = false;
        let picture = self.current_job_picture(ui, s);
        card(ui, "Current printing", |ui| {
            ui.horizontal(|ui| {
                job_picture_tile(ui, picture);
                ui.vertical(|ui| {
                    self.job_details(ui, s);
                    (cmd, ask_again) = self.job_buttons(ui, s);
                });
            });
            if let Some(b) = &s.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(b);
                });
            }
        });
        if let Some(c) = cmd {
            self.send(c);
        }
        if ask_again {
            self.again = self.again_target();
        }
    }

    /// The real picture of the part when the job came from this app: loads it into a
    /// texture once, and returns it while the printer is still on that file.
    fn current_job_picture(&mut self, ui: &Ui, s: &LinkState) -> Option<egui::TextureId> {
        let current = s
            .status
            .file
            .clone()
            .or_else(|| self.seen_file.as_ref().map(|f| f.0.clone()));
        if let Some(job) = &self.last_job {
            let shows_job = current
                .as_deref()
                .is_none_or(|f| f.ends_with(job.name.as_str()));
            let loaded = self
                .job_picture
                .as_ref()
                .is_some_and(|(n, _)| *n == job.name);
            if let (true, false, Some(img)) = (shows_job, loaded, &job.picture) {
                let size = [img.width() as usize, img.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                let tex = ui
                    .ctx()
                    .load_texture("job-picture", color, egui::TextureOptions::LINEAR);
                self.job_picture = Some((job.name.clone(), tex));
            }
        }
        self.job_picture
            .as_ref()
            .filter(|(n, _)| current.as_deref().is_none_or(|f| f.ends_with(n.as_str())))
            .map(|(_, t)| t.id())
    }

    /// File name, time printing, layer, state with the estimated finish, and progress.
    fn job_details(&self, ui: &mut Ui, s: &LinkState) {
        let st = &s.status;
        let file = st
            .file
            .clone()
            .or_else(|| self.seen_file.as_ref().map(|f| f.0.clone()));
        ui.label(
            RichText::new(file.unwrap_or_else(|| "No print".into()))
                .size(16.0)
                .strong(),
        );
        if let Some(e) = st.elapsed_s {
            ui.label(
                RichText::new(format!("Printing for {}", hours_minutes(e))).color(theme::TEXT_DIM),
            );
        }
        if let Some(l) = st.layer {
            // Some printers report 0 layers; use the count from the job sent from here.
            let known = self
                .last_job
                .as_ref()
                .filter(|j| {
                    st.file
                        .as_deref()
                        .is_none_or(|f| f.ends_with(j.name.as_str()))
                })
                .map(|j| j.layers as u32);
            let text = match st.total_layers.filter(|t| *t > 0).or(known) {
                Some(t) => format!("Layer {l} of {t}"),
                None => format!("Layer {l}"),
            };
            ui.label(RichText::new(text).color(theme::TEXT_DIM));
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(st.state.label())
                    .color(theme::ACCENT)
                    .strong(),
            );
            if let Some(r) = st.remaining_s.filter(|_| st.state.is_active()) {
                ui.label(format!("Estimated completion time: {}", completion_time(r)))
                    .on_hover_text(format!("Your local time ({})", utc_offset_label()));
                ui.label(
                    RichText::new(format!("({} left)", hours_minutes(r))).color(theme::TEXT_DIM),
                );
            }
        });
        if let Some(p) = st.progress {
            ui.add(
                egui::ProgressBar::new(p / 100.0)
                    .text(format!("{p:.0} %"))
                    .fill(theme::ACCENT_DARK),
            );
        }
    }

    /// Print again, Pause or Resume, and Stop. Stop asks first (sets `pending`); returns
    /// the Pause/Resume command to send and whether Print again was clicked.
    fn job_buttons(&mut self, ui: &mut Ui, s: &LinkState) -> (Option<Command>, bool) {
        let st = &s.status;
        let mut cmd = None;
        let mut ask_again = false;
        ui.horizontal(|ui| {
            let can = s.connected && s.busy.is_none();
            let can_again = can && !st.state.is_active() && self.again_target().is_some();
            if ui
                .add_enabled(can_again, egui::Button::new("Print again"))
                .on_hover_text("Print the last job again")
                .clicked()
            {
                ask_again = true;
            }
            match st.state {
                JobState::Paused => {
                    if ui.add_enabled(can, egui::Button::new("Resume")).clicked() {
                        cmd = Some(Command::Resume);
                    }
                }
                _ => {
                    let printing = st.state == JobState::Printing;
                    if ui
                        .add_enabled(can && printing, egui::Button::new("Pause"))
                        .clicked()
                    {
                        cmd = Some(Command::Pause);
                    }
                }
            }
            let stop = egui::Button::new(RichText::new("Stop").color(Color32::WHITE))
                .fill(Color32::from_rgb(150, 50, 50));
            if ui.add_enabled(can && st.state.is_active(), stop).clicked() {
                self.pending = Some(Pending::Stop);
            }
        });
        (cmd, ask_again)
    }

    fn send_card(&mut self, ui: &mut Ui, s: &LinkState, job: Option<PrintJob>) {
        card(ui, "Send the sliced plate", |ui| match job {
            None => {
                ui.label(
                    RichText::new("Slice the plate first, then come back here to print it.")
                        .color(theme::TEXT_DIM),
                );
            }
            Some(job) => {
                ui.label(format!("Ready to send: {}", job.name));
                let self_levelling = levels_itself(self.api());
                calibration_checkbox(ui, &mut self.calibrate, self_levelling);
                let busy = s.status.state.is_active() || s.busy.is_some();
                let b = egui::Button::new(
                    RichText::new("Print on this printer")
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(theme::ACCENT_DARK);
                let r = ui.add_enabled(s.connected && !busy, b);
                let r = if busy {
                    r.on_disabled_hover_text("The printer is busy")
                } else {
                    r
                };
                if r.clicked() {
                    self.pending = Some(Pending::Print(job));
                }
            }
        });
    }

    /// Calibration, temperatures, model fan and printing speed.
    fn settings_card(&mut self, ui: &mut Ui, s: &LinkState) {
        let st = s.status.clone();
        let controls = self.api().is_some_and(|a| a.abilities().controls) && s.connected;
        let mut cmds = Vec::new();
        let self_levelling = levels_itself(self.api());
        let mut calibration_changed = false;
        card(ui, "Print settings", |ui| {
            calibration_changed = self.calibration_details(ui, &st, self_levelling);
            ui.add_space(6.0);
            if !controls {
                ui.label(RichText::new("These controls aren't available for this printer's connection (PrusaLink). Use the printer's screen.").color(theme::TEXT_DIM));
            }
            ui.add_enabled_ui(controls, |ui| {
                self.temperature_tiles(ui, &st, &mut cmds);
                ui.add_space(6.0);
                self.model_fan_tile(ui, &st, &mut cmds);
                ui.add_space(6.0);
                self.printing_speed_tile(ui, &st, &mut cmds);
            });
        });
        // A printer with its own calibration setting is told straight away.
        if calibration_changed && self_levelling && s.connected {
            cmds.push(Command::SetCalibration(self.calibrate));
        }
        for c in cmds {
            self.send(c);
        }
    }

    /// Device details: whether the printer calibrates (levels the bed) before printing,
    /// with a note on what that means for this printer. Returns whether the box changed.
    fn calibration_details(
        &mut self,
        ui: &mut Ui,
        st: &PrinterStatus,
        self_levelling: bool,
    ) -> bool {
        let mut changed = false;
        egui::Frame::new()
            .fill(theme::PANEL)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    changed =
                        calibration_checkbox(ui, &mut self.calibrate, self_levelling).changed();
                    ui.add_space(12.0);
                    let note = match (self_levelling, st.printer_calibration) {
                        (true, Some(true)) => "The printer reports: calibration on".to_string(),
                        (true, Some(false)) => "The printer reports: calibration off".to_string(),
                        (true, None) => "Sent to the printer with each print".to_string(),
                        (false, _) if self.calibrate => {
                            "The app adds a levelling step to each print".to_string()
                        }
                        (false, _) => "No levelling step is added".to_string(),
                    };
                    ui.label(RichText::new(note).color(theme::TEXT_DIM));
                });
            });
        changed
    }

    /// The Nozzle Temp and Bed Temp tiles side by side.
    fn temperature_tiles(&mut self, ui: &mut Ui, st: &PrinterStatus, cmds: &mut Vec<Command>) {
        ui.horizontal_wrapped(|ui| {
            if let Some(v) = temp_tile(
                ui,
                "Nozzle Temp",
                st.nozzle,
                st.nozzle_target,
                300.0,
                &mut self.nozzle_edit,
            ) {
                cmds.push(Command::SetNozzle(v));
            }
            if let Some(v) = temp_tile(
                ui,
                "Bed Temp",
                st.bed,
                st.bed_target,
                120.0,
                &mut self.bed_edit,
            ) {
                cmds.push(Command::SetBed(v));
            }
        });
    }

    /// The Model fan tile: an on/off toggle and a wind speed slider.
    fn model_fan_tile(&mut self, ui: &mut Ui, st: &PrinterStatus, cmds: &mut Vec<Command>) {
        tile(ui, 300.0, |ui| {
            let shown = self.fan_edit.or(st.fan_percent).unwrap_or(0.0);
            if shown > 0.0 {
                self.last_fan = shown;
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Model fan").size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut on = shown > 0.0;
                    if toggle(ui, &mut on).clicked() {
                        let p = if on { self.last_fan.max(10.0) } else { 0.0 };
                        cmds.push(Command::SetFan(p));
                        self.fan_edit = Some(p);
                    }
                });
            });
            let mut v = shown;
            ui.label(format!("Wind speed: {v:.0}%"));
            ui.spacing_mut().slider_width = ui.available_width();
            let r = ui.add(egui::Slider::new(&mut v, 0.0..=100.0).show_value(false));
            if r.changed() {
                self.fan_edit = Some(v);
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                cmds.push(Command::SetFan(v));
            }
            // Once the printer reports the new value, follow it again.
            if !r.dragged()
                && self
                    .fan_edit
                    .is_some_and(|f| st.fan_percent.is_some_and(|p| (p - f).abs() < 1.5))
            {
                self.fan_edit = None;
            }
        });
    }

    /// The Printing speed tile: one button per speed mode, and the speed the printer
    /// reports.
    fn printing_speed_tile(&mut self, ui: &mut Ui, st: &PrinterStatus, cmds: &mut Vec<Command>) {
        tile(ui, ui.available_width(), |ui| {
            ui.label(RichText::new("Printing speed").size(15.0));
            ui.add_space(4.0);
            let current = self.speed_mode.or_else(|| {
                let p = st.speed_percent?.round() as u32;
                SPEED_MODES.iter().rposition(|(_, v)| *v == p)
            });
            ui.horizontal_wrapped(|ui| {
                for (i, (label, pct)) in SPEED_MODES.iter().enumerate() {
                    let r = ui.add(
                        egui::Button::selectable(
                            current == Some(i),
                            RichText::new(*label).size(15.0),
                        )
                        .min_size(egui::vec2(120.0, 34.0)),
                    );
                    if r.on_hover_text(format!("{pct}% of the sliced speed"))
                        .clicked()
                    {
                        self.speed_mode = Some(i);
                        cmds.push(Command::SetSpeed(*pct));
                    }
                }
            });
            if let Some(p) = st.speed_percent {
                ui.label(
                    RichText::new(format!("The printer reports {p:.0}% speed."))
                        .color(theme::TEXT_DIM)
                        .small(),
                );
            }
        });
    }

    /// The tabbed card under the printer: XYZ Control, Local Files, Records and
    /// Timelapse Video.
    fn tabs_card(&mut self, ui: &mut Ui, s: &LinkState) {
        let abilities = self.api().map(|a| a.abilities());
        let controls = abilities.as_ref().is_some_and(|a| a.controls);
        let files = abilities.as_ref().is_some_and(|a| a.files);
        let history = abilities.as_ref().is_some_and(|a| a.history);
        let mut cmds = Vec::new();
        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.tab_strip(ui);
                ui.separator();
                match self.tab {
                    Tab::Xyz => self.xyz_control_tab(ui, s, controls, &mut cmds),
                    Tab::Files => self.local_files_tab(ui, s, files, &mut cmds),
                    Tab::Records => self.records_tab(ui, s, history, &mut cmds),
                    Tab::Timelapse => timelapse_tab(ui),
                }
            });
        for c in cmds {
            self.send(c);
        }
    }

    /// The row of tab names; the open one is highlighted.
    fn tab_strip(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            for (t, label) in [
                (Tab::Xyz, "XYZ Control"),
                (Tab::Files, "Local Files"),
                (Tab::Records, "Records"),
                (Tab::Timelapse, "Timelapse Video"),
            ] {
                let text = RichText::new(label).size(15.0);
                let text = if self.tab == t {
                    text.color(theme::ACCENT).strong()
                } else {
                    text
                };
                if ui.add(egui::Button::new(text).frame(false)).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    /// XYZ Control: the jog step, the jog pad and a reminder to home first. Locked while
    /// printing or when the connection has no controls.
    fn xyz_control_tab(
        &mut self,
        ui: &mut Ui,
        s: &LinkState,
        controls: bool,
        cmds: &mut Vec<Command>,
    ) {
        let can = controls && s.connected && !s.status.state.is_active();
        if !can {
            let why = if s.status.state.is_active() {
                "Moving the print head is locked while printing."
            } else {
                "Not available for this connection."
            };
            ui.label(RichText::new(why).color(theme::TEXT_DIM));
        }
        ui.add_enabled_ui(can, |ui| {
            ui.horizontal(|ui| {
                ui.label("Step");
                for step in JOG_STEPS {
                    if ui
                        .add(
                            egui::Button::selectable(
                                self.jog_step == step,
                                format!("{step:.0} mm"),
                            )
                            .min_size(egui::vec2(70.0, 30.0)),
                        )
                        .clicked()
                    {
                        self.jog_step = step;
                    }
                }
            });
            ui.add_space(8.0);
            jog_pad(ui, self.jog_step, cmds);
            ui.label(
                RichText::new("Home the printer first after it has been switched on.")
                    .color(theme::TEXT_DIM)
                    .small(),
            );
        });
    }

    /// Local Files: the files stored on the printer, each with a Print button (which asks
    /// first). Asks the printer for the list the first time the tab is shown.
    fn local_files_tab(
        &mut self,
        ui: &mut Ui,
        s: &LinkState,
        files: bool,
        cmds: &mut Vec<Command>,
    ) {
        if !files {
            ui.label(
                RichText::new(
                    "A printer on a USB cable has no file list. Send a sliced plate instead.",
                )
                .color(theme::TEXT_DIM),
            );
            return;
        }
        if !self.asked_files && s.connected {
            self.asked_files = true;
            cmds.push(Command::RefreshFiles);
        }
        if ui.button("Refresh").clicked() {
            cmds.push(Command::RefreshFiles);
        }
        match &s.files {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Asking the printer for its files…");
                });
            }
            Some(files) if files.is_empty() => {
                ui.label(RichText::new("No print files on the printer.").color(theme::TEXT_DIM));
            }
            Some(files) => {
                let busy = s.status.state.is_active() || s.busy.is_some() || !s.connected;
                egui::Grid::new("files_grid")
                    .striped(true)
                    .num_columns(4)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        for f in files.iter().take(200) {
                            ui.label(&f.name);
                            ui.label(
                                RichText::new(f.size.map(file_size).unwrap_or_default())
                                    .color(theme::TEXT_DIM),
                            );
                            ui.label(
                                RichText::new(f.modified.map(date_time).unwrap_or_default())
                                    .color(theme::TEXT_DIM),
                            );
                            if ui.add_enabled(!busy, egui::Button::new("Print")).clicked() {
                                self.pending = Some(Pending::PrintFile {
                                    name: f.name.clone(),
                                    path: f.path.clone(),
                                });
                            }
                            ui.end_row();
                        }
                    });
            }
        }
    }

    /// Records: prints this app saw finish, plus the printer's own history when it keeps
    /// one, newest first.
    fn records_tab(
        &mut self,
        ui: &mut Ui,
        s: &LinkState,
        from_printer: bool,
        cmds: &mut Vec<Command>,
    ) {
        if from_printer {
            if !self.asked_history && s.connected {
                self.asked_history = true;
                cmds.push(Command::RefreshHistory);
            }
            if ui.button("Refresh").clicked() {
                cmds.push(Command::RefreshHistory);
            }
        }
        let rows = merge_records(&self.records, s.history.as_deref().unwrap_or(&[]));
        if rows.is_empty() {
            let text = if from_printer && s.history.is_none() {
                "Asking the printer for its records…"
            } else {
                "No records yet. Prints that finish while this app is connected are listed here."
            };
            ui.label(RichText::new(text).color(theme::TEXT_DIM));
        } else {
            egui::Grid::new("records_grid")
                .striped(true)
                .num_columns(4)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for r in rows.iter().take(100) {
                        ui.label(&r.name);
                        ui.label(RichText::new(&r.outcome).color(theme::TEXT_DIM));
                        ui.label(
                            RichText::new(r.ended.map(date_time).unwrap_or_default())
                                .color(theme::TEXT_DIM),
                        );
                        ui.label(
                            RichText::new(r.duration_s.map(hours_minutes).unwrap_or_default())
                                .color(theme::TEXT_DIM),
                        );
                        ui.end_row();
                    }
                });
        }
    }

    // ----- dialogs ------------------------------------------------------------------------------

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.pending else { return };
        let printer = self
            .link
            .as_ref()
            .map(|l| l.printer.name.clone())
            .unwrap_or_default();
        let (title, body, yes) = match pending {
            Pending::Print(job) => (
                "Start printing?",
                format!("Send {} to {printer} and start printing straight away?\n\nMake sure the plate is clean and empty.", job.name),
                "Send and print",
            ),
            Pending::PrintFile { name, .. } => (
                "Start printing?",
                format!("Print {name} from the printer's storage?\n\nMake sure the plate is clean and empty."),
                "Print",
            ),
            Pending::Stop => ("Stop the print?", "The print will be cancelled and can't be resumed.".to_string(), "Stop print"),
        };
        let mut answer = None;
        let printing = matches!(pending, Pending::Print(_));
        let self_levelling = levels_itself(self.api());
        let calibrate = &mut self.calibrate;
        egui::Modal::new(egui::Id::new("printer_confirm")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading(title);
            ui.add_space(6.0);
            ui.label(body);
            if printing {
                ui.add_space(6.0);
                calibration_checkbox(ui, calibrate, self_levelling);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new(yes).color(Color32::WHITE))
                            .fill(theme::ACCENT_DARK),
                    )
                    .clicked()
                {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            let pending = self.pending.take();
            if !yes || self.link.is_none() {
                return;
            }
            match pending {
                Some(Pending::Print(job)) => self.send_job(job),
                Some(Pending::PrintFile { path, .. }) => self.start_stored_file(path),
                Some(Pending::Stop) => self.send(Command::Stop),
                None => {}
            }
        }
    }

    /// "Print again?" with Yes / No, shown when a print finishes.
    fn again_dialog(&mut self, ctx: &egui::Context) {
        let Some(again) = &self.again else { return };
        let name = again.name().to_string();
        let mut answer = None;
        let self_levelling = levels_itself(self.api());
        let calibrate = &mut self.calibrate;
        egui::Modal::new(egui::Id::new("print_again")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Print again?");
            ui.add_space(6.0);
            ui.label(format!(
                "{name} has finished. Do you want to print it again?"
            ));
            ui.label(
                RichText::new("Take the finished print off the plate first.").color(theme::WARNING),
            );
            ui.add_space(4.0);
            calibration_checkbox(ui, calibrate, self_levelling);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Yes").color(Color32::WHITE))
                            .fill(theme::ACCENT_DARK)
                            .min_size(egui::vec2(80.0, 30.0)),
                    )
                    .clicked()
                {
                    answer = Some(true);
                }
                if ui
                    .add(egui::Button::new("No").min_size(egui::vec2(80.0, 30.0)))
                    .clicked()
                {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            let again = self.again.take();
            if !yes {
                return;
            }
            match again {
                Some(Again::Job(job)) => self.send_job(job),
                Some(Again::File { path, .. }) => self.start_stored_file(path),
                None => {}
            }
        }
    }
}

/// Whether a saved printer answers: its USB port exists, or something is listening on its
/// connection's port (any printer port when the kind isn't known).
pub fn device_is_present(p: &FoundPrinter) -> bool {
    use crate::printer::{creality, link, moonraker};
    let t = std::time::Duration::from_millis(1500);
    match p.api {
        Some(Api::Usb) => usb::list_ports().contains(&p.host),
        Some(Api::CrealityLan) => link::port_open(&p.host, creality::WS_PORT, t),
        Some(Api::Moonraker) => link::port_open(&p.host, moonraker::PORT, t),
        Some(Api::PrusaLink) => link::port_open(&p.host, prusalink::PORT, t),
        None => link::detect_api(&p.host).is_some(),
    }
}

fn waker(ctx: &egui::Context) -> Wake {
    let c = ctx.clone();
    Arc::new(move || c.request_repaint())
}

fn log_card(ui: &mut Ui, s: &LinkState) {
    card(ui, "Activity", |ui| {
        for line in s.log.iter().rev().take(8) {
            ui.label(RichText::new(line).small().color(theme::TEXT_DIM));
        }
    });
}

/// A darker rounded box inside a card, like the tiles on the printer screen.
fn tile<R>(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::PANEL)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(width - 24.0);
            ui.vertical(add).inner
        })
        .inner
}

/// Current and target temperature; returns a new target once the person has finished
/// typing or dragging it.
fn temp_tile(
    ui: &mut Ui,
    title: &str,
    now: Option<f32>,
    target: Option<f32>,
    max: f32,
    edit: &mut Option<f32>,
) -> Option<f32> {
    tile(ui, 250.0, |ui| {
        ui.label(RichText::new(title).size(15.0));
        let mut out = None;
        egui::Grid::new(title)
            .num_columns(2)
            .spacing([28.0, 4.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Current").color(theme::TEXT_DIM));
                ui.label(RichText::new("Target").color(theme::TEXT_DIM));
                ui.end_row();
                ui.label(
                    RichText::new(
                        now.map(|n| format!("{n:.0} °C"))
                            .unwrap_or_else(|| "–".into()),
                    )
                    .size(16.0),
                );
                let mut v = edit.or(target).unwrap_or(0.0);
                let r = ui.add(
                    egui::DragValue::new(&mut v)
                        .range(0.0..=max)
                        .speed(1.0)
                        .fixed_decimals(0)
                        .suffix(" °C"),
                );
                if r.changed() {
                    *edit = Some(v);
                }
                if edit.is_some() && !r.has_focus() && !r.dragged() {
                    out = edit.take();
                }
                ui.end_row();
            });
        out
    })
}

/// A small drawn "×" button (the symbol isn't in every font).
fn close_button(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), Sense::click());
    let color = if response.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    let r = rect.shrink(4.0);
    let stroke = egui::Stroke::new(1.6, color);
    ui.painter()
        .line_segment([r.left_top(), r.right_bottom()], stroke);
    ui.painter()
        .line_segment([r.right_top(), r.left_bottom()], stroke);
    response
}

/// An on/off switch.
fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(40.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(response.id, *on);
    let bg = if *on {
        theme::ACCENT
    } else {
        Color32::from_rgb(90, 96, 106)
    };
    ui.painter().rect_filled(rect, 11.0, bg);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, Color32::WHITE);
    response
}

/// A plain picture standing in for the print's thumbnail: a file with a cube on it.
fn thumbnail(ui: &mut Ui, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, Color32::from_rgb(28, 62, 92));
    let doc = egui::Rect::from_center_size(rect.center(), egui::vec2(size.y * 0.55, size.y * 0.7));
    p.rect_filled(doc, 4.0, Color32::from_rgb(64, 120, 170));
    let c = doc.center() + egui::vec2(0.0, 6.0);
    let r = doc.width() * 0.28;
    let hex: Vec<egui::Pos2> = (0..6)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_3 * i as f32 + std::f32::consts::FRAC_PI_6;
            c + egui::vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::convex_polygon(
        hex,
        Color32::from_rgb(120, 200, 240),
        egui::Stroke::NONE,
    ));
    p.line_segment(
        [c, c + egui::vec2(0.0, -r)],
        egui::Stroke::new(1.5, Color32::from_rgb(40, 110, 160)),
    );
    p.line_segment(
        [c, c + egui::vec2(r * 0.87, r * 0.5)],
        egui::Stroke::new(1.5, Color32::from_rgb(40, 110, 160)),
    );
    p.line_segment(
        [c, c + egui::vec2(-r * 0.87, r * 0.5)],
        egui::Stroke::new(1.5, Color32::from_rgb(40, 110, 160)),
    );
}

/// The picture beside the current job: the part's own picture on a blue tile when
/// there is one, otherwise the plain placeholder.
fn job_picture_tile(ui: &mut Ui, picture: Option<egui::TextureId>) {
    match picture {
        Some(id) => {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(150.0, 120.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, 6.0, Color32::from_rgb(28, 62, 92));
            let img = egui::Rect::from_center_size(rect.center(), egui::vec2(116.0, 116.0));
            ui.painter().image(
                id,
                img,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => thumbnail(ui, egui::vec2(150.0, 120.0)),
    }
}

/// The jog pad: Y+/Y− and X−/Home/X+ in a cross, with Z+ and Z− to the right. Each
/// click queues a move of `step` mm (or Home).
fn jog_pad(ui: &mut Ui, step: f32, cmds: &mut Vec<Command>) {
    let b = |ui: &mut Ui, label: &str, hint: &str| {
        ui.add(egui::Button::new(RichText::new(label).size(15.0)).min_size(egui::vec2(64.0, 40.0)))
            .on_hover_text(hint)
            .clicked()
    };
    egui::Grid::new("jog_grid")
        .spacing([8.0, 8.0])
        .show(ui, |ui| {
            ui.label("");
            if b(ui, "Y+", "Move towards the back") {
                cmds.push(Command::Jog {
                    axis: 'Y',
                    mm: step,
                });
            }
            ui.label("      ");
            if b(ui, "Z+", "Move the nozzle up (or the bed down)") {
                cmds.push(Command::Jog {
                    axis: 'Z',
                    mm: step,
                });
            }
            ui.end_row();
            if b(ui, "X−", "Move left") {
                cmds.push(Command::Jog {
                    axis: 'X',
                    mm: -step,
                });
            }
            if b(ui, "Home", "Send every axis home (G28)") {
                cmds.push(Command::Home);
            }
            if b(ui, "X+", "Move right") {
                cmds.push(Command::Jog {
                    axis: 'X',
                    mm: step,
                });
            }
            ui.label("");
            ui.label("");
            ui.end_row();
            ui.label("");
            if b(ui, "Y−", "Move towards the front") {
                cmds.push(Command::Jog {
                    axis: 'Y',
                    mm: -step,
                });
            }
            ui.label("");
            ui.label("");
            if b(ui, "Z−", "Move the nozzle down (or the bed up)") {
                cmds.push(Command::Jog {
                    axis: 'Z',
                    mm: -step,
                });
            }
            ui.end_row();
        });
}

/// Timelapse Video: a note that the videos can't be listed yet.
fn timelapse_tab(ui: &mut Ui) {
    ui.label(
        RichText::new("Timelapse videos stay on the printer for now: how the printer shares them isn't publicly documented, so this app can't list or download them yet. You can still watch the live camera on the left.")
            .color(theme::TEXT_DIM),
    );
}

/// "00h 37m", as on the printer screen.
pub fn hours_minutes(secs: u64) -> String {
    format!("{:02}h {:02}m", secs / 3600, (secs % 3600) / 60)
}

/// The local clock time a print will end, as "11/16 01:10".
pub fn completion_time(remaining_s: u64) -> String {
    let end = chrono::Local::now() + chrono::Duration::seconds(remaining_s as i64);
    end.format("%m/%d %H:%M").to_string()
}

/// Makes clock times follow this computer's own time zone. Some launchers start
/// programs with `TZ=UTC`, which would show the estimated completion time hours off from
/// the clock on the wall. A plain UTC setting is dropped when the system has its own zone
/// (/etc/localtime); any other TZ the person set on purpose is kept. Call it at start-up,
/// before other threads run.
pub fn use_system_time_zone() {
    #[cfg(unix)]
    if let Ok(tz) = std::env::var("TZ") {
        let t = tz.trim().trim_start_matches(':');
        let plain_utc = matches!(
            t,
            "" | "UTC" | "UTC0" | "Etc/UTC" | "Universal" | "Zulu" | "Etc/Universal" | "Etc/Zulu"
        );
        if plain_utc && std::path::Path::new("/etc/localtime").exists() {
            std::env::remove_var("TZ");
        }
    }
}

/// "UTC+07:00" for this computer's time zone.
pub fn utc_offset_label() -> String {
    let s = local_offset_seconds();
    let sign = if s < 0 { '-' } else { '+' };
    let a = s.abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
}

/// This computer's offset from UTC right now, in seconds.
pub fn local_offset_seconds() -> i32 {
    use chrono::Offset;
    chrono::Local::now().offset().fix().local_minus_utc()
}

fn date_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn file_size(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.0} KB", (bytes as f64 / 1024.0).ceil())
    }
}

/// How far apart this app's and the printer's end times may be for one print.
const SAME_PRINT_SECONDS: i64 = 300;

/// This app's records plus the printer's history, newest first. A print the printer
/// also lists (same name, ended within a few minutes) is shown once, as the printer has it.
fn merge_records(app: &[HistoryEntry], printer: &[HistoryEntry]) -> Vec<HistoryEntry> {
    let on_printer = |r: &HistoryEntry| {
        printer.iter().any(|p| {
            p.name == r.name
                && matches!((p.ended, r.ended), (Some(a), Some(b)) if (a - b).abs() <= SAME_PRINT_SECONDS)
        })
    };
    let mut rows: Vec<HistoryEntry> = app.iter().filter(|r| !on_printer(r)).cloned().collect();
    rows.extend(printer.iter().cloned());
    rows.sort_by_key(|r| std::cmp::Reverse(r.ended));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_print_the_printer_also_lists_is_shown_once() {
        let entry = |name: &str, ended: i64| HistoryEntry {
            name: name.into(),
            outcome: "Finished".into(),
            ended: Some(ended),
            duration_s: None,
        };
        let app = [entry("cube.gcode", 1_000), entry("only-here.gcode", 500)];
        let printer = [entry("cube.gcode", 1_030), entry("older.gcode", 100)];
        let rows = merge_records(&app, &printer);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["cube.gcode", "only-here.gcode", "older.gcode"]);
        assert_eq!(
            rows[0].ended,
            Some(1_030),
            "the printer's own entry is kept"
        );
    }

    #[test]
    fn times_read_like_the_printer_screen() {
        assert_eq!(hours_minutes(37 * 60 + 5), "00h 37m");
        assert_eq!(hours_minutes(3 * 3600 + 7 * 60), "03h 07m");
        let t = completion_time(0);
        assert_eq!(t.len(), "11/16 01:10".len());
        assert!(t.chars().nth(2) == Some('/') && t.chars().nth(5) == Some(' '));
    }

    /// The completion time must use this computer's own time zone, as the system clock
    /// does, even when the app is started with TZ=UTC.
    #[cfg(unix)]
    #[test]
    fn completion_time_uses_the_local_time_zone() {
        // The system's own zone, ignoring any TZ setting.
        let Ok(out) = std::process::Command::new("date")
            .arg("+%z")
            .env_remove("TZ")
            .output()
        else {
            return;
        };
        let z = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if z.len() != 5 || !std::path::Path::new("/etc/localtime").exists() {
            return;
        }
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let system =
            sign * (z[1..3].parse::<i32>().unwrap() * 3600 + z[3..5].parse::<i32>().unwrap() * 60);
        // Started the way some launchers do it.
        std::env::set_var("TZ", "UTC");
        use_system_time_zone();
        let ours = local_offset_seconds();
        assert_eq!(
            ours, system,
            "app offset {ours}s, system offset {system}s ({z})"
        );
        // A zone the person chose on purpose is respected.
        std::env::set_var("TZ", "Europe/London");
        use_system_time_zone();
        assert_eq!(std::env::var("TZ").as_deref(), Ok("Europe/London"));
        std::env::remove_var("TZ");
    }

    /// Waits (briefly) for the saved-printer search to finish.
    fn finish_locating(page: &mut DevicePage, ctx: &egui::Context) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while page.locating.is_some() && std::time::Instant::now() < until {
            page.poll_locating(ctx);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn a_saved_printer_that_cannot_be_found_leaves_none_selected_and_alerts() {
        let ctx = egui::Context::default();
        let mut page = DevicePage::default();
        let missing = FoundPrinter {
            host: "/dev/printcraft-no-such-port".into(),
            name: "USB printer".into(),
            api: Some(Api::Usb),
            baud: Some(115_200),
            ..Default::default()
        };
        page.select_saved_device(missing, &ctx);
        assert_eq!(page.locating_name(), Some("USB printer"));
        finish_locating(&mut page, &ctx);
        assert!(page.link.is_none(), "no printer is selected");
        assert!(page.current_device().is_none());
        let (title, body) = page.alert.clone().expect("an alert");
        assert_eq!(title, "Printer not found");
        assert!(
            body.contains("could not be located") && body.contains("/dev/printcraft-no-such-port")
        );
        // A network address that refuses: same result (port 1 on this computer is closed).
        page.alert = None;
        let gone = FoundPrinter {
            host: "127.0.0.1:1".into(),
            name: "Ender3V3KE-A654".into(),
            ..Default::default()
        };
        page.select_saved_device(gone, &ctx);
        finish_locating(&mut page, &ctx);
        assert!(page.link.is_none());
        assert!(page
            .alert
            .as_ref()
            .is_some_and(|(_, b)| b.contains("Ender3V3KE-A654")));
    }

    #[test]
    fn speed_modes_match_the_screen() {
        let labels: Vec<&str> = SPEED_MODES.iter().map(|m| m.0).collect();
        assert_eq!(
            labels,
            ["Silent", "Stable 50%", "Standard 100%", "Ultrafast 125%"]
        );
        assert_eq!(SPEED_MODES[3].1, 125);
    }

    fn state(job: JobState, file: Option<&str>) -> LinkState {
        let mut s = LinkState {
            connected: true,
            ..Default::default()
        };
        s.status.state = job;
        s.status.file = file.map(str::to_string);
        s.status.elapsed_s = Some(600);
        s
    }

    #[test]
    fn finishing_a_print_asks_to_print_again_once() {
        let mut page = DevicePage::default();
        page.last_job = Some(PrintJob {
            name: "spacer.gcode".into(),
            gcode: Arc::new("G28\n".into()),
            layers: 10,
            picture: None,
        });
        page.observe(&state(JobState::Printing, Some("spacer.gcode")));
        assert!(page.again.is_none(), "not while printing");
        page.observe(&state(JobState::Finished, None));
        assert!(matches!(&page.again, Some(Again::Job(j)) if j.name == "spacer.gcode"));
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].outcome, "Finished");
        // Answering No clears it; staying "finished" doesn't ask again.
        page.again = None;
        page.observe(&state(JobState::Finished, None));
        assert!(page.again.is_none());
        assert_eq!(page.records.len(), 1);
    }

    #[test]
    fn a_stopped_print_is_recorded_but_not_offered_again() {
        let mut page = DevicePage::default();
        page.last_job = Some(PrintJob {
            name: "a.gcode".into(),
            gcode: Arc::new(String::new()),
            layers: 1,
            picture: None,
        });
        page.observe(&state(JobState::Printing, Some("a.gcode")));
        page.observe(&state(JobState::Stopped, None));
        assert!(page.again.is_none());
        assert_eq!(page.records[0].outcome, "Stopped");
        // Connecting to a printer that has already finished doesn't ask either.
        let mut fresh = DevicePage::default();
        fresh.last_job = page.last_job.clone();
        fresh.observe(&state(JobState::Finished, Some("a.gcode")));
        assert!(fresh.again.is_none());
    }

    #[test]
    fn print_again_offers_the_job_sent_from_here() {
        let mut page = DevicePage::default();
        assert!(page.again_target().is_none());
        page.last_job = Some(PrintJob {
            name: "spacer.gcode".into(),
            gcode: Arc::new("G28\n".into()),
            layers: 10,
            picture: None,
        });
        page.seen_file = Some((
            "spacer.gcode".into(),
            Some("/usr/data/printer_data/gcodes/spacer.gcode".into()),
        ));
        assert!(matches!(page.again_target(), Some(Again::Job(j)) if j.name == "spacer.gcode"));
        // A different file printed from the printer's screen: no network link, so the
        // file can't be restarted remotely; fall back to the job sent from here.
        page.seen_file = Some(("other.gcode".into(), Some("other.gcode".into())));
        assert!(matches!(page.again_target(), Some(Again::Job(_))));
    }
}
