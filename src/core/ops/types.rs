//! Публичные типы и события очереди файловых операций.

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
        retryable: bool,
    },
    Done {
        id: u64,
        failed: usize,
    },
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
