use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::format::format_size;
use crate::core::ops::{ConflictAction, ErrorAction, OpEvent, OpJob, OpKind, OpReply, OpStage};
use crate::ui::FileManagerApp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClipboardCommand {
    Copy,
    Cut,
    Paste,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClipboardTarget {
    Files,
    Text,
}

/// Keep text-edit clipboard behavior unless selection/clipboard data has a
/// file-manager meaning.
pub(super) fn clipboard_target(
    command: ClipboardCommand,
    text_edit_focused: bool,
    has_file_selection: bool,
    has_file_clipboard: bool,
) -> ClipboardTarget {
    if !text_edit_focused
        || match command {
            ClipboardCommand::Copy | ClipboardCommand::Cut => has_file_selection,
            ClipboardCommand::Paste => has_file_clipboard,
        }
    {
        ClipboardTarget::Files
    } else {
        ClipboardTarget::Text
    }
}

fn paths_for_selection(
    entries: &[crate::core::fs::FileEntry],
    selection: &std::collections::BTreeSet<usize>,
) -> Vec<PathBuf> {
    selection
        .iter()
        .filter_map(|&i| entries.get(i))
        .map(|e| e.path.clone())
        .collect()
}

fn internal_paste_plan(clipboard: &Clipboard, dest: PathBuf) -> (OpKind, bool) {
    if clipboard.cut {
        (OpKind::Move { sources: clipboard.paths.clone(), dest }, true)
    } else {
        (OpKind::Copy { sources: clipboard.paths.clone(), dest }, false)
    }
}

fn external_paste_plan(paths: Vec<PathBuf>, dest: PathBuf) -> Option<OpKind> {
    (!paths.is_empty()).then_some(OpKind::Copy { sources: paths, dest })
}

fn delete_confirmation(paths: Vec<PathBuf>, permanent: bool) -> Option<(Vec<PathBuf>, bool)> {
    (!paths.is_empty()).then_some((paths, !permanent))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteDecision {
    Confirm,
    Cancel,
    Pending,
}

fn confirmed_delete(paths: Vec<PathBuf>, recycle: bool, decision: DeleteDecision) -> Option<OpKind> {
    if decision != DeleteDecision::Confirm {
        return None;
    }
    Some(if recycle {
        OpKind::Recycle { sources: paths }
    } else {
        OpKind::Delete { sources: paths }
    })
}

fn toggle_bookmark(bookmarks: &mut Vec<PathBuf>, path: &std::path::Path) -> bool {
    if let Some(i) = bookmarks.iter().position(|b| b == path) {
        bookmarks.remove(i);
        false
    } else {
        bookmarks.push(path.to_path_buf());
        true
    }
}

fn rename_target(from: &std::path::Path, input: &str) -> Option<PathBuf> {
    let name = valid_creation_name(input)?;
    let normalized = name.trim_end_matches(|c| c == ' ' || c == '.');
    if normalized.is_empty()
        || normalized == "."
        || normalized == ".."
        || name == from.file_name().unwrap_or_default()
    {
        return None;
    }
    Some(from.with_file_name(name))
}

fn rename_operation(from: PathBuf, input: &str) -> Option<OpKind> {
    let to = rename_target(&from, input)?;
    Some(OpKind::Rename { from, to })
}

fn valid_creation_name(input: &str) -> Option<&str> {
    let name = input.trim();
    (!name.is_empty() && !name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']))
        .then_some(name)
}

fn creation_operation(parent: &std::path::Path, input: &str, is_dir: bool) -> Option<OpKind> {
    let path = parent.join(valid_creation_name(input)?);
    Some(if is_dir {
        OpKind::CreateDir { path }
    } else {
        OpKind::CreateFile { path }
    })
}

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

#[derive(Clone)]
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
        if tab.archive_prefix.is_some() {
            return Vec::new();
        }
        paths_for_selection(&tab.entries, &tab.selection)
    }

    // ------------------------------------------------------------------
    // Actions (shortcuts, menus).
    // ------------------------------------------------------------------

    pub(crate) fn act_copy(&mut self) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            if let Err(e) = crate::platform::set_clipboard_files(&paths) {
                tracing::warn!(error = %e, count = paths.len(), "failed to publish file clipboard data");
            }
            tracing::info!(count = paths.len(), "files copied to internal clipboard");
            self.ops_ui.clipboard = Some(Clipboard { paths, cut: false });
        } else {
            tracing::info!("copy requested with empty file selection");
        }
    }

    pub(crate) fn act_cut(&mut self) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            if let Err(e) = crate::platform::set_clipboard_files(&paths) {
                tracing::warn!(error = %e, count = paths.len(), "failed to publish file clipboard data");
            }
            self.ops_ui.clipboard = Some(Clipboard { paths, cut: true });
        }
    }

    pub(crate) fn act_paste(&mut self) {
        if self.tabs[self.active].archive_prefix.is_some() {
            return;
        }
        // Internal clipboard first (tracks cut-vs-copy), then Explorer's.
        if let Some(cb) = self.ops_ui.clipboard.clone() {
            let dest = self.tabs[self.active].path.clone();
            tracing::info!(count = cb.paths.len(), cut = cb.cut, "pasting internal file clipboard");
            let (kind, clear_internal) = internal_paste_plan(&cb, dest);
            if matches!(kind, OpKind::Move { .. }) {
                self.enqueue("ops.moving", kind);
            } else {
                self.enqueue("ops.copying", kind);
            }
            if clear_internal {
                self.ops_ui.clipboard = None;
            }
            return;
        }
        let ext = crate::platform::get_clipboard_files();
        if let Some(kind) = external_paste_plan(ext, self.tabs[self.active].path.clone()) {
            if let OpKind::Copy { sources, .. } = &kind {
                tracing::info!(count = sources.len(), "pasting Explorer file clipboard");
            }
            self.enqueue("ops.copying", kind);
        }
    }

    /// `permanent`: Shift+Delete — bypass the Recycle Bin.
    pub(crate) fn act_delete(&mut self, permanent: bool) {
        let paths = self.selected_paths();
        self.ops_ui.delete_confirm = delete_confirmation(paths, permanent);
    }

    pub(crate) fn act_rename(&mut self) {
        let tab = &self.tabs[self.active];
        if tab.archive_prefix.is_some() {
            return;
        }
        if tab.selection.len() == 1 {
            if let Some(entry) = tab.entries.get(*tab.selection.iter().next().unwrap()) {
                self.ops_ui.rename = Some((entry.path.clone(), entry.name.clone()));
            }
        }
    }

    pub(crate) fn act_bookmark(&mut self) {
        if self.tabs[self.active].archive_prefix.is_some() {
            return;
        }
        let path = self.tabs[self.active].path.clone();
        self.act_bookmark_path(&path);
    }

    pub(crate) fn act_bookmark_path(&mut self, path: &Path) {
        toggle_bookmark(&mut self.store.settings.bookmarks, path);
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
                if ui.small_button("✕").clicked() {
                    cancel_id = Some(job.id);
                }
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
        let mut decision = DeleteDecision::Pending;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(msg);
                ui.horizontal(|ui| {
                    if ui.button(ok).clicked() {
                        decision = DeleteDecision::Confirm;
                    }
                    if ui.button(cancel).clicked() {
                        decision = DeleteDecision::Cancel;
                    }
                });
            });
        self.ops_ui.delete_confirm = None;
        if decision == DeleteDecision::Pending {
            self.ops_ui.delete_confirm = Some((paths, recycle));
        } else if let Some(kind) = confirmed_delete(paths, recycle, decision) {
            self.enqueue("ops.deleting", kind);
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
                1 => {
                    if let Some(kind) = rename_operation(from, &new_name) {
                        self.enqueue("ops.creating", kind);
                    }
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
            match action {
                1 => {
                    if let Some(kind) = creation_operation(&self.tabs[self.active].path, &name, is_dir) {
                        self.enqueue("ops.creating", kind);
                    }
                }
                0 => self.ops_ui.create = Some((is_dir, name.trim().to_owned())),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> crate::core::fs::FileEntry {
        crate::core::fs::FileEntry {
            name: name.to_owned(),
            path: PathBuf::from(format!("C:\\data\\{name}")),
            is_dir: false,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: Some(1),
            modified: None,
            created: None,
            extension: None,
            archive_member: None,
        }
    }

    #[test]
    fn file_selection_resolves_to_paths_and_ignores_stale_indices() {
        let entries = vec![entry("a.txt"), entry("b.txt"), entry("c.txt")];
        let selection = [0, 2, 99].into_iter().collect();
        assert_eq!(
            paths_for_selection(&entries, &selection),
            vec![PathBuf::from("C:\\data\\a.txt"), PathBuf::from("C:\\data\\c.txt")]
        );
    }

    #[test]
    fn clipboard_shortcuts_choose_file_operations_or_text_by_user_context() {
        use ClipboardCommand::{Copy, Cut, Paste};
        use ClipboardTarget::{Files, Text};

        assert_eq!(clipboard_target(Copy, false, false, false), Files);
        assert_eq!(clipboard_target(Cut, false, false, false), Files);
        assert_eq!(clipboard_target(Paste, false, false, false), Files);

        assert_eq!(clipboard_target(Copy, true, true, false), Files);
        assert_eq!(clipboard_target(Cut, true, true, false), Files);

        assert_eq!(clipboard_target(Copy, true, false, false), Text);
        assert_eq!(clipboard_target(Cut, true, false, false), Text);
        assert_eq!(clipboard_target(Paste, true, false, false), Text);
        assert_eq!(clipboard_target(Paste, true, false, true), Files);
    }

    #[test]
    fn internal_copy_paste_keeps_clipboard_while_cut_paste_moves_and_clears_it() {
        let source = PathBuf::from("C:\\data\\source.txt");
        let dest = PathBuf::from("D:\\target");
        let copy = Clipboard {
            paths: vec![source.clone()],
            cut: false,
        };
        let (copy_job, clear_copy) = internal_paste_plan(&copy, dest.clone());
        assert!(!clear_copy);
        assert!(matches!(
            copy_job,
            OpKind::Copy { sources, dest: d } if sources == vec![source.clone()] && d == dest
        ));

        let cut = Clipboard {
            paths: vec![source.clone()],
            cut: true,
        };
        let (move_job, clear_cut) = internal_paste_plan(&cut, dest.clone());
        assert!(clear_cut);
        assert!(matches!(
            move_job,
            OpKind::Move { sources, dest: d } if sources == vec![source] && d == dest
        ));
    }

    #[test]
    fn explorer_paste_is_copy_and_empty_external_clipboard_is_noop() {
        let dest = PathBuf::from("D:\\target");
        let source = PathBuf::from("C:\\source.txt");
        assert!(external_paste_plan(Vec::new(), dest.clone()).is_none());
        assert!(matches!(
            external_paste_plan(vec![source.clone()], dest.clone()),
            Some(OpKind::Copy { sources, dest: d }) if sources == vec![source] && d == dest
        ));
    }

    #[test]
    fn delete_requires_selection_and_confirmation_and_respects_permanent_choice() {
        let a = PathBuf::from("C:\\a.txt");
        let selected = vec![a.clone()];
        assert!(delete_confirmation(Vec::new(), false).is_none());
        assert_eq!(delete_confirmation(selected.clone(), false), Some((selected.clone(), true)));
        assert_eq!(delete_confirmation(selected.clone(), true), Some((selected.clone(), false)));

        assert!(confirmed_delete(selected.clone(), true, DeleteDecision::Pending).is_none());
        assert!(confirmed_delete(selected.clone(), true, DeleteDecision::Cancel).is_none());
        assert!(matches!(
            confirmed_delete(selected.clone(), true, DeleteDecision::Confirm),
            Some(OpKind::Recycle { sources }) if sources == selected
        ));
        assert!(matches!(
            confirmed_delete(selected.clone(), false, DeleteDecision::Confirm),
            Some(OpKind::Delete { sources }) if sources == selected
        ));
    }

    #[test]
    fn bookmark_toggle_adds_then_removes_the_folder() {
        let path = PathBuf::from("C:\\data");
        let mut bookmarks = Vec::new();
        assert!(toggle_bookmark(&mut bookmarks, &path));
        assert_eq!(bookmarks, vec![path.clone()]);
        assert!(!toggle_bookmark(&mut bookmarks, &path));
        assert!(bookmarks.is_empty());
    }

    #[test]
    fn create_dialog_accepts_trimmed_names_and_rejects_empty_or_windows_separators() {
        assert_eq!(valid_creation_name("  отчёт 2026.txt  "), Some("отчёт 2026.txt"));
        assert_eq!(valid_creation_name("   "), None);
        for invalid in ["a\\b", "a/b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b"] {
            assert_eq!(valid_creation_name(invalid), None, "accepted {invalid:?}");
        }
        let parent = PathBuf::from("C:\\data");
        assert!(matches!(
            creation_operation(&parent, " new folder ", true),
            Some(OpKind::CreateDir { path }) if path == parent.join("new folder")
        ));
        assert!(matches!(
            creation_operation(&parent, " new file.txt ", false),
            Some(OpKind::CreateFile { path }) if path == parent.join("new file.txt")
        ));
        assert!(creation_operation(&parent, "bad:name", true).is_none());
    }

    #[test]
    fn rename_keeps_the_parent_and_ignores_empty_or_unchanged_names() {
        let from = PathBuf::from("C:\\data\\old name.txt");
        assert_eq!(
            rename_target(&from, "  new name.txt "),
            Some(PathBuf::from("C:\\data\\new name.txt"))
        );
        assert_eq!(rename_target(&from, " old name.txt "), None);
        assert_eq!(rename_target(&from, "   "), None);
        for invalid in [r"C:\outside.txt", r"..\outside.txt", "../outside.txt", ".", ".."] {
            assert_eq!(rename_target(&from, invalid), None);
        }
        assert!(matches!(
            rename_operation(from.clone(), " new name.txt "),
            Some(OpKind::Rename { from: old, to }) if old == from && to == PathBuf::from("C:\\data\\new name.txt")
        ));
    }
}
