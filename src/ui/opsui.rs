use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::core::format::format_size;
use crate::core::ops::{ConflictAction, ErrorAction, OpEvent, OpJob, OpKind, OpReply, OpStage};
use crate::ui::FileManagerApp;
use crate::ui::tabs::SelectionKey;

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

pub(crate) fn operation_error_key(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::PermissionDenied => "error.operation.access_denied",
        ErrorKind::NotFound => "error.operation.not_found",
        ErrorKind::AlreadyExists => "error.operation.exists",
        ErrorKind::StorageFull => "error.operation.storage_full",
        ErrorKind::WouldBlock | ErrorKind::TimedOut => "error.operation.busy",
        ErrorKind::InvalidInput | ErrorKind::InvalidData => "error.operation.invalid",
        ErrorKind::NotADirectory => "error.operation.not_directory",
        ErrorKind::IsADirectory => "error.operation.is_directory",
        ErrorKind::DirectoryNotEmpty => "error.operation.directory_not_empty",
        ErrorKind::FileTooLarge => "error.operation.too_large",
        ErrorKind::Unsupported => "error.operation.unsupported",
        ErrorKind::Interrupted => "background_ops.cancelled",
        _ => "error.operation.failed",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingEntryKind {
    File,
    Folder,
}

fn name_taken_error_key(kind: ExistingEntryKind) -> &'static str {
    match kind {
        ExistingEntryKind::File => "error.name_taken_file",
        ExistingEntryKind::Folder => "error.name_taken_folder",
    }
}

// Проверка самого пути использует правила регистра конкретного каталога Windows.
fn existing_entry_kind(path: &Path) -> Option<ExistingEntryKind> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(if metadata.is_dir() {
        ExistingEntryKind::Folder
    } else {
        ExistingEntryKind::File
    })
}

fn operation_error_key_for_context(
    kind: ErrorKind,
    is_creation: bool,
    existing: Option<ExistingEntryKind>,
) -> &'static str {
    if is_creation
        && matches!(kind, ErrorKind::PermissionDenied | ErrorKind::AlreadyExists)
        && let Some(existing) = existing
    {
        return name_taken_error_key(existing);
    }
    operation_error_key(kind)
}

fn user_facing_operation_error(
    path: &Path,
    kind: ErrorKind,
    reason_key: Option<&'static str>,
    translate: impl FnOnce(&str) -> String,
) -> String {
    format!(
        "{}\n{}",
        path.display(),
        translate(reason_key.unwrap_or_else(|| operation_error_key(kind)))
    )
}

fn paths_for_selection(
    entries: &[crate::core::fs::FileEntry],
    selection: &std::collections::BTreeSet<SelectionKey>,
) -> Vec<PathBuf> {
    let selected_keys: HashSet<_> = selection.iter().map(SelectionKey::borrowed).collect();
    entries
        .iter()
        .filter(|entry| {
            selected_keys.contains(&(entry.path.as_path(), entry.archive_member.as_deref()))
        })
        .map(|entry| entry.path.clone())
        .collect()
}

fn internal_paste_plan(clipboard: &Clipboard, dest: PathBuf) -> (OpKind, bool) {
    if clipboard.cut {
        (
            OpKind::Move {
                sources: clipboard.paths.clone(),
                dest,
            },
            true,
        )
    } else {
        (
            OpKind::Copy {
                sources: clipboard.paths.clone(),
                dest,
            },
            false,
        )
    }
}

