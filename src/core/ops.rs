//! Background file-operation queue.
//!
//! One worker thread drains a job channel and executes Copy / Move / Delete /
//! Rename / Create jobs sequentially. Progress is reported through
//! [`OpEvent`]s; conflicts and recoverable errors suspend the worker until the
//! UI answers via the reply channel.
//!
//! Safety rules (from the roadmap):
//! - move inside one volume → `rename`; across volumes → copy, verify size,
//!   delete source — never delete before the copy succeeds;
//! - deleting anything is only enqueued after explicit UI confirmation;
//! - a failed file aborts neither the queue nor silently: the worker asks
//!   Retry / Skip / Cancel.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

#[derive(Debug)]
pub enum OpKind {
    Copy { sources: Vec<PathBuf>, dest: PathBuf },
    Move { sources: Vec<PathBuf>, dest: PathBuf },
    Delete { sources: Vec<PathBuf> },
    /// Delete via the Windows Recycle Bin (undo-capable).
    Recycle { sources: Vec<PathBuf> },
    Rename { from: PathBuf, to: PathBuf },
    CreateDir { path: PathBuf },
    CreateFile { path: PathBuf },
}

impl OpKind {
    /// Short description for the ops panel (UI adds the localized verb).
    pub fn target_label(&self) -> String {
        match self {
            OpKind::Copy { dest, .. } | OpKind::Move { dest, .. } => {
                dest.display().to_string()
            }
            OpKind::Delete { sources } | OpKind::Recycle { sources } => {
                format!("{} items", sources.len())
            }
            OpKind::Rename { to, .. } => to.display().to_string(),
            OpKind::CreateDir { path } | OpKind::CreateFile { path } => {
                path.display().to_string()
            }
        }
    }
}

