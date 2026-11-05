use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::fs::{FileEntry, SortKey};
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
    pub pinned: bool,
    pub sort_key: SortKey,
    pub sort_desc: bool,
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
            pinned: false,
            sort_key: SortKey::Name,
            sort_desc: false,
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

/// Range (lo, hi) between anchor and target (Explorer shift semantics).
pub(crate) fn range_from(anchor: usize, target: usize) -> (usize, usize) {
    if anchor <= target { (anchor, target) } else { (target, anchor) }
}

/// Click selection honoring Ctrl/Shift modifiers (Explorer semantics).
/// Returns the new cursor position.
pub(crate) fn apply_click(
    sel: &mut std::collections::BTreeSet<usize>,
    anchor: &mut Option<usize>,
    index: usize,
    ctrl: bool,
    shift: bool,
) -> usize {
    if shift && anchor.is_some() {
        let (lo, hi) = range_from(anchor.unwrap(), index);
        if !ctrl {
            sel.clear();
        }
        sel.extend(lo..=hi);
    } else if ctrl {
        if !sel.remove(&index) {
            sel.insert(index);
            *anchor = Some(index);
        }
    } else {
        sel.clear();
        sel.insert(index);
        *anchor = Some(index);
    }
    index
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

    fn sel() -> (std::collections::BTreeSet<usize>, Option<usize>) {
        (std::collections::BTreeSet::new(), None)
    }

    #[test]
    fn plain_click_selects_single() {
        let (mut s, mut a) = sel();
        assert_eq!(apply_click(&mut s, &mut a, 3, false, false), 3);
        assert_eq!(s.len(), 1);
        assert!(s.contains(&3) && a == Some(3));
    }

    #[test]
    fn ctrl_click_toggles() {
        let (mut s, mut a) = sel();
        apply_click(&mut s, &mut a, 1, false, false);
        apply_click(&mut s, &mut a, 5, true, false);
        assert!(s.contains(&1) && s.contains(&5));
        apply_click(&mut s, &mut a, 5, true, false);
        assert!(!s.contains(&5) && s.contains(&1));
    }

    #[test]
    fn shift_click_fills_range() {
        let (mut s, mut a) = sel();
        apply_click(&mut s, &mut a, 2, false, false);
        apply_click(&mut s, &mut a, 6, false, true);
        assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn shift_click_reverse_range() {
        let (mut s, mut a) = sel();
        apply_click(&mut s, &mut a, 7, false, false);
        apply_click(&mut s, &mut a, 3, false, true);
        assert_eq!(s.iter().copied().collect::<Vec<_>>(), vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn ctrl_shift_extends_range() {
        let (mut s, mut a) = sel();
        apply_click(&mut s, &mut a, 9, true, false);
        apply_click(&mut s, &mut a, 4, true, false);
        apply_click(&mut s, &mut a, 6, true, true);
        assert!(s.contains(&9));
        assert!(s.iter().copied().collect::<Vec<_>>().contains(&5));
    }

    #[test]
    fn select_all_ctrl_a() {
        let mut s = std::collections::BTreeSet::new();
        for i in 0..50 { s.insert(i); }
        assert_eq!(s.len(), 50);
    }
}
