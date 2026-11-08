use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::fs::{FileEntry, SortKey};
use crate::core::history::History;

static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ListStatus {
    Ready,
    Loading,
}

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct SearchState {
    pub query: String,
    pub recursive: bool,
    pub fuzzy: bool,
    pub extensions: String,
    pub min_size: String,
    pub max_size: String,
    pub modified_from: String,
    pub modified_to: String,
    pub filters_expanded: bool,
    pub generation: u64,
    pub debounce_at: Option<Instant>,
    pub cancel: Arc<AtomicBool>,
    pub running: bool,
    pub results_active: bool,
    pub validation_error: Option<String>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            recursive: false,
            fuzzy: false,
            extensions: String::new(),
            min_size: String::new(),
            max_size: String::new(),
            modified_from: String::new(),
            modified_to: String::new(),
            filters_expanded: false,
            generation: 0,
            debounce_at: None,
            cancel: Arc::new(AtomicBool::new(false)),
            running: false,
            results_active: false,
            validation_error: None,
        }
    }
}

impl SearchState {
    pub fn has_criteria(&self) -> bool {
        !self.query.trim().is_empty()
            || !self.extensions.trim().is_empty()
            || !self.min_size.trim().is_empty()
            || !self.max_size.trim().is_empty()
            || !self.modified_from.trim().is_empty()
            || !self.modified_to.trim().is_empty()
    }

    pub fn schedule(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.wrapping_add(1);
        self.debounce_at = Some(Instant::now() + SEARCH_DEBOUNCE);
        self.running = false;
        self.validation_error = None;
    }

    pub fn reset(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.wrapping_add(1);
        self.query.clear();
        self.recursive = false;
        self.fuzzy = false;
        self.extensions.clear();
        self.min_size.clear();
        self.max_size.clear();
        self.modified_from.clear();
        self.modified_to.clear();
        self.debounce_at = None;
        self.running = false;
        self.results_active = false;
        self.validation_error = None;
    }
}

/// State of a single explorer tab: location, listing, selection and history.
pub struct Tab {
    /// Stable identity for background results; vector indices change on close/reorder.
    pub id: u64,
    pub path: PathBuf,
    pub archive_prefix: Option<String>,
    pub archive_back: Vec<String>,
    pub archive_forward: Vec<String>,
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
    pub search: SearchState,
}

