//! Публичные типы и события очереди файловых операций.

use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Debug)]
pub enum OpKind {
    Copy {
        sources: Vec<PathBuf>,
        dest: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        dest: PathBuf,
    },
    Delete {
        sources: Vec<PathBuf>,
    },
    /// Delete via the Windows Recycle Bin (undo-capable).
    Recycle {
        sources: Vec<PathBuf>,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    CreateDir {
        path: PathBuf,
    },
    CreateFile {
        path: PathBuf,
    },
}

impl OpKind {
    /// Short description for the ops panel (UI adds the localized verb).
    pub fn target_label(&self) -> String {
        match self {
            OpKind::Copy { dest, .. } | OpKind::Move { dest, .. } => dest.display().to_string(),
            OpKind::Delete { sources } | OpKind::Recycle { sources } => {
                format!("{} items", sources.len())
            }
            OpKind::Rename { to, .. } => to.display().to_string(),
            OpKind::CreateDir { path } | OpKind::CreateFile { path } => path.display().to_string(),
        }
    }
}

pub struct OpJob {
    pub id: u64,
    pub kind: OpKind,
    pub cancel: Arc<AtomicBool>,
}

/// Набор платформенных файловых операций, внедряемый при запуске worker.
#[derive(Clone, Copy)]
pub struct OpsPlatform {
    free_space_fn: fn(&Path) -> Option<u64>,
    recycle_bin_fn: fn(&[PathBuf]) -> io::Result<()>,
    replace_file_fn: fn(&Path, &Path) -> io::Result<()>,
}

impl OpsPlatform {
    pub const fn new(
        free_space: fn(&Path) -> Option<u64>,
        recycle_bin: fn(&[PathBuf]) -> io::Result<()>,
        replace_file: fn(&Path, &Path) -> io::Result<()>,
    ) -> Self {
        Self {
            free_space_fn: free_space,
            recycle_bin_fn: recycle_bin,
            replace_file_fn: replace_file,
        }
    }

    pub(super) fn free_space(&self, path: &Path) -> Option<u64> {
        (self.free_space_fn)(path)
    }

    pub(super) fn recycle_bin(&self, paths: &[PathBuf]) -> io::Result<()> {
        (self.recycle_bin_fn)(paths)
    }

    pub(super) fn replace_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        (self.replace_file_fn)(from, to)
    }
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
    AskConflict {
        id: u64,
        src: PathBuf,
        dst: PathBuf,
    },
    /// An item failed — worker waits for Retry / Skip / Cancel.
    AskError {
        id: u64,
        path: PathBuf,
        message: String,
        kind: io::ErrorKind,
        retryable: bool,
    },
    Done {
        id: u64,
        failed: usize,
    },
    /// Операцию отменили до запуска, поэтому диск и кэш не менялись.
    Cancelled { id: u64 },
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
