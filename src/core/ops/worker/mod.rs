//! Выполнение очереди файловых операций.

mod copy_move;
mod mutations;

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use super::types::*;

static NEXT_COPY_TEMP_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_RENAME_BACKUP_ID: AtomicU64 = AtomicU64::new(0);

/// Spawns the operations worker thread.
/// Jobs: `jobs_rx`. Events to UI: `events_tx`. Decisions: `reply_rx`.
pub fn spawn(
    jobs_rx: Receiver<OpJob>,
    reply_rx: Receiver<OpReply>,
    events_tx: Sender<OpEvent>,
    platform: OpsPlatform,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(job) = jobs_rx.recv() {
            let mut w = Worker {
                reply_rx: &reply_rx,
                events: &events_tx,
                platform,
                overwrite_all: false,
                skip_all: false,
                failed: 0,
            };
            // Отменённая в FIFO-очереди операция не должна успеть изменить диск.
            if job.cancel.load(Ordering::Relaxed) {
                let _ = events_tx.send(OpEvent::Cancelled { id: job.id });
                continue;
            }
            w.run(&job);
            let _ = events_tx.send(OpEvent::Done {
                id: job.id,
                failed: w.failed,
            });
        }
    })
}

struct Worker<'a> {
    reply_rx: &'a Receiver<OpReply>,
    events: &'a Sender<OpEvent>,
    platform: OpsPlatform,
    overwrite_all: bool,
    skip_all: bool,
    failed: usize,
}

#[cfg(test)]
fn test_ops_platform() -> OpsPlatform {
    crate::platform::ops_platform()
}

impl<'a> Worker<'a> {
    fn run(&mut self, job: &OpJob) {
        if self.cancelled(job) {
            return;
        }
        let _ = match &job.kind {
            OpKind::Copy { sources, dest } => self.copy_many(job, sources, dest, false),
            OpKind::Move { sources, dest } => self.copy_many(job, sources, dest, true),
            OpKind::Delete { sources } => self.delete_many(job, sources),
            OpKind::Recycle { sources } => self.recycle_many(job, sources),
            OpKind::Rename { from, to } => self.rename(job, from, to),
            OpKind::CreateDir { path } => self.create(job, path, true),
            OpKind::CreateFile { path } => self.create(job, path, false),
        };
    }

    // ------------------------------------------------------------------
    // Question helpers — suspend the worker until the UI replies.
    // ------------------------------------------------------------------

    fn ask_conflict(&mut self, job: &OpJob, src: &Path, dst: &Path) -> ConflictAction {
        if self.overwrite_all {
            return ConflictAction::Overwrite;
        }
        if self.skip_all {
            return ConflictAction::Skip;
        }
        let _ = self.events.send(OpEvent::AskConflict {
            id: job.id,
            src: src.to_path_buf(),
            dst: dst.to_path_buf(),
        });
        loop {
            match self.reply_rx.recv() {
                Ok(OpReply::Conflict { id, action }) if id == job.id => match action {
                    ConflictAction::OverwriteAll => {
                        self.overwrite_all = true;
                        return ConflictAction::Overwrite;
                    }
                    ConflictAction::SkipAll => {
                        self.skip_all = true;
                        return ConflictAction::Skip;
                    }
                    _ => return action,
                },
                Ok(_) => continue, // answer to another question — ignore
                Err(_) => return ConflictAction::Cancel,
            }
        }
    }

    fn ask_error(&mut self, job: &OpJob, path: &Path, e: &io::Error) -> ErrorAction {
        let retryable = is_retryable(e);
        tracing::debug!(
            operation_id = job.id,
            path = %path.display(),
            error_kind = ?e.kind(),
            raw_os_error = ?e.raw_os_error(),
            error = %e,
            "file operation failed"
        );
        let _ = self.events.send(OpEvent::AskError {
            id: job.id,
            path: path.to_path_buf(),
            message: e.to_string(),
            kind: e.kind(),
            retryable,
        });
        loop {
            match self.reply_rx.recv() {
                Ok(OpReply::Error { id, action }) if id == job.id => return action,
                Ok(_) => continue,
                Err(_) => return ErrorAction::Cancel,
            }
        }
    }