fn external_paste_plan(paths: Vec<PathBuf>, dest: PathBuf) -> Option<OpKind> {
    (!paths.is_empty()).then_some(OpKind::Copy {
        sources: paths,
        dest,
    })
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

fn confirmed_delete(
    paths: Vec<PathBuf>,
    recycle: bool,
    decision: DeleteDecision,
) -> Option<OpKind> {
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

fn text_file_name(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".txt") {
        name.to_owned()
    } else if Path::new(name).extension().is_some() {
        name.to_owned()
    } else {
        format!("{name}.txt")
    }
}

fn creation_target(parent: &Path, input: &str, is_dir: bool) -> Option<PathBuf> {
    let name = valid_creation_name(input)?;
    let final_name = if is_dir {
        name.to_owned()
    } else {
        text_file_name(name)
    };
    Some(parent.join(final_name))
}

fn creation_operation(parent: &std::path::Path, input: &str, is_dir: bool) -> Option<OpKind> {
    let path = creation_target(parent, input, is_dir)?;
    Some(if is_dir {
        OpKind::CreateDir { path }
    } else {
        OpKind::CreateFile { path }
    })
}

enum CreateSubmission {
    Enqueue(OpKind),
    KeepOpen {
        is_dir: bool,
        name: String,
        error_key: &'static str,
    },
    Invalid,
}

fn creation_submission(
    parent: &Path,
    input: &str,
    is_dir: bool,
    lookup: impl FnOnce(&Path) -> Option<ExistingEntryKind>,
) -> CreateSubmission {
    let Some(operation) = creation_operation(parent, input, is_dir) else {
        return CreateSubmission::Invalid;
    };
    let path = match &operation {
        OpKind::CreateDir { path } | OpKind::CreateFile { path } => path,
        _ => unreachable!("создание всегда задаёт путь нового элемента"),
    };
    match lookup(path) {
        Some(existing) => CreateSubmission::KeepOpen {
            is_dir,
            name: input.trim().to_owned(),
            error_key: name_taken_error_key(existing),
        },
        None => CreateSubmission::Enqueue(operation),
    }
}

fn apply_creation_submission(
    form: &mut Option<(bool, String)>,
    error_key: &mut Option<&'static str>,
    submission: CreateSubmission,
) -> Option<OpKind> {
    match submission {
        CreateSubmission::Enqueue(operation) => {
            *form = None;
            *error_key = None;
            Some(operation)
        }
        CreateSubmission::KeepOpen {
            is_dir,
            name,
            error_key: reason,
        } => {
            *form = Some((is_dir, name));
            *error_key = Some(reason);
            None
        }
        CreateSubmission::Invalid => {
            *form = None;
            *error_key = None;
            None
        }
    }
}

fn apply_conflict_reason(existing: Option<ExistingEntryKind>) -> &'static str {
    existing
        .map(name_taken_error_key)
        .unwrap_or("conflict.exists")
}

pub struct JobUi {
    pub id: u64,
    pub verb: &'static str,
    pub stage: OpStage,
    pub file: PathBuf,
    pub done_b: u64,
    pub total_b: u64,
    pub done_f: usize,
    pub total_f: usize,
    pub cancel: Arc<AtomicBool>,
    pub is_creation: bool,
    pub is_rename: bool,
    /// Папки, чей кэш списка надо сбросить, когда операция закончится.
    pub touched_dirs: Vec<PathBuf>,
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
    pub delete_ask: Option<Vec<PathBuf>>,
    pub conflict: Option<(u64, PathBuf, PathBuf)>,
    pub conflict_reason: Option<&'static str>,
    pub error_q: Option<(u64, PathBuf, ErrorKind, bool)>,
    pub error_reason: Option<&'static str>,
    pub create_error: Option<&'static str>,
    pub show_journal: bool,
    pub journal: Vec<String>,
}

