//! File dialogs that don't freeze the window. A system file dialog waits until the
//! person picks a file; run on the window's own thread it stops the window from
//! answering the desktop, which then offers to force-quit PrintCraft. So each dialog
//! runs on a thread of its own and its answer comes back through a channel the window
//! checks every frame. Only one dialog is open at a time.
//!
//! The dialogs are rfd's desktop-portal dialogs, which may be opened from any thread.

use super::project_ui::ProjectAction;
use super::PrintCraftApp;
use std::path::PathBuf;
use std::sync::mpsc;

/// What a dialog was opened for, with the file (or files) picked.
pub(super) enum Picked {
    /// Models to add to the plate.
    Models(Vec<PathBuf>),
    /// A project to open.
    OpenProject(PathBuf),
    /// Where to save the project, and what to do once it is saved (the New or Open
    /// that asked to save first).
    SaveProject {
        path: PathBuf,
        then: Option<ProjectAction>,
    },
    /// A presets file to import.
    ImportPresets(PathBuf),
    /// Where to export the presets, with the text to write.
    ExportPresets { path: PathBuf, text: String },
    /// Where to save the print file, with its contents and notes for the status line.
    Export {
        path: PathBuf,
        bytes: Vec<u8>,
        notes: Vec<String>,
    },
}

impl PrintCraftApp {
    /// Opens a file dialog on its own thread: `dialog` shows it and says what was picked
    /// (`None` when cancelled). Does nothing while another dialog is open.
    pub(super) fn ask_file(&mut self, dialog: impl FnOnce() -> Option<Picked> + Send + 'static) {
        if self.file_dialog.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            // INTENTIONAL: the window may have closed while the dialog was open.
            let _ = tx.send(dialog());
            ctx.request_repaint();
        });
        self.file_dialog = Some(rx);
    }

    /// Acts on the open dialog's answer once it has one.
    pub(super) fn poll_file_dialog(&mut self) {
        let Some(rx) = &self.file_dialog else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.file_dialog = None;
        match answer {
            Some(Picked::Models(paths)) => self.load_paths(&paths),
            Some(Picked::OpenProject(path)) => self.open_project(&path),
            Some(Picked::SaveProject { path, then }) => {
                self.save_to(&crate::project::with_project_extension(path));
                if let Some(action) = then.filter(|_| !self.has_unsaved_changes()) {
                    self.run(action);
                }
            }
            Some(Picked::Export { path, bytes, notes }) => self.write_export(&path, &bytes, &notes),
            Some(Picked::ImportPresets(path)) => self.import_presets(&path),
            Some(Picked::ExportPresets { path, text }) => self.write_presets(&path, &text),
            None => {}
        }
    }
}