    fn progress(
        &self,
        job: &OpJob,
        stage: OpStage,
        file: &Path,
        done_b: u64,
        total_b: u64,
        done_f: usize,
        total_f: usize,
    ) {
        let _ = self.events.send(OpEvent::Progress {
            id: job.id,
            stage,
            file: file.to_path_buf(),
            done_bytes: done_b,
            total_bytes: total_b,
            done_files: done_f,
            total_files: total_f,
        });
    }

    fn cancelled(&self, job: &OpJob) -> bool {
        job.cancel.load(Ordering::Relaxed)
    }
}

fn rename_with_overwrite(
    from: &Path,
    to: &Path,
    overwrite_confirmed: bool,
    platform: OpsPlatform,
) -> io::Result<()> {
    let from = long_path(from);
    let to = long_path(to);
    if !to.exists() {
        return fs::rename(&from, &to);
    }
    if !overwrite_confirmed {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination appeared before rename",
        ));
    }

    let from_meta = fs::symlink_metadata(&from)?;
    let to_meta = fs::symlink_metadata(&to)?;
    if from_meta.is_file() && to_meta.is_file() {
        return platform.replace_file(&from, &to);
    }

    let backup_dir = create_rename_backup_dir(&to)?;
    let backup_target = long_path(&backup_dir.join("previous"));
    if let Err(error) = fs::rename(&to, &backup_target) {
        let _ = fs::remove_dir(&backup_dir);
        return Err(error);
    }

    match fs::rename(&from, &to) {
        Ok(()) => {
            let _ = remove_any(&backup_target);
            let _ = fs::remove_dir(&backup_dir);
            Ok(())
        }
        Err(rename_error) => match fs::rename(&backup_target, &to) {
            Ok(()) => {
                let _ = fs::remove_dir(&backup_dir);
                Err(rename_error)
            }
            Err(restore_error) => Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "rename failed: {rename_error}; previous destination retained at {} because restore failed: {restore_error}",
                    backup_target.display()
                ),
            )),
        },
    }
}

fn create_rename_backup_dir(destination: &Path) -> io::Result<PathBuf> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for _ in 0..128 {
        let id = NEXT_RENAME_BACKUP_ID.fetch_add(1, Ordering::Relaxed);
        let backup_dir =
            long_path(&parent.join(format!(".filemanager-rename-{}-{id}", std::process::id())));
        match fs::create_dir(&backup_dir) {
            Ok(()) => return Ok(backup_dir),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a rename backup directory",
    ))
}

// ----------------------------------------------------------------------
// Free helpers
// ----------------------------------------------------------------------

/// Totals `size` bytes / `count` files under `root` (files + dirs counted).
fn scan(root: &Path, bytes: &mut u64, count: &mut usize, job: &OpJob) -> io::Result<()> {
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let meta = fs::symlink_metadata(root)?;
    if meta.is_dir() {
        for item in fs::read_dir(root)? {
            let item = item?;
            scan(&item.path(), bytes, count, job)?;
        }
    } else {
        *bytes += meta.len();
        *count += 1;
    }
    Ok(())
}

/// Streams `src` to a sibling temporary file, then replaces `dst` only after
/// the copy is complete. Cancellation and I/O errors leave the old destination
/// untouched.
fn copy_file_stream(
    src: &Path,
    dst: &Path,
    on_chunk: &mut dyn FnMut(u64),
    platform: OpsPlatform,
    job: &OpJob,
) -> io::Result<()> {
    let dst = long_path(dst);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let (temp_path, temp_file) = create_copy_temp(&dst)?;
    let copy_result = (|| {
        let mut reader = io::BufReader::with_capacity(1 << 20, fs::File::open(long_path(src))?);
        let mut writer = io::BufWriter::with_capacity(1 << 20, temp_file);
        let mut buf = vec![0u8; 1 << 16];
        loop {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok::<_, io::Error>(false);
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            on_chunk(n as u64);
        }
        writer.flush()?;
        let src_len = fs::metadata(long_path(src))?.len();
        let dst_len = writer.get_ref().metadata()?.len();
        if src_len != dst_len {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "size mismatch after copy",
            ));
        }
        if let Ok(mtime) = fs::metadata(long_path(src)).and_then(|m| m.modified()) {
            let _ = writer.get_ref().set_modified(mtime);
        }
        writer.get_ref().sync_all()?;
        Ok(true)
    })();

    match copy_result {
        Ok(true) if job.cancel.load(Ordering::Relaxed) => {
            let _ = fs::remove_file(&temp_path);
            Ok(())
        }
        Ok(true) => {
            if let Err(error) = platform.replace_file(&temp_path, &dst) {
                let _ = fs::remove_file(&temp_path);
                return Err(error);
            }
            Ok(())
        }
        Ok(false) => {
            let _ = fs::remove_file(&temp_path);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(error)
        }
    }
}