impl FileManagerApp {
    pub(crate) fn enqueue(&mut self, verb: &'static str, kind: OpKind) {
        if self.shutdown_active() {
            return;
        }
        let is_creation = matches!(&kind, OpKind::CreateDir { .. } | OpKind::CreateFile { .. });
        let is_rename = matches!(&kind, OpKind::Rename { .. });
        self.ops_ui.next_id += 1;
        let id = self.ops_ui.next_id;
        let touched_dirs = crate::core::dir_cache::affected_dirs(&kind);
        let job = OpJob {
            id,
            kind,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if let Some(ops_tx) = &self.ops_tx {
            let _ = ops_tx.send(OpJob {
                id,
                kind: job.kind,
                cancel: job.cancel.clone(),
            });
        }
        self.ops_ui.jobs.push(JobUi {
            id,
            verb,
            stage: OpStage::Scanning,
            file: PathBuf::new(),
            done_b: 0,
            total_b: 0,
            done_f: 0,
            total_f: 0,
            cancel: job.cancel,
            is_creation,
            is_rename,
            touched_dirs,
        });
    }

    pub(crate) fn open_create_dialog(&mut self, is_dir: bool) {
        self.ops_ui.create = Some((is_dir, String::new()));
        self.ops_ui.create_error = None;
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
        if self.shutdown_active() {
            return;
        }
        let paths = self.selected_paths();
        if !paths.is_empty() {
            if let Err(e) = crate::platform::set_clipboard_files(&paths) {
                tracing::debug!(error = %e, count = paths.len(), "failed to publish file clipboard data");
            }
            tracing::info!(count = paths.len(), "files copied to internal clipboard");
            self.ops_ui.clipboard = Some(Clipboard { paths, cut: false });
        } else {
            tracing::info!("copy requested with empty file selection");
        }
    }

    pub(crate) fn act_cut(&mut self) {
        if self.shutdown_active() {
            return;
        }
        let paths = self.selected_paths();
        if !paths.is_empty() {
            if let Err(e) = crate::platform::set_clipboard_files(&paths) {
                tracing::debug!(error = %e, count = paths.len(), "failed to publish file clipboard data");
            }
            self.ops_ui.clipboard = Some(Clipboard { paths, cut: true });
        }
    }

    pub(crate) fn act_paste(&mut self) {
        if self.shutdown_active() {
            return;
        }
        if self.tabs[self.active].archive_prefix.is_some() {
            return;
        }
        // Internal clipboard first (tracks cut-vs-copy), then Explorer's.
        if let Some(cb) = self.ops_ui.clipboard.clone() {
            let dest = self.tabs[self.active].path.clone();
            tracing::info!(
                count = cb.paths.len(),
                cut = cb.cut,
                "pasting internal file clipboard"
            );
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
        if self.shutdown_active() {
            return;
        }
        let paths = self.selected_paths();
        self.ops_ui.delete_confirm = delete_confirmation(paths, permanent);
    }

    pub(crate) fn act_rename(&mut self) {
        if self.shutdown_active() {
            return;
        }
        let tab = &self.tabs[self.active];
        if tab.archive_prefix.is_some() {
            return;
        }
        if tab.selection.len() == 1 {
            let selected_key = tab.selection.iter().next().unwrap();
            if let Some(entry) = tab.entries.iter().find(|entry| selected_key.matches(entry)) {
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
                OpEvent::Progress {
                    id,
                    stage,
                    file,
                    done_bytes,
                    total_bytes,
                    done_files,
                    total_files,
                } => {
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
                    let is_rename = self
                        .ops_ui
                        .jobs
                        .iter()
                        .any(|job| job.id == id && job.is_rename);
                    let reason = if is_rename {
                        apply_conflict_reason(existing_entry_kind(&dst))
                    } else {
                        "conflict.exists"
                    };
                    self.ops_ui.conflict = Some((id, src, dst));
                    self.ops_ui.conflict_reason = Some(reason);
                }
                OpEvent::AskError {
                    id,
                    path,
                    message,
                    kind,
                    retryable,
                } => {
                    let is_creation = self
                        .ops_ui
                        .jobs
                        .iter()
                        .any(|job| job.id == id && job.is_creation);
                    let existing = if is_creation
                        && matches!(kind, ErrorKind::PermissionDenied | ErrorKind::AlreadyExists)
                    {
                        existing_entry_kind(&path)
                    } else {
                        None
                    };
                    let reason_key = operation_error_key_for_context(kind, is_creation, existing);
                    let specific_reason =
                        (reason_key != operation_error_key(kind)).then_some(reason_key);
                    #[cfg(not(debug_assertions))]
                    let _ = message;
                    #[cfg(debug_assertions)]
                    {
                        let detail = if let Some(key) = specific_reason {
                            format!("{} [OS {kind:?}: {message}]", self.t(key))
                        } else {
                            format!("[{kind:?}]: {message}")
                        };
                        self.ops_ui
                            .journal
                            .push(format!("{}: {detail}", path.display()));
                    }
                    #[cfg(not(debug_assertions))]
                    {
                        let user_message = specific_reason
                            .map(|key| self.t(key).to_owned())
                            .unwrap_or_else(|| self.operation_error_message(kind));
                        self.ops_ui
                            .journal
                            .push(format!("{}: {user_message}", path.display()));
                    }
                    self.ops_ui.error_q = Some((id, path, kind, retryable));
                    self.ops_ui.error_reason = specific_reason;
                }
                OpEvent::Cancelled { id } => {
                    if let Some(pos) = self.ops_ui.jobs.iter().position(|job| job.id == id) {
                        self.ops_ui.jobs.remove(pos);
                    }
                }
                OpEvent::Done { id, failed } => {
                    let touched = self
                        .ops_ui
                        .jobs
                        .iter()
                        .find(|job| job.id == id)
                        .map(|job| job.touched_dirs.clone())
                        .unwrap_or_default();
                    if let Some(pos) = self.ops_ui.jobs.iter().position(|j| j.id == id) {
                        self.ops_ui.jobs.remove(pos);
                    }
                    if !self.shutdown_active() {
                        for path in touched {
                            self.dir_cache.invalidate_dir(&path);
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
    }

    pub(crate) fn reply_conflict(&self, id: u64, action: ConflictAction) {
        if let Some(reply_tx) = &self.op_reply {
            let _ = reply_tx.send(OpReply::Conflict { id, action });
        }
    }

    pub(crate) fn reply_error(&self, id: u64, action: ErrorAction) {
        if let Some(reply_tx) = &self.op_reply {
            let _ = reply_tx.send(OpReply::Error { id, action });
        }
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
        if self.shutdown_active() {
            self.conflict_dialog(ctx);
            self.error_dialog(ctx);
            return;
        }
        self.delete_dialog(ctx);
        self.delete_ask_dialog(ctx);
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
            self.t(if recycle {
                "dlg.delete_msg_recycle"
            } else {
                "dlg.delete_msg"
            }),
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
            match kind {
                OpKind::Recycle { sources } => self.dispatch_recycle(sources),
                other => self.enqueue("ops.deleting", other),
            }
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
        let Some((is_dir, original_name)) = self.ops_ui.create.clone() else {
            return;
        };
        let title = self
            .t(if is_dir {
                "dlg.create_folder_title"
            } else {
                "dlg.create_file_title"
            })
            .to_owned();
        let ok = self.t("dlg.ok").to_owned();
        let cancel = self.t("dlg.cancel").to_owned();
        let mut action = 0u8;
        let mut name = original_name.clone();
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut name).desired_width(280.0));
                if let Some(error_key) = self.ops_ui.create_error {
                    ui.colored_label(ui.visuals().error_fg_color, self.t(error_key).to_owned());
                }
                ui.horizontal(|ui| {
                    if ui.button(ok).clicked() {
                        action = 1;
                    }
                    if ui.button(cancel).clicked() {
                        action = 2;
                    }
                });
            });
        match action {
            1 => {
                let submission = creation_submission(
                    &self.tabs[self.active].path,
                    &name,
                    is_dir,
                    existing_entry_kind,
                );
                if let Some(kind) = apply_creation_submission(
                    &mut self.ops_ui.create,
                    &mut self.ops_ui.create_error,
                    submission,
                ) {
                    self.enqueue("ops.creating", kind);
                }
            }
            0 => {
                let trimmed = name.trim().to_owned();
                if trimmed != original_name {
                    self.ops_ui.create_error = None;
                }
                self.ops_ui.create = Some((is_dir, trimmed));
            }
            _ => {
                self.ops_ui.create = None;
                self.ops_ui.create_error = None;
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
            self.t(self.ops_ui.conflict_reason.unwrap_or("conflict.exists"),),
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
            self.ops_ui.conflict_reason = None;
        }
    }

    fn error_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, path, kind, _retryable)) = self.ops_ui.error_q.clone() else {
            return;
        };
        let title = self.t("err.title").to_owned();
        let msg = user_facing_operation_error(&path, kind, self.ops_ui.error_reason, |key| {
            self.t(key).to_owned()
        });
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
            self.ops_ui.error_reason = None;
        }
    }

