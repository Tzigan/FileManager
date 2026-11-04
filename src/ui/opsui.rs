use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::format::format_size;
use crate::core::ops::{ConflictAction, ErrorAction, OpEvent, OpJob, OpKind, OpReply, OpStage};
use crate::ui::FileManagerApp;

pub struct JobUi {
    pub id: u64,
    pub label: String,
    pub verb: &'static str,
    pub stage: OpStage,
    pub file: PathBuf,
    pub done_b: u64,
    pub total_b: u64,
    pub done_f: usize,
    pub total_f: usize,
    pub cancel: Arc<AtomicBool>,
    pub failed: usize,
}

pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

#[derive(Default)]
pub struct OpsUi {
    pub next_id: u64,
    pub jobs: Vec<JobUi>,
    pub clipboard: Option<Clipboard>,
    // Dialog state.
    pub rename: Option<(PathBuf, String)>,
    pub create: Option<(bool, String)>, // (is_dir, name)
    pub delete_confirm: Option<(Vec<PathBuf>, bool)>, // (paths, recycle)
    pub conflict: Option<(u64, PathBuf, PathBuf)>,
    pub error_q: Option<(u64, PathBuf, String, bool)>,
    pub show_journal: bool,
    pub journal: Vec<String>,
}

impl FileManagerApp {
    pub(crate) fn enqueue(&mut self, verb: &'static str, kind: OpKind) {
        self.ops_ui.next_id += 1;
        let id = self.ops_ui.next_id;
        let job = OpJob {
            id,
            kind,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let _ = self.ops_tx.send(OpJob {
            id,
            kind: job.kind,
            cancel: job.cancel.clone(),
        });
        self.ops_ui.jobs.push(JobUi {
            id,
            label: String::new(),
            verb,
            stage: OpStage::Scanning,
            file: PathBuf::new(),
            done_b: 0,
            total_b: 0,
            done_f: 0,
            total_f: 0,
            cancel: job.cancel,
            failed: 0,
        });
    }

    /// Selection of the active tab as paths.
    pub(crate) fn selected_paths(&self) -> Vec<PathBuf> {
        let tab = &self.tabs[self.active];
        tab.selection
            .iter()
            .filter_map(|&i| tab.entries.get(i))
            .map(|e| e.path.clone())
            .collect()
    }

    // ------------------------------------------------------------------
    // Actions (shortcuts, menus).
    // ------------------------------------------------------------------

    pub(crate) fn act_copy(&mut self) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            let _ = crate::platform::set_clipboard_files(&paths);
            self.ops_ui.clipboard = Some(Clipboard { paths, cut: false });
        }
    }

    pub(crate) fn act_cut(&mut self) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            let _ = crate::platform::set_clipboard_files(&paths);
            self.ops_ui.clipboard = Some(Clipboard { paths, cut: true });
        }
    }

    pub(crate) fn act_paste(&mut self) {
        // Internal clipboard first (tracks cut-vs-copy), then Explorer's.
        if let Some(cb) = &self.ops_ui.clipboard {
            let dest = self.tabs[self.active].path.clone();
            if cb.cut {
                self.enqueue("ops.moving", OpKind::Move { sources: cb.paths.clone(), dest });
                self.ops_ui.clipboard = None;
            } else {
                self.enqueue("ops.copying", OpKind::Copy { sources: cb.paths.clone(), dest });
            }
            return;
        }
        let ext = crate::platform::get_clipboard_files();
        if !ext.is_empty() {
            let dest = self.tabs[self.active].path.clone();
            self.enqueue("ops.copying", OpKind::Copy { sources: ext, dest });
        }
    }

    /// `permanent`: Shift+Delete — bypass the Recycle Bin.
    pub(crate) fn act_delete(&mut self, permanent: bool) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            self.ops_ui.delete_confirm = Some((paths, !permanent));
        }
    }

    pub(crate) fn act_rename(&mut self) {
        let tab = &self.tabs[self.active];
        if tab.selection.len() == 1 {
            if let Some(entry) = tab.entries.get(*tab.selection.iter().next().unwrap()) {
                self.ops_ui.rename = Some((entry.path.clone(), entry.name.clone()));
            }
        }
    }

    pub(crate) fn act_bookmark(&mut self) {
        let path = self.tabs[self.active].path.clone();
        let bms = &mut self.store.settings.bookmarks;
        if let Some(i) = bms.iter().position(|b| b == &path) {
            bms.remove(i);
        } else {
            bms.push(path);
        }
        self.persist();
    }

    pub(crate) fn drain_ops_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.ops_evt.try_recv() {
            match ev {
                OpEvent::Progress { id, stage, file, done_bytes, total_bytes, done_files, total_files } => {
                    if let Some(j) = self.ops_ui.jobs.iter_mut().find(|j| j.id == id) {
                        j.stage = stage;
                        j.file = file;
                        j.done_b = done_bytes;
                        j.total_b = total_bytes;
                        j.done_f = done_files;
                        j.total_f = total_files;
                    }
                }
                OpEvent::AskConflict { id, src, dst } => {
                    self.ops_ui.conflict = Some((id, src, dst));
                }
                OpEvent::AskError { id, path, message, retryable } => {
                    self.ops_ui.journal.push(format!("{}: {}", path.display(), message));
                    self.ops_ui.error_q = Some((id, path, message, retryable));
                }
                OpEvent::Done { id, failed } => {
                    if let Some(pos) = self.ops_ui.jobs.iter().position(|j| j.id == id) {
                        self.ops_ui.jobs.remove(pos);
                    }
                    if failed > 0 {
                        self.ops_ui.show_journal = true;
                    }
                    // Reflect changes: reload listing + invalidate tree cache.
                    self.navigate_fresh(self.active, ctx);
                    self.tree.children.clear();
                }
            }
        }
    }

    pub(crate) fn reply_conflict(&self, id: u64, action: ConflictAction) {
        let _ = self.op_reply.send(OpReply::Conflict { id, action });
    }

    pub(crate) fn reply_error(&self, id: u64, action: ErrorAction) {
        let _ = self.op_reply.send(OpReply::Error { id, action });
    }

    // ------------------------------------------------------------------
    // Ops panel (progress) — rendered above the status bar.
    // ------------------------------------------------------------------

    pub(crate) fn ops_panel(&mut self, ui: &mut egui::Ui) {
        if self.ops_ui.jobs.is_empty() {
            return;
        }
        let mut cancel_id = None;
        for job in &self.ops_ui.jobs {
            ui.horizontal(|ui| {
                let frac = if job.total_b > 0 {
                    job.done_b as f32 / job.total_b as f32
                } else {
                    0.0
                };
                let verb = self.stage_verb(job);
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(200.0)
                        .text(format!("{verb} {:.0}%", frac * 100.0)),
                );
                ui.label(format!(
                    "{} / {} · {}",
                    format_size(job.done_b),
                    format_size(job.total_b),
                    job.file
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                if ui.small_button("✕").clicked() {
                    cancel_id = Some(job.id);
                }
            });
        }
        if let Some(id) = cancel_id {
            if let Some(j) = self.ops_ui.jobs.iter().find(|j| j.id == id) {
                j.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if self.ops_ui.show_journal || !self.ops_ui.journal.is_empty() {
            if ui.button(self.t("ops.journal").to_owned()).clicked() {
                self.ops_ui.show_journal = true;
            }
        }
    }

    fn stage_verb(&self, job: &JobUi) -> String {
        match job.stage {
            OpStage::Scanning => self.t("ops.scanning").to_owned(),
            OpStage::Copying => self.t("ops.copying").to_owned(),
            OpStage::Moving => self.t("ops.moving").to_owned(),
            OpStage::Deleting => self.t("ops.deleting").to_owned(),
            OpStage::Creating => self.t("ops.creating").to_owned(),
        }
    }

    // ------------------------------------------------------------------
    // Dialogs — called every frame from ui().
    // ------------------------------------------------------------------

    pub(crate) fn ops_dialogs(&mut self, ctx: &egui::Context) {
        self.delete_dialog(ctx);
        self.rename_dialog(ctx);
        self.create_dialog(ctx);
        self.conflict_dialog(ctx);
        self.error_dialog(ctx);
        self.journal_window(ctx);
    }

    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some((paths, recycle)) = self.ops_ui.delete_confirm.clone() else {
            return;
        };
        let title = self.t("dlg.delete_title").to_owned();
        let msg = format!(
            "{}: {}",
            self.t(if recycle { "dlg.delete_msg_recycle" } else { "dlg.delete_msg" }),
            paths.len()
        );
        let ok = self.t("dlg.ok").to_owned();
        let cancel = self.t("dlg.cancel").to_owned();
        let mut action = 0u8; // 1=delete 2=cancel
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(msg);
                ui.horizontal(|ui| {
                    if ui.button(ok).clicked() {
                        action = 1;
                    }
                    if ui.button(cancel).clicked() {
                        action = 2;
                    }
                });
            });
        self.ops_ui.delete_confirm = None;
        match action {
            1 if recycle => self.enqueue("ops.deleting", OpKind::Recycle { sources: paths }),
            1 => self.enqueue("ops.deleting", OpKind::Delete { sources: paths }),
            0 => self.ops_ui.delete_confirm = Some((paths, recycle)),
            _ => {}
        }
    }

    fn rename_dialog(&mut self, ctx: &egui::Context) {
        if self.ops_ui.rename.is_none() {
            return;
        }
        let title = self.t("dlg.rename_title").to_owned();
        let ok = self.t("dlg.ok").to_owned();
        let cancel = self.t("dlg.cancel").to_owned();
        let mut action = 0u8;
        let mut new_name = self.ops_ui.rename.as_ref().unwrap().1.clone();
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut new_name).desired_width(280.0));
                ui.horizontal(|ui| {
                    if ui.button(ok).clicked() {
                        action = 1;
                    }
                    if ui.button(cancel).clicked() {
                        action = 2;
                    }
                });
            });
        if let Some((from, _)) = self.ops_ui.rename.take() {
            match action {
                1 if !new_name.trim().is_empty() && new_name.trim() != from.file_name().unwrap_or_default() => {
                    let to = from.with_file_name(new_name.trim());
                    self.enqueue("ops.creating", OpKind::Rename { from, to });
                }
                0 => self.ops_ui.rename = Some((from, new_name)),
                _ => {}
            }
        }
    }

    fn create_dialog(&mut self, ctx: &egui::Context) {
        let Some((is_dir, _)) = self.ops_ui.create.clone() else {
            return;
        };
        let title = self
            .t(if is_dir { "dlg.create_folder_title" } else { "dlg.create_file_title" })
            .to_owned();
        let ok = self.t("dlg.ok").to_owned();
        let cancel = self.t("dlg.cancel").to_owned();
        let mut action = 0u8;
        let mut name = self.ops_ui.create.as_ref().unwrap().1.clone();
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut name).desired_width(280.0));
                ui.horizontal(|ui| {
                    if ui.button(ok).clicked() {
                        action = 1;
                    }
                    if ui.button(cancel).clicked() {
                        action = 2;
                    }
                });
            });
        if let Some((is_dir, _)) = self.ops_ui.create.take() {
            let name = name.trim().to_owned();
            match action {
                1 if !name.is_empty() && !name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) => {
                    let path = self.tabs[self.active].path.join(&name);
                    let kind = if is_dir {
                        OpKind::CreateDir { path }
                    } else {
                        OpKind::CreateFile { path }
                    };
                    self.enqueue("ops.creating", kind);
                }
                0 => self.ops_ui.create = Some((is_dir, name)),
                _ => {}
            }
        }
    }

    fn conflict_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, src, dst)) = self.ops_ui.conflict.clone() else {
            return;
        };
        let title = self.t("conflict.title").to_owned();
        let msg = format!(
            "{}\n{} → {}",
            self.t("conflict.exists"),
            src.file_name().unwrap_or_default().to_string_lossy(),
            dst.display()
        );
        let mut action = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(msg);
                ui.horizontal(|ui| {
                    for (key, act) in [
                        ("conflict.overwrite", ConflictAction::Overwrite),
                        ("conflict.skip", ConflictAction::Skip),
                        ("conflict.overwrite_all", ConflictAction::OverwriteAll),
                        ("conflict.skip_all", ConflictAction::SkipAll),
                        ("conflict.cancel", ConflictAction::Cancel),
                    ] {
                        if ui.button(self.t(key).to_owned()).clicked() {
                            action = Some(act);
                        }
                    }
                });
            });
        if let Some(act) = action {
            self.reply_conflict(id, act);
            self.ops_ui.conflict = None;
        }
    }

    fn error_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, path, message, _retryable)) = self.ops_ui.error_q.clone() else {
            return;
        };
        let title = self.t("err.title").to_owned();
        let msg = format!("{}\n{}", path.display(), message);
        let mut action = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.colored_label(ui.visuals().error_fg_color, msg);
                ui.horizontal(|ui| {
                    if ui.button(self.t("err.retry").to_owned()).clicked() {
                        action = Some(ErrorAction::Retry);
                    }
                    if ui.button(self.t("err.skip").to_owned()).clicked() {
                        action = Some(ErrorAction::Skip);
                    }
                    if ui.button(self.t("err.cancel").to_owned()).clicked() {
                        action = Some(ErrorAction::Cancel);
                    }
                });
            });
        if let Some(act) = action {
            self.reply_error(id, act);
            self.ops_ui.error_q = None;
        }
    }

    fn journal_window(&mut self, ctx: &egui::Context) {
        if !self.ops_ui.show_journal {
            return;
        }
        let title = self.t("ops.journal").to_owned();
        let mut open = true;
        let items = self.ops_ui.journal.clone();
        egui::Window::new(title)
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for line in &items {
                        ui.label(line);
                    }
                });
            });
        self.ops_ui.show_journal = open;
    }
}