fn create_copy_temp(dst: &Path) -> io::Result<(PathBuf, fs::File)> {
    let parent = dst
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for _ in 0..128 {
        let id = NEXT_COPY_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            long_path(&parent.join(format!(".filemanager-copy-{}-{id}.tmp", std::process::id())));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a temporary copy file",
    ))
}

fn remove_any(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(long_path(path))
    } else {
        fs::remove_file(long_path(path))
    }
}

/// A move fallback may remove its source only when copying completed and the
/// destination exists. Destination existence alone is not evidence of success:
/// a partial tree or a skipped overwrite can leave it behind.
fn remove_move_source_after_copy(
    src: &Path,
    dst: &Path,
    copy_complete: bool,
    cancelled: bool,
) -> io::Result<bool> {
    if !copy_complete || cancelled || !dst.exists() {
        return Ok(false);
    }
    remove_any(src)?;
    Ok(true)
}

/// Windows: a locked file is worth a Retry; transient sharing errors too.
fn is_retryable(e: &io::Error) -> bool {
    match e.raw_os_error() {
        // ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33), ACCESS_DENIED (5)
        Some(32) | Some(33) | Some(5) => true,
        _ => matches!(
            e.kind(),
            io::ErrorKind::PermissionDenied | io::ErrorKind::Interrupted
        ),
    }
}

/// `EXDEV` — cross-device link; on Windows rename returns other errors too,
/// so treat any rename failure as potentially cross-volume (the fallback is
/// still correct: copy + verify + delete).
fn is_cross_volume(_e: &io::Error) -> bool {
    true
}

/// First free sibling name for same-folder pastes:
/// `name — копия.ext`, then `name — копия (2).ext`, etc.
fn copy_variant(dst: &Path) -> PathBuf {
    let stem = dst.file_stem().unwrap_or_default().to_string_lossy();
    let ext = dst
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..=999u32 {
        let suffix = if n == 1 {
            format!(" — копия{ext}")
        } else {
            format!(" — копия ({n}){ext}")
        };
        let cand = dst.with_file_name(format!("{stem}{suffix}"));
        if !cand.exists() {
            return cand;
        }
    }
    dst.to_path_buf()
}

fn same_existing_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

fn destination_is_inside_directory(source: &Path, destination: &Path) -> bool {
    let source = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let destination = fs::canonicalize(destination).unwrap_or_else(|_| destination.to_path_buf());
    destination == source || destination.starts_with(&source)
}

/// Prefixes paths longer than ~240 chars with `\\?\` for Windows long paths.
#[cfg(windows)]
fn long_path(p: &Path) -> PathBuf {
    let s = p.as_os_str();
    if s.to_string_lossy().len() > 240 && !s.to_string_lossy().starts_with(r"\\?\") {
        PathBuf::from(format!(r"\\?\{}", s.to_string_lossy()))
    } else {
        p.to_path_buf()
    }
}

#[cfg(not(windows))]
fn long_path(p: &Path) -> PathBuf {
    p.to_path_buf()
}

#[cfg(test)]
#[path = "tests/tests.rs"]
mod tests;
#[cfg(test)]
#[path = "tests/tests_acc04.rs"]
mod tests_acc04;
#[cfg(test)]
#[path = "tests/tests_copy_destination.rs"]
mod tests_copy_destination;
#[cfg(test)]
#[path = "tests/tests_extra.rs"]
mod tests_extra;
#[cfg(test)]
#[path = "tests/tests_move_safety.rs"]
mod tests_move_safety;
#[cfg(test)]
#[path = "tests/tests_operation_decisions.rs"]
mod tests_operation_decisions;
#[cfg(test)]
#[path = "tests/tests_rename_safety.rs"]
mod tests_rename_safety;
#[cfg(test)]
#[path = "tests/tests_samefolder.rs"]
mod tests_samefolder;
#[cfg(test)]
#[path = "tests/tests_v02lp.rs"]
mod tests_v02lp;
#[cfg(test)]
#[path = "tests/tests_v04.rs"]
mod tests_v04;