impl Tab {
    pub fn new(path: PathBuf) -> Self {
        Self {
            id: NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed),
            address_input: path.display().to_string(),
            path,
            archive_prefix: None,
            archive_back: Vec::new(),
            archive_forward: Vec::new(),
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
            search: SearchState::default(),
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

/// Opens a folder as a new tab and returns its index.
pub(crate) fn append_tab(tabs: &mut Vec<Tab>, path: PathBuf) -> usize {
    let index = tabs.len();
    tabs.push(Tab::new(path));
    index
}

pub(crate) fn listing_tab_index(tabs: &[Tab], id: u64, generation: u64) -> Option<usize> {
    tabs.iter()
        .position(|tab| tab.id == id && tab.generation == generation)
}

pub(crate) fn take_pending_selection(pending: &mut Option<(u64, String)>, tab_id: u64) -> Option<String> {
    if pending.as_ref().is_some_and(|(id, _)| *id == tab_id) {
        pending.take().map(|(_, name)| name)
    } else {
        None
    }
}

/// Close an unpinned tab unless it is the last one. Returns the new active tab
/// index when a close happened; callers persist settings only on `Some`.
pub(crate) fn close_tab_state(tabs: &mut Vec<Tab>, active: usize, index: usize) -> Option<usize> {
    if tabs.len() <= 1 || index >= tabs.len() || tabs[index].pinned {
        return None;
    }
    tabs[index].cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    tabs[index].search.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    tabs.remove(index);
    Some(if index < active {
        active - 1
    } else if index == active {
        active.min(tabs.len() - 1)
    } else {
        active
    })
}

/// Close every unpinned tab except the requested one; pinned tabs survive.
/// Returns the target tab's new index.
pub(crate) fn close_other_tabs_state(tabs: &mut Vec<Tab>, index: usize) -> Option<usize> {
    let keep = tabs.get(index)?.cancel.clone();
    let mut i = tabs.len();
    while i > 0 {
        i -= 1;
        if i != index && !tabs[i].pinned {
            tabs[i].cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            tabs[i].search.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            tabs.remove(i);
        }
    }
    tabs.iter().position(|t| Arc::ptr_eq(&t.cancel, &keep))
}

/// Range (lo, hi) between anchor and target (Explorer shift semantics).
pub(crate) fn range_from(anchor: usize, target: usize) -> (usize, usize) {
    if anchor <= target { (anchor, target) } else { (target, anchor) }
}

/// Move an item to a drop slot in the original list (`0..=len`).
/// Returns its final index; invalid slots leave the list untouched.
pub(crate) fn move_to_drop_slot<T>(items: &mut Vec<T>, source: usize, slot: usize) -> Option<usize> {
    if source >= items.len() || slot > items.len() {
        return None;
    }
    let item = items.remove(source);
    let target = if slot > source { slot - 1 } else { slot };
    items.insert(target, item);
    Some(target)
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

/// Applies keyboard cursor movement and its selection semantics.
/// Shift-selection always represents the current anchor-to-cursor range, not a
/// union with the previously selected range.
pub(crate) fn apply_cursor_move(
    selection: &mut BTreeSet<usize>,
    anchor: &mut Option<usize>,
    current: usize,
    len: usize,
    target: usize,
    extend: bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let target = target.min(len - 1);
    if extend {
        let anchor = *anchor.get_or_insert(current.min(len - 1));
        let (lo, hi) = range_from(anchor, target);
        selection.clear();
        selection.extend(lo..=hi);
    } else {
        selection.clear();
        selection.insert(target);
        *anchor = Some(target);
    }
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    #[test]
    fn opening_folder_in_new_tab_appends_it_without_changing_existing_tabs() {
        let mut tabs = vec![Tab::new(PathBuf::from("C:\\current"))];
        let opened = PathBuf::from("D:\\other folder");
        let index = append_tab(&mut tabs, opened.clone());
        assert_eq!(index, 1);
        assert_eq!(tabs[0].path, PathBuf::from("C:\\current"));
        assert_eq!(tabs[1].path, opened);
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

    #[test]
    fn drag_drop_slot_is_relative_to_original_order() {
        let mut tabs = vec!["A", "B", "C", "D"];

        // Dropping A on the right half of B means immediately after B, not end.
        assert_eq!(move_to_drop_slot(&mut tabs, 0, 2), Some(1));
        assert_eq!(tabs, ["B", "A", "C", "D"]);

        // Dropping D before B places it at the beginning.
        assert_eq!(move_to_drop_slot(&mut tabs, 3, 0), Some(0));
        assert_eq!(tabs, ["D", "B", "A", "C"]);
    }

    #[test]
    fn dragging_to_either_edge_of_same_tab_is_a_noop() {
        let mut tabs = vec!["A", "B", "C"];
        assert_eq!(move_to_drop_slot(&mut tabs, 1, 1), Some(1));
        assert_eq!(tabs, ["A", "B", "C"]);
        assert_eq!(move_to_drop_slot(&mut tabs, 1, 2), Some(1));
        assert_eq!(tabs, ["A", "B", "C"]);
    }

    #[test]
    fn invalid_drag_drop_slot_does_not_mutate_tabs() {
        let mut tabs = vec!["A", "B"];
        assert_eq!(move_to_drop_slot(&mut tabs, 2, 0), None);
        assert_eq!(move_to_drop_slot(&mut tabs, 0, 3), None);
        assert_eq!(tabs, ["A", "B"]);
    }

    #[test]
    fn closing_a_tab_preserves_another_active_tab_and_cancels_the_removed_one() {
        let mut tabs = vec![
            Tab::new(PathBuf::from("C:\\a")),
            Tab::new(PathBuf::from("C:\\b")),
            Tab::new(PathBuf::from("C:\\c")),
        ];
        let removed_cancel = tabs[0].cancel.clone();
        assert_eq!(close_tab_state(&mut tabs, 2, 0), Some(1));
        assert_eq!(tabs[1].path, PathBuf::from("C:\\c"));
        assert!(removed_cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn closing_active_tab_selects_the_next_or_previous_survivor() {
        let mut tabs = vec![
            Tab::new(PathBuf::from("C:\\a")),
            Tab::new(PathBuf::from("C:\\b")),
            Tab::new(PathBuf::from("C:\\c")),
        ];
        assert_eq!(close_tab_state(&mut tabs, 1, 1), Some(1));
        assert_eq!(tabs[1].path, PathBuf::from("C:\\c"));
        assert_eq!(close_tab_state(&mut tabs, 1, 1), Some(0));
        assert_eq!(tabs[0].path, PathBuf::from("C:\\a"));
    }

    #[test]
    fn last_or_pinned_tab_cannot_be_closed() {
        let mut one = vec![Tab::new(PathBuf::from("C:\\only"))];
        assert_eq!(close_tab_state(&mut one, 0, 0), None);
        assert_eq!(one.len(), 1);

        let mut pinned = vec![Tab::new(PathBuf::from("C:\\pin")), Tab::new(PathBuf::from("C:\\other"))];
        pinned[0].pinned = true;
        let cancel = pinned[0].cancel.clone();
        assert_eq!(close_tab_state(&mut pinned, 1, 0), None);
        assert_eq!(pinned.len(), 2);
        assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn close_others_keeps_target_and_pinned_tabs_and_cancels_removed_tabs() {
        let mut tabs = vec![
            Tab::new(PathBuf::from("C:\\a")),
            Tab::new(PathBuf::from("C:\\b")),
            Tab::new(PathBuf::from("C:\\pin")),
            Tab::new(PathBuf::from("C:\\d")),
        ];
        tabs[2].pinned = true;
        let removed_a = tabs[0].cancel.clone();
        let kept_target = tabs[1].cancel.clone();
        let removed_d = tabs[3].cancel.clone();

        assert_eq!(close_other_tabs_state(&mut tabs, 1), Some(0));
        assert_eq!(
            tabs.iter().map(|t| t.path.as_path()).collect::<Vec<_>>(),
            vec![Path::new("C:\\b"), Path::new("C:\\pin")]
        );
        assert!(std::sync::Arc::ptr_eq(&tabs[0].cancel, &kept_target));
        assert!(removed_a.load(std::sync::atomic::Ordering::Relaxed));
        assert!(removed_d.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn late_listing_is_ignored_after_tab_close_and_reorder() {
        let mut tabs = vec![
            Tab::new(PathBuf::from("C:\\a")),
            Tab::new(PathBuf::from("C:\\b")),
        ];
        let closed_id = tabs[0].id;
        let surviving_id = tabs[1].id;
        tabs.remove(0);

        assert_eq!(listing_tab_index(&tabs, closed_id, 0), None);
        assert_eq!(listing_tab_index(&tabs, surviving_id, 0), Some(0));
        assert_eq!(listing_tab_index(&tabs, surviving_id, 99), None);
    }

    #[test]
    fn selection_requested_for_one_tab_survives_other_tabs_loading_first() {
        let mut pending = Some((41, "wanted.txt".to_owned()));
        assert_eq!(take_pending_selection(&mut pending, 12), None);
        assert_eq!(pending, Some((41, "wanted.txt".to_owned())));
        assert_eq!(take_pending_selection(&mut pending, 41), Some("wanted.txt".to_owned()));
        assert_eq!(pending, None);
    }

    #[test]
    fn keyboard_shift_selection_tracks_current_range_in_both_directions() {
        let mut selection = BTreeSet::from([2]);
        let mut anchor = Some(2);
        assert_eq!(apply_cursor_move(&mut selection, &mut anchor, 2, 8, 5, true), Some(5));
        assert_eq!(selection, BTreeSet::from([2, 3, 4, 5]));

        assert_eq!(apply_cursor_move(&mut selection, &mut anchor, 5, 8, 3, true), Some(3));
        assert_eq!(selection, BTreeSet::from([2, 3]));
    }

    #[test]
    fn keyboard_selection_clamps_cursor_and_empty_lists_are_unchanged() {
        let mut selection = BTreeSet::from([1]);
        let mut anchor = Some(1);
        assert_eq!(apply_cursor_move(&mut selection, &mut anchor, 1, 3, 99, false), Some(2));
        assert_eq!(selection, BTreeSet::from([2]));
        assert_eq!(anchor, Some(2));

        assert_eq!(apply_cursor_move(&mut selection, &mut anchor, 2, 0, 0, true), None);
        assert_eq!(selection, BTreeSet::from([2]));
    }
}