    pub(crate) fn operation_error_message(&self, kind: ErrorKind) -> String {
        self.t(operation_error_key(kind)).to_owned()
    }

    pub(crate) fn record_io_error(&mut self, path: Option<&Path>, error: &std::io::Error) {
        let user_message = self.operation_error_message(error.kind());
        crate::core::logging::log_io_error("UI operation", None, path, error);
        let line = if let Some(path) = path {
            format!("{}: {user_message}", path.display())
        } else {
            user_message
        };
        #[cfg(debug_assertions)]
        let line = format!("{line}\n{:?}: {error}", error.kind());
        self.ops_ui.journal.push(line);
        self.ops_ui.show_journal = true;
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
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
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
    fn file_selection_resolves_stable_keys_and_ignores_removed_entries() {
        let entries = vec![entry("a.txt"), entry("b.txt"), entry("c.txt")];
        let stale = entry("removed.txt");
        let selection = std::collections::BTreeSet::from([
            SelectionKey::from_entry(&entries[0]),
            SelectionKey::from_entry(&entries[2]),
            SelectionKey::from_entry(&stale),
        ]);
        assert_eq!(
            paths_for_selection(&entries, &selection),
            vec![
                PathBuf::from("C:\\data\\a.txt"),
                PathBuf::from("C:\\data\\c.txt")
            ]
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
        assert_eq!(
            delete_confirmation(selected.clone(), false),
            Some((selected.clone(), true))
        );
        assert_eq!(
            delete_confirmation(selected.clone(), true),
            Some((selected.clone(), false))
        );

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
        assert_eq!(
            valid_creation_name("  отчёт 2026.txt  "),
            Some("отчёт 2026.txt")
        );
        assert_eq!(valid_creation_name("   "), None);
        for invalid in [
            "a\\b", "a/b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b",
        ] {
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
    fn text_file_creation_appends_txt_when_name_has_no_extension() {
        assert_eq!(
            creation_target(Path::new(r"C:\data"), "1", false),
            Some(PathBuf::from(r"C:\data\1.txt"))
        );
    }

    #[test]
    fn text_file_creation_does_not_double_txt_case_insensitive() {
        let parent = Path::new(r"C:\data");
        assert_eq!(
            creation_target(parent, "a.txt", false),
            Some(parent.join("a.txt"))
        );
        assert_eq!(
            creation_target(parent, "a.TXT", false),
            Some(parent.join("a.TXT"))
        );
    }

    #[test]
    fn text_file_creation_keeps_other_extension_as_typed() {
        let parent = Path::new(r"C:\data");
        assert_eq!(
            creation_target(parent, "a.md", false),
            Some(parent.join("a.md"))
        );
    }

    #[test]
    fn folder_creation_uses_name_as_typed_without_extension() {
        let parent = Path::new(r"C:\data");
        assert_eq!(
            creation_target(parent, "a.md", true),
            Some(parent.join("a.md"))
        );
    }

    fn assert_collision_keeps_creation_form(
        is_dir: bool,
        input: &str,
        target: &Path,
        existing: ExistingEntryKind,
        expected_key: &'static str,
    ) {
        let parent = Path::new(r"C:\data");
        let mut form = Some((is_dir, input.to_owned()));
        let mut error_key = None;
        let submission = creation_submission(parent, input, is_dir, |path| {
            assert_eq!(path, target);
            Some(existing)
        });
        assert!(apply_creation_submission(&mut form, &mut error_key, submission).is_none());
        assert_eq!(form, Some((is_dir, input.to_owned())));
        assert_eq!(error_key, Some(expected_key));
    }

    #[test]
    fn create_name_taken_by_folder_shows_folder_reason_and_keeps_form_open() {
        assert_collision_keeps_creation_form(
            false,
            "x",
            Path::new(r"C:\data\x.txt"),
            ExistingEntryKind::Folder,
            "error.name_taken_folder",
        );
    }

    #[test]
    fn create_name_taken_by_file_shows_file_reason_and_keeps_form_open() {
        assert_collision_keeps_creation_form(
            true,
            "x",
            Path::new(r"C:\data\x"),
            ExistingEntryKind::File,
            "error.name_taken_file",
        );
    }

    #[test]
    fn create_race_access_denied_on_directory_maps_to_name_taken_reason() {
        let key = operation_error_key_for_context(
            ErrorKind::PermissionDenied,
            true,
            Some(ExistingEntryKind::Folder),
        );
        assert_eq!(key, "error.name_taken_folder");
        assert_eq!(
            operation_error_key_for_context(
                ErrorKind::AlreadyExists,
                true,
                Some(ExistingEntryKind::File)
            ),
            "error.name_taken_file"
        );
        let shown = user_facing_operation_error(
            Path::new(r"C:\data\x"),
            ErrorKind::PermissionDenied,
            Some(key),
            |key| key.to_owned(),
        );
        assert!(shown.ends_with("error.name_taken_folder"));
    }

    #[test]
    fn genuine_permission_denied_still_shows_permissions_text() {
        use crate::core::i18n::{I18n, Language};

        assert_eq!(
            operation_error_key_for_context(ErrorKind::PermissionDenied, true, None),
            "error.operation.access_denied"
        );
        assert_eq!(
            operation_error_key_for_context(
                ErrorKind::PermissionDenied,
                false,
                Some(ExistingEntryKind::Folder)
            ),
            "error.operation.access_denied"
        );
        assert!(
            I18n::new(Language::Ru)
                .t("error.operation.access_denied")
                .contains("разрешения")
        );
    }

    #[test]
    fn create_submenu_localization_ru_en_complete() {
        use crate::core::i18n::{I18n, Language};

        let ru = I18n::new(Language::Ru);
        assert_eq!(ru.t("ctx.create"), "Создать");
        assert_eq!(ru.t("ctx.create_folder"), "Папка");
        assert_eq!(ru.t("ctx.create_text_file"), "Текстовый файл");
        assert_eq!(
            ru.t("error.name_taken_folder"),
            "Папка с таким именем уже существует."
        );
        assert_eq!(
            ru.t("error.name_taken_file"),
            "Файл с таким именем уже существует."
        );

        let en = I18n::new(Language::En);
        assert_eq!(en.t("ctx.create"), "Create");
        assert_eq!(en.t("ctx.create_folder"), "Folder");
        assert_eq!(en.t("ctx.create_text_file"), "Text file");
        assert_eq!(
            en.t("error.name_taken_folder"),
            "A folder with this name already exists."
        );
        assert_eq!(
            en.t("error.name_taken_file"),
            "A file with this name already exists."
        );
    }

    #[test]
    fn orphaned_old_create_menu_keys_removed_or_justified() {
        let empty_area_menu = include_str!("list/actions.rs")
            .split("fn empty_area_menu")
            .nth(1)
            .unwrap()
            .split("pub(super) fn entry_actions")
            .next()
            .unwrap();
        let toolbar_menu = include_str!("app/toolbar.rs");
        assert!(!empty_area_menu.contains("ctx.new_folder"));
        assert!(!empty_area_menu.contains("ctx.new_file"));
        assert!(toolbar_menu.contains("ctx.new_folder"));
        assert!(toolbar_menu.contains("ctx.new_file"));
        let dialogs = include_str!("opsui.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(dialogs.contains("dlg.create_folder_title"));
        assert!(dialogs.contains("dlg.create_file_title"));
    }

    #[test]
    fn rename_occupied_name_reason_consistent_with_create() {
        let reason = name_taken_error_key(ExistingEntryKind::Folder);
        assert_eq!(
            apply_conflict_reason(Some(ExistingEntryKind::Folder)),
            reason
        );
        let from = PathBuf::from(r"C:\data\old");
        assert_eq!(
            rename_target(&from, "new"),
            Some(PathBuf::from(r"C:\data\new"))
        );
    }

    #[test]
    fn existing_error_layer_and_ops_tests_not_regressed() {
        assert_eq!(
            operation_error_key(ErrorKind::PermissionDenied),
            "error.operation.access_denied"
        );
        assert_eq!(
            operation_error_key(ErrorKind::AlreadyExists),
            "error.operation.exists"
        );
        let worker = include_str!("../core/ops/worker/mod.rs");
        let mutations = include_str!("../core/ops/worker/mutations.rs");
        assert!(worker.contains("OpEvent::AskError"));
        assert!(mutations.contains("self.ask_error(job, path, &e)"));
        assert!(mutations.contains("self.failed += 1"));
        let ui = include_str!("opsui.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for action in [
            "ErrorAction::Retry",
            "ErrorAction::Skip",
            "ErrorAction::Cancel",
        ] {
            assert!(ui.contains(action));
        }
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
        for invalid in [
            r"C:\outside.txt",
            r"..\outside.txt",
            "../outside.txt",
            ".",
            "..",
        ] {
            assert_eq!(rename_target(&from, invalid), None);
        }
        assert!(matches!(
            rename_operation(from.clone(), " new name.txt "),
            Some(OpKind::Rename { from: old, to }) if old == from && to == PathBuf::from("C:\\data\\new name.txt")
        ));
    }

    #[test]
    fn user_facing_error_has_no_raw_io_error_or_stacktrace() {
        let path = Path::new(r"C:\data\file.txt");
        let error = std::io::Error::other("HRESULT 0x80070005\nstack trace: private detail");
        let shown = user_facing_operation_error(path, error.kind(), None, |key| match key {
            "error.operation.failed" => "Operation could not be completed.".to_owned(),
            _ => key.to_owned(),
        });
        assert!(shown.contains(path.to_string_lossy().as_ref()));
        assert!(shown.contains("Operation could not be completed."));
        assert!(!shown.contains(&error.to_string()));
        assert!(!shown.contains("HRESULT"));
        assert!(!shown.contains("stack trace"));
    }

    #[test]
    fn uncovered_error_kind_falls_back_to_neutral_text_not_raw_code() {
        let path = Path::new(r"C:\data\unknown.txt");
        let error = std::io::Error::from_raw_os_error(0x5A17);
        assert_eq!(operation_error_key(error.kind()), "error.operation.failed");
        let shown = user_facing_operation_error(path, error.kind(), None, |_| {
            "The operation could not be completed.".to_owned()
        });
        assert!(shown.contains(path.to_string_lossy().as_ref()));
        assert!(shown.contains("The operation could not be completed."));
        assert!(!shown.contains("0x5A17"));
        assert!(!shown.contains(&error.to_string()));
    }

    #[test]
    fn operation_error_keys_are_localized_in_both_languages() {
        let keys = [
            operation_error_key(std::io::ErrorKind::PermissionDenied),
            operation_error_key(std::io::ErrorKind::NotFound),
            operation_error_key(std::io::ErrorKind::AlreadyExists),
            operation_error_key(std::io::ErrorKind::StorageFull),
            operation_error_key(std::io::ErrorKind::WouldBlock),
            operation_error_key(std::io::ErrorKind::InvalidInput),
            operation_error_key(std::io::ErrorKind::NotADirectory),
            operation_error_key(std::io::ErrorKind::IsADirectory),
            operation_error_key(std::io::ErrorKind::DirectoryNotEmpty),
            operation_error_key(std::io::ErrorKind::FileTooLarge),
            operation_error_key(std::io::ErrorKind::Unsupported),
            operation_error_key(std::io::ErrorKind::Other),
            "error.panic.title",
            "error.panic.message",
            "settings.load_invalid",
            "settings.load_failed",
            "settings.save_failed",
        ];
        for json in [
            include_str!("../../i18n/en.json"),
            include_str!("../../i18n/ru.json"),
        ] {
            let labels: serde_json::Value = serde_json::from_str(json).unwrap();
            for key in keys {
                assert!(labels.get(key).is_some(), "missing localized key {key}");
            }
        }
    }
}