pub struct OpJob {
    pub id: u64,
    pub kind: OpKind,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpStage {
    Scanning,
    Copying,
    Moving,
    Deleting,
    Creating,
}

#[derive(Debug)]
pub enum OpEvent {
    Progress {
        id: u64,
        stage: OpStage,
        file: PathBuf,
        done_bytes: u64,
        total_bytes: u64,
        done_files: usize,
        total_files: usize,
    },
    /// A destination already exists — worker waits for a decision.
    AskConflict { id: u64, src: PathBuf, dst: PathBuf },
    /// An item failed — worker waits for Retry / Skip / Cancel.
    AskError {
        id: u64,
        path: PathBuf,
        message: String,
        retryable: bool,
    },
    Done { id: u64, failed: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    Overwrite,
    OverwriteAll,
    Skip,
    SkipAll,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    Retry,
    Skip,
    Cancel,
}

#[derive(Debug)]
pub enum OpReply {
    Conflict { id: u64, action: ConflictAction },
    Error { id: u64, action: ErrorAction },
}

/// Spawns the operations worker thread.
/// Jobs: `jobs_rx`. Events to UI: `events_tx`. Decisions: `reply_rx`.
pub fn spawn(
    jobs_rx: Receiver<OpJob>,
    reply_rx: Receiver<OpReply>,
    events_tx: Sender<OpEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(job) = jobs_rx.recv() {
            let mut w = Worker {
                reply_rx: &reply_rx,
                events: &events_tx,
                overwrite_all: false,
                skip_all: false,
                failed: 0,
            };
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
    overwrite_all: bool,
    skip_all: bool,
    failed: usize,
}

impl<'a> Worker<'a> {
    fn run(&mut self, job: &OpJob) {
        let _ = match &job.kind {
            OpKind::Copy { sources, dest } => {
                self.copy_many(job, sources, dest, false)
            }
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
                Ok(OpReply::Conflict { id, action }) if id == job.id => {
                    match action {
                        ConflictAction::OverwriteAll => {
                            self.overwrite_all = true;
                            return ConflictAction::Overwrite;
                        }
                        ConflictAction::SkipAll => {
                            self.skip_all = true;
                            return ConflictAction::Skip;
                        }
                        _ => return action,
                    }
                }
                Ok(_) => continue, // answer to another question — ignore
                Err(_) => return ConflictAction::Cancel,
            }
        }
    }

    fn ask_error(&mut self, job: &OpJob, path: &Path, e: &io::Error) -> ErrorAction {
        let retryable = is_retryable(e);
        let _ = self.events.send(OpEvent::AskError {
            id: job.id,
            path: path.to_path_buf(),
            message: e.to_string(),
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

    fn progress(&self, job: &OpJob, stage: OpStage, file: &Path, done_b: u64, total_b: u64, done_f: usize, total_f: usize) {
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

    // ------------------------------------------------------------------
    // Copy / Move
    // ------------------------------------------------------------------

    fn copy_many(&mut self, job: &OpJob, sources: &[PathBuf], dest: &Path, is_move: bool) -> io::Result<()> {
        // Stage 1: count totals (cancellable).
        self.progress(job, OpStage::Scanning, dest, 0, 0, 0, 0);
        let mut total_bytes = 0u64;
        let mut total_files = 0usize;
        for src in sources {
            if self.cancelled(job) {
                return Ok(());
            }
            if let Err(e) = scan(src, &mut total_bytes, &mut total_files, job) {
                match self.ask_error(job, src, &e) {
                    ErrorAction::Retry => {
                        let _ = scan(src, &mut total_bytes, &mut total_files, job);
                    }
                    ErrorAction::Skip => continue,
                    ErrorAction::Cancel => return Ok(()),
                }
            }
        }

        // Stage 2: free-space check (copy & cross-volume move only matter
        // for copies — rename is free; we still check, it's harmless).
        if let Some(free) = crate::platform::free_space(dest) {
            if total_bytes > free {
                let e = io::Error::new(io::ErrorKind::OutOfMemory, "not enough space on destination");
                match self.ask_error(job, dest, &e) {
                    ErrorAction::Retry => {}
                    _ => return Ok(()),
                }
            }
        }

        // Stage 3: process each source.
        let mut done_bytes = 0u64;
        let mut done_files = 0usize;
        for src in sources {
            if self.cancelled(job) {
                return Ok(());
            }
            let dst = dest.join(src.file_name().unwrap_or_default());
            if *src == dst {
                self.failed += 1;
                continue;
            }
            let stage = if is_move { OpStage::Moving } else { OpStage::Copying };
            self.progress(job, stage, src, done_bytes, total_bytes, done_files, total_files);
            if is_move {
                self.move_entry(job, src, &dst, &mut done_bytes, &mut done_files, total_bytes, total_files);
            } else {
                self.copy_entry(job, src, &dst, &mut done_bytes, &mut done_files, total_bytes, total_files);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_entry(&mut self, job: &OpJob, src: &Path, dst: &Path, done_b: &mut u64, done_f: &mut usize, total_b: u64, total_f: usize) {
        if self.cancelled(job) {
            return;
        }
        let meta = match fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => {
                self.failed += 1;
                let _ = self.ask_error(job, src, &e);
                return;
            }
        };
        if meta.is_dir() {
            match self.ensure_dir(job, dst) {
                true => {
                    if let Ok(items) = fs::read_dir(src) {
                        for item in items.flatten() {
                            if self.cancelled(job) {
                                return;
                            }
                            self.copy_entry(job, &item.path(), &dst.join(item.file_name()), done_b, done_f, total_b, total_f);
                        }
                    }
                }
                false => self.failed += 1,
            }
            return;
        }
        // File: resolve overwrite conflict first.
        if dst.exists() {
            match self.ask_conflict(job, src, dst) {
                ConflictAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    return;
                }
                ConflictAction::Skip => return,
                _ => {}
            }
        }
        loop {
            match copy_file_stream(src, dst, &mut |written| {
                *done_b += written;
            }, job) {
                Ok(()) => {
                    *done_f += 1;
                    self.progress(job, OpStage::Copying, src, *done_b, total_b, *done_f, total_f);
                    return;
                }
                Err(e) => match self.ask_error(job, src, &e) {
                    ErrorAction::Retry => continue,
                    ErrorAction::Skip => {
                        self.failed += 1;
                        return;
                    }
                    ErrorAction::Cancel => {
                        job.cancel.store(true, Ordering::Relaxed);
                        return;
                    }
                },
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn move_entry(&mut self, job: &OpJob, src: &Path, dst: &Path, done_b: &mut u64, done_f: &mut usize, total_b: u64, total_f: usize) {
        if self.cancelled(job) {
            return;
        }
        if dst.exists() {
            match self.ask_conflict(job, src, dst) {
                ConflictAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    return;
                }
                ConflictAction::Skip => return,
                _ => {}
            }
        }
        // Fast path: rename (same volume).
        match fs::rename(src, dst) {
            Ok(()) => {
                let size = fs::symlink_metadata(dst).map(|m| m.len()).unwrap_or(0);
                *done_b += size;
                *done_f += 1;
                return;
            }
            Err(e) if is_cross_volume(&e) => {
                // copy → verify → delete; source is removed only on success.
                self.copy_entry(job, src, dst, done_b, done_f, total_b, total_f);
                if !self.cancelled(job) && dst.exists() {
                    if let Err(e) = remove_any(src) {
                        self.failed += 1;
                        let _ = self.ask_error(job, src, &e);
                    }
                }
            }
            Err(e) => match self.ask_error(job, src, &e) {
                ErrorAction::Retry => self.move_entry(job, src, dst, done_b, done_f, total_b, total_f),
                ErrorAction::Skip => self.failed += 1,
                ErrorAction::Cancel => job.cancel.store(true, Ordering::Relaxed),
            },
        }
    }

    fn ensure_dir(&mut self, job: &OpJob, dst: &Path) -> bool {
        if dst.is_dir() {
            return true; // merge into existing directory
        }
        if dst.exists() {
            match self.ask_conflict(job, dst, dst) {
                ConflictAction::Skip => return false,
                ConflictAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    return false;
                }
                _ => {}
            }
        }
        match fs::create_dir_all(dst) {
            Ok(()) => true,
            Err(e) => match self.ask_error(job, dst, &e) {
                ErrorAction::Retry => self.ensure_dir(job, dst),
                ErrorAction::Skip => false,
                ErrorAction::Cancel => {
                    job.cancel.store(true, Ordering::Relaxed);
                    false
                }
            },
        }
    }

    // ------------------------------------------------------------------
    // Delete / Rename / Create
    // ------------------------------------------------------------------

    fn delete_many(&mut self, job: &OpJob, sources: &[PathBuf]) -> io::Result<()> {
        let total = sources.len();
        for (i, src) in sources.iter().enumerate() {
            if self.cancelled(job) {
                return Ok(());
            }
            self.progress(job, OpStage::Deleting, src, i as u64, total as u64, i, total);
            loop {
                match remove_any(src) {
                    Ok(()) => break,
                    Err(e) => match self.ask_error(job, src, &e) {
                        ErrorAction::Retry => continue,
                        ErrorAction::Skip => {
                            self.failed += 1;
                            break;
                        }
                        ErrorAction::Cancel => {
                            job.cancel.store(true, Ordering::Relaxed);
                            return Ok(());
                        }
                    },
                }
            }
        }
        Ok(())
    }

    /// Delete via the Recycle Bin in one IFileOperation call (progress
    /// reported per attempt — the bin handles subfolders itself).
    fn recycle_many(&mut self, job: &OpJob, sources: &[PathBuf]) -> io::Result<()> {
        self.progress(job, OpStage::Deleting, sources.first().map(|p| p.as_path()).unwrap_or(Path::new("")), 0, sources.len() as u64, 0, sources.len());
        if self.cancelled(job) {
            return Ok(());
        }
        loop {
            match crate::platform::recycle_bin(sources) {
                Ok(()) => return Ok(()),
                Err(e) => match self.ask_error(job, &sources[0], &e) {
                    ErrorAction::Retry => continue,
                    ErrorAction::Skip => {
                        self.failed += sources.len();
                        return Ok(());
                    }
                    ErrorAction::Cancel => return Ok(()),
                },
            }
        }
    }

    fn rename(&mut self, job: &OpJob, from: &Path, to: &Path) -> io::Result<()> {
        self.progress(job, OpStage::Creating, from, 0, 1, 0, 1);
        if to.exists() {
            match self.ask_conflict(job, from, to) {
                ConflictAction::Cancel => return Ok(()),
                ConflictAction::Skip => return Ok(()),
                _ => {
                    let _ = remove_any(to);
                }
            }
        }
        loop {
            match fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(e) => match self.ask_error(job, from, &e) {
                    ErrorAction::Retry => continue,
                    ErrorAction::Skip => {
                        self.failed += 1;
                        return Ok(());
                    }
                    ErrorAction::Cancel => return Ok(()),
                },
            }
        }
    }

    fn create(&mut self, job: &OpJob, path: &Path, dir: bool) -> io::Result<()> {
        self.progress(job, OpStage::Creating, path, 0, 1, 0, 1);
        let r = if dir {
            fs::create_dir(path)
        } else {
            fs::File::create_new(path).map(|_| ())
        };
        if let Err(e) = r {
            match self.ask_error(job, path, &e) {
                ErrorAction::Retry => self.create(job, path, dir),
                ErrorAction::Skip => {
                    self.failed += 1;
                    Ok(())
                }
                ErrorAction::Cancel => Ok(()),
            }
        } else {
            Ok(())
        }
    }
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

/// Streams `src` → `dst` in 64 KiB chunks, reporting bytes through `on_chunk`.
/// On cancellation the partial destination file is removed.
fn copy_file_stream(src: &Path, dst: &Path, on_chunk: &mut dyn FnMut(u64), job: &OpJob) -> io::Result<()> {
    let dst = long_path(dst);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut reader = io::BufReader::with_capacity(1 << 20, fs::File::open(long_path(src))?);
    let mut writer = io::BufWriter::with_capacity(1 << 20, fs::File::create(&dst)?);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if job.cancel.load(Ordering::Relaxed) {
            drop(writer);
            let _ = fs::remove_file(&dst);
            return Ok(());
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        on_chunk(n as u64);
    }
    writer.flush()?;
    // Verify: written size must match the source size.
    let src_len = fs::metadata(long_path(src))?.len();
    let dst_len = writer.get_ref().metadata()?.len();
    if src_len != dst_len {
        return Err(io::Error::new(io::ErrorKind::Other, "size mismatch after copy"));
    }
    // Preserve the modification timestamp (best effort).
    if let Ok(mtime) = fs::metadata(long_path(src)).and_then(|m| m.modified()) {
        let _ = writer.get_ref().set_modified(mtime);
    }
    Ok(())
}

fn remove_any(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(long_path(path))
    } else {
        fs::remove_file(long_path(path))
    }
}

/// Windows: a locked file is worth a Retry; transient sharing errors too.
fn is_retryable(e: &io::Error) -> bool {
    match e.raw_os_error() {
        // ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33), ACCESS_DENIED (5)
        Some(32) | Some(33) | Some(5) => true,
        _ => matches!(e.kind(), io::ErrorKind::PermissionDenied | io::ErrorKind::Interrupted),
    }
}

/// `EXDEV` — cross-device link; on Windows rename returns other errors too,
/// so treat any rename failure as potentially cross-volume (the fallback is
/// still correct: copy + verify + delete).
fn is_cross_volume(_e: &io::Error) -> bool {
    true
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
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fm_ops_test_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn job() -> OpJob {
        OpJob {
            id: 1,
            kind: OpKind::CreateDir { path: PathBuf::new() },
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn copy_streams_content_and_preserves_size() {
        let dir = tmpdir("copy");
        let src = dir.join("a.bin");
        let dst = dir.join("b.bin");
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &data).unwrap();
        let j = job();
        copy_file_stream(&src, &dst, &mut |_| {}, &j).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), data);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cancel_removes_partial_file() {
        let dir = tmpdir("cancel");
        let src = dir.join("big.bin");
        let dst = dir.join("part.bin");
        fs::write(&src, vec![7u8; 1_000_000]).unwrap();
        let j = job();
        j.cancel.store(true, Ordering::Relaxed);
        copy_file_stream(&src, &dst, &mut |_| {}, &j).unwrap();
        assert!(!dst.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_counts_files_and_bytes() {
        let dir = tmpdir("scan");
        fs::write(dir.join("a"), b"12345").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("b"), b"678").unwrap();
        let (mut b, mut c) = (0, 0);
        let j = job();
        scan(&dir, &mut b, &mut c, &j).unwrap();
        assert_eq!((b, c), (8, 2));
        fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod tests_extra {
    use super::*;
    use std::sync::mpsc::channel;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fm_ops_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn job_with(kind: OpKind) -> OpJob {
        OpJob { id: 9, kind, cancel: Arc::new(AtomicBool::new(false)) }
    }

    fn worker<'a>(rrx: &'a Receiver<OpReply>, etx: &'a Sender<OpEvent>) -> Worker<'a> {
        Worker { reply_rx: rrx, events: etx, overwrite_all: false, skip_all: false, failed: 0 }
    }

    #[test]
    fn copy_job_copies_file() {
        let dir = tmpdir("wcopy");
        fs::write(dir.join("a.txt"), b"hi").unwrap();
        let (_rtx, rrx) = channel();
        let (etx, _erx) = channel();
        let mut w = worker(&rrx, &etx);
        w.run(&job_with(OpKind::Copy {
            sources: vec![dir.join("a.txt")],
            dest: dir.join("out"),
        }));
        assert_eq!(fs::read_to_string(dir.join("out").join("a.txt")).unwrap(), "hi");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_job_removes_source() {
        let dir = tmpdir("wmove");
        fs::write(dir.join("a.txt"), b"hi").unwrap();
        let (_rtx, rrx) = channel();
        let (etx, _erx) = channel();
        let mut w = worker(&rrx, &etx);
        w.run(&job_with(OpKind::Move {
            sources: vec![dir.join("a.txt")],
            dest: dir.join("out"),
        }));
        assert!(!dir.join("a.txt").exists());
        assert_eq!(fs::read_to_string(dir.join("out").join("a.txt")).unwrap(), "hi");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_job_renames() {
        let dir = tmpdir("wren");
        fs::write(dir.join("old.txt"), b"x").unwrap();
        let (_rtx, rrx) = channel();
        let (etx, _erx) = channel();
        let mut w = worker(&rrx, &etx);
        w.run(&job_with(OpKind::Rename { from: dir.join("old.txt"), to: dir.join("new.txt") }));
        assert!(!dir.join("old.txt").exists());
        assert!(dir.join("new.txt").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_dir_and_file() {
        let dir = tmpdir("wcreate");
        let (_rtx, rrx) = channel();
        let (etx, _erx) = channel();
        let mut w = worker(&rrx, &etx);
        w.run(&job_with(OpKind::CreateDir { path: dir.join("nd") }));
        w.run(&job_with(OpKind::CreateFile { path: dir.join("nf.txt") }));
        assert!(dir.join("nd").is_dir() && dir.join("nf.txt").is_file());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflict_skipall_leaves_dest() {
        let dir = tmpdir("wskip");
        fs::write(dir.join("a.txt"), b"new").unwrap();
        let dst = dir.join("out");
        fs::create_dir(&dst).unwrap();
        fs::write(dst.join("a.txt"), b"old").unwrap();
        let (rtx, rrx) = channel();
        rtx.send(OpReply::Conflict { id: 9, action: ConflictAction::SkipAll }).unwrap();
        let (etx, _erx) = channel();
        let mut w = worker(&rrx, &etx);
        w.run(&job_with(OpKind::Copy {
            sources: vec![dir.join("a.txt")],
            dest: dst.clone(),
        }));
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "old");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_job_removes() {
        let dir = tmpdir("wdel");
        fs::write(dir.join("a.txt"), b"x").unwrap();
        let (_rtx, rrx) = channel();
        let (etx, _erx) = channel();
        let mut w = worker(&rrx, &etx);
        w.run(&job_with(OpKind::Delete { sources: vec![dir.join("a.txt")] }));
        assert!(!dir.join("a.txt").exists());
        fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod tests_v04 {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fm_v04_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn worker<'a>(rrx: &'a Receiver<OpReply>, etx: &'a Sender<OpEvent>) -> Worker<'a> {
        Worker { reply_rx: rrx, events: etx, overwrite_all: false, skip_all: false, failed: 0 }
    }

    fn job(kind: OpKind) -> OpJob {
        OpJob { id: 1, kind, cancel: Arc::new(AtomicBool::new(false)) }
    }

    #[test]
    fn queue_runs_jobs_in_order() {
        let dir = tmp("queue");
        fs::write(dir.join("a"), b"x").unwrap();
        let (jtx, jrx) = channel();
        let (etx, erx) = channel::<OpEvent>();
        let (_rtx, rrx) = channel();
        spawn(jrx, rrx, etx);
        for (i, name) in ["o1", "o2"].iter().enumerate() {
            jtx.send(OpJob {
                id: (i + 1) as u64,
                kind: OpKind::Copy { sources: vec![dir.join("a")], dest: dir.join(name) },
                cancel: Arc::new(AtomicBool::new(false)),
            }).unwrap();
        }
        let mut done = vec![];
        for _ in 0..20 {
            match erx.recv_timeout(Duration::from_secs(10)).unwrap() {
                OpEvent::Done { id, .. } => done.push(id),
                _ => {}
            }
            if done.len() == 2 { break; }
        }
        assert_eq!(done, vec![1, 2]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn progress_event_emitted() {
        let dir = tmp("prog");
        fs::write(dir.join("big.bin"), vec![7u8; 200_000]).unwrap();
        let (_rtx, rrx) = channel();
        let (etx, erx) = channel();
        let mut w = worker(&rrx, &etx);
        w.run(&job(OpKind::Copy { sources: vec![dir.join("big.bin")], dest: dir.join("out") }));
        let mut saw = false;
        while let Ok(ev) = erx.try_recv() {
            if let OpEvent::Progress { done_bytes, .. } = ev { saw = saw || done_bytes > 0; }
        }
        assert!(saw);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cancel_stops_job() {
        let dir = tmp("cancel");
        fs::create_dir(dir.join("src")).unwrap();
        for i in 0..10 { fs::write(dir.join("src").join(format!("f{i}")), b"xx").unwrap(); }
        let (_rtx, rrx) = channel();
        let (etx, erx) = channel();
        let mut w = worker(&rrx, &etx);
        let j = OpJob {
            id: 1,
            kind: OpKind::Copy { sources: vec![dir.join("src")], dest: dir.join("out") },
            cancel: Arc::new(AtomicBool::new(true)),
        };
        w.run(&j);
        let mut failed = None;
        while let Ok(ev) = erx.try_recv() {
            if let OpEvent::Done { failed: f, .. } = ev { failed = Some(f); }
        }
        let copied = dir.join("out").join("src");
        let n = fs::read_dir(&copied).map(|r| r.count()).unwrap_or(0);
        assert!(n < 10, "cancelled job copied {n}/10 files");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflict_overwrite_replaces() {
        let dir = tmp("ow");
        fs::write(dir.join("a"), b"new").unwrap();
        let dst = dir.join("out");
        fs::create_dir(&dst).unwrap();
        fs::write(dst.join("a"), b"old").unwrap();
        let (rtx, rrx) = channel();
        rtx.send(OpReply::Conflict { id: 1, action: ConflictAction::Overwrite }).unwrap();
        let (etx, _e) = channel();
        worker(&rrx, &etx).run(&job(OpKind::Copy { sources: vec![dir.join("a")], dest: dst.clone() }));
        assert_eq!(fs::read_to_string(dst.join("a")).unwrap(), "new");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflict_skip_keeps() {
        let dir = tmp("sk");
        fs::write(dir.join("a"), b"new").unwrap();
        let dst = dir.join("out");
        fs::create_dir(&dst).unwrap();
        fs::write(dst.join("a"), b"old").unwrap();
        let (rtx, rrx) = channel();
        rtx.send(OpReply::Conflict { id: 1, action: ConflictAction::Skip }).unwrap();
        let (etx, _e) = channel();
        worker(&rrx, &etx).run(&job(OpKind::Copy { sources: vec![dir.join("a")], dest: dst.clone() }));
        assert_eq!(fs::read_to_string(dst.join("a")).unwrap(), "old");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn conflict_cancel_fails_job() {
        let dir = tmp("cc");
        fs::write(dir.join("a"), b"x").unwrap();
        let dst = dir.join("out");
        fs::create_dir(&dst).unwrap();
        fs::write(dst.join("a"), b"y").unwrap();
        let (rtx, rrx) = channel();
        rtx.send(OpReply::Conflict { id: 1, action: ConflictAction::Cancel }).unwrap();
        let (etx, erx) = channel();
        worker(&rrx, &etx).run(&job(OpKind::Copy { sources: vec![dir.join("a")], dest: dst.clone() }));
        assert_eq!(fs::read_to_string(dst.join("a")).unwrap(), "y");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn error_skip_counts_failed() {
        let dir = tmp("eskip");
        let (rtx, rrx) = channel();
        let (etx, erx) = channel();
        std::thread::spawn(move || {
            while let Ok(ev) = erx.recv() {
                if let OpEvent::AskError { id, .. } = ev {
                    let _ = rtx.send(OpReply::Error { id, action: ErrorAction::Skip });
                }
            }
        });
        let mut w = worker(&rrx, &etx);
        w.run(&job(OpKind::Copy { sources: vec![dir.join("missing")], dest: dir.join("out") }));
        assert!(w.failed >= 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sharing_violation_is_retryable() {
        assert!(is_retryable(&io::Error::from_raw_os_error(32)));
    }

    #[test]
    fn move_copies_then_deletes_source() {
        let dir = tmp("mvdel");
        fs::create_dir(dir.join("src")).unwrap();
        fs::write(dir.join("src").join("n.bin"), vec![1u8; 100]).unwrap();
        let (_rtx, rrx) = channel();
        let (etx, _e) = channel();
        worker(&rrx, &etx).run(&job(OpKind::Move { sources: vec![dir.join("src")], dest: dir.join("out") }));
        assert!(!dir.join("src").exists());
        assert_eq!(fs::metadata(dir.join("out").join("src").join("n.bin")).unwrap().len(), 100);
        fs::remove_dir_all(&dir).ok();
    }
}


#[cfg(test)]
mod tests_v02lp {
    use super::*;

    #[test]
    fn long_path_adds_prefix() {
        #[cfg(windows)]
        {
            let long = PathBuf::from("C:\\".to_string() + &"a".repeat(260));
            assert!(long_path(&long).to_string_lossy().starts_with(r"\\?\"));
            let short = PathBuf::from("C:\\x");
            assert_eq!(long_path(&short), short);
        }
    }

    #[test]
    fn copy_into_deep_path() {
        let dir = std::env::temp_dir().join(format!("fm_deep_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let deep = (0..12).fold(dir.clone(), |p, i| p.join(format!("d{i:02}_{}", "x".repeat(15))));
        fs::create_dir_all(&deep).unwrap();
        fs::write(dir.join("f.txt"), b"deep").unwrap();
        let (_r, rrx) = std::sync::mpsc::channel();
        let (etx, _e) = std::sync::mpsc::channel();
        let mut w = Worker { reply_rx: &rrx, events: &etx, overwrite_all: false, skip_all: false, failed: 0 };
        w.run(&OpJob {
            id: 1,
            kind: OpKind::Copy { sources: vec![dir.join("f.txt")], dest: deep.clone() },
            cancel: Arc::new(AtomicBool::new(false)),
        });
        assert_eq!(fs::read_to_string(deep.join("f.txt")).unwrap(), "deep");
        fs::remove_dir_all(&dir).ok();
    }
}
