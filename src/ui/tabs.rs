use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::fs::FileEntry;
use crate::core::history::History;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ListStatus {
    Ready,
    Loading,
}

/// State of a single explorer tab: location, listing, selection and history.
pub struct Tab {
    pub path: PathBuf,
    pub address_input: String,
    pub entries: Vec<FileEntry>,
    /// Selected row indices into `entries`.
    pub selection: BTreeSet<usize>,
    /// Anchor index for Shift-range selection.
    pub anchor: Option<usize>,
    /// Keyboard-focused row index.
    pub cursor: usize,
    pub history: History,
    pub status: ListStatus,
    pub generation: u64,
    pub cancel: Arc<AtomicBool>,
    pub error: Option<String>,
}

impl Tab {
    pub fn new(path: PathBuf) -> Self {
        Self {
            address_input: path.display().to_string(),
            path,
            entries: Vec::new(),
            selection: BTreeSet::new(),
            anchor: None,
            cursor: 0,
            history: History::default(),
            status: ListStatus::Loading,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            error: None,
        }
    }

    /// Tab caption: the folder name, or the full path for drive roots.
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.anchor = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_uses_folder_name() {
        let t = Tab::new(PathBuf::from("C:\\foo\\bar"));
        assert_eq!(t.title(), "bar");
    }

    #[test]
    fn title_root_not_empty() {
        let t = Tab::new(PathBuf::from("C:\\"));
        assert!(!t.title().is_empty());
    }
}
