use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::fs::{FileEntry, SortKey};
use crate::core::history::History;

static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(1);

/// Стабильный ключ файла; archive_member различает элементы одного ZIP.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SelectionKey {
    path: PathBuf,
    archive_member: Option<String>,
}

impl SelectionKey {
    pub(crate) fn from_entry(entry: &FileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            archive_member: entry.archive_member.clone(),
        }
    }

    pub(crate) fn matches(&self, entry: &FileEntry) -> bool {
        self.path == entry.path && self.archive_member.as_deref() == entry.archive_member.as_deref()
    }

    pub(crate) fn borrowed(&self) -> (&Path, Option<&str>) {
        (&self.path, self.archive_member.as_deref())
    }
}

/// Рамка выделения хранит только временно добавленные ключи, чтобы обновлять её инкрементально.
#[derive(Debug)]
pub(crate) struct SelectionMarquee {
    pub start: egui::Pos2,
    pub current: egui::Pos2,
    pub added: BTreeSet<SelectionKey>,
}

pub(crate) fn add_marquee_entry(
    selection: &mut BTreeSet<SelectionKey>,
    marquee: &mut SelectionMarquee,
    entry: &FileEntry,
) -> SelectionKey {
    let key = SelectionKey::from_entry(entry);
    if selection.insert(key.clone()) {
        marquee.added.insert(key.clone());
    }
    key
}

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
    /// Выбор привязан к пути, а не к меняющейся позиции строки.
    pub selection: BTreeSet<SelectionKey>,
    /// Стабильный якорь для диапазона Shift.
    pub anchor: Option<SelectionKey>,
    /// Keyboard-focused row index.
    pub cursor: usize,
    pub selection_marquee: Option<SelectionMarquee>,
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
            selection_marquee: None,
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
        self.selection_marquee = None;
    }

    pub fn is_selected(&self, entry: &FileEntry) -> bool {
        self.selection.contains(&SelectionKey::from_entry(entry))
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

/// Перемещает видимую вкладку, не меняя порядок скрытых оконных вкладок.
pub(crate) fn move_to_visible_drop_slot<T>(
    items: &mut Vec<T>,
    visible_indices: &[usize],
    source_slot: usize,
    drop_slot: usize,
) -> Option<usize> {
    if drop_slot > visible_indices.len() {
        return None;
    }
    let source = *visible_indices.get(source_slot)?;
    if source >= items.len() {
        return None;
    }

    let target_slot = drop_slot.saturating_sub(usize::from(drop_slot > source_slot));
    let item = items.remove(source);
    let visible_after: Vec<usize> = visible_indices
        .iter()
        .enumerate()
        .filter_map(|(slot, &index)| {
            (slot != source_slot).then_some(if index > source { index - 1 } else { index })
        })
        .collect();
    let target = visible_after
        .get(target_slot)
        .copied()
        .unwrap_or_else(|| visible_after.last().map_or(items.len(), |index| index + 1));
    items.insert(target.min(items.len()), item);
    Some(target.min(items.len() - 1))
}

/// Обновляет выбор по стабильному ключу, а диапазон считает в порядке отрисованных элементов.
pub(crate) fn apply_click(
    selection: &mut BTreeSet<SelectionKey>,
    anchor: &mut Option<SelectionKey>,
    entries: &[FileEntry],
    visual_order: Option<&[usize]>,
    index: usize,
    command: bool,
    shift: bool,
) -> usize {
    let Some(entry) = entries.get(index) else {
        return index;
    };
    let key = SelectionKey::from_entry(entry);
    if shift && let Some(anchor_key) = anchor.as_ref() {
        let anchor_position = visual_position_for_key(entries, visual_order, anchor_key);
        let target_position = visual_position_for_index(entries, visual_order, index);
        if let (Some(anchor_position), Some(target_position)) = (anchor_position, target_position) {
            let (lo, hi) = range_from(anchor_position, target_position);
            if !command {
                selection.clear();
            }
            insert_visual_range(selection, entries, visual_order, lo, hi);
        } else {
            if !command {
                selection.clear();
            }
            selection.insert(key.clone());
            *anchor = Some(key);
        }
    } else if command {
        if !selection.remove(&key) {
            selection.insert(key.clone());
            *anchor = Some(key);
        }
    } else {
        selection.clear();
        selection.insert(key.clone());
        *anchor = Some(key);
    }
    index
}

pub(crate) fn select_all_entries(
    selection: &mut BTreeSet<SelectionKey>,
    entries: &[FileEntry],
) {
    selection.extend(entries.iter().map(SelectionKey::from_entry));
}

fn insert_visual_range(
    selection: &mut BTreeSet<SelectionKey>,
    entries: &[FileEntry],
    visual_order: Option<&[usize]>,
    lo: usize,
    hi: usize,
) {
    for position in lo..=hi {
        let entry_index = match visual_order {
            Some(order) => order.get(position).copied(),
            None => Some(position),
        };
        if let Some(entry) = entry_index.and_then(|index| entries.get(index)) {
            selection.insert(SelectionKey::from_entry(entry));
        }
    }
}

fn visual_position_for_index(
    entries: &[FileEntry],
    visual_order: Option<&[usize]>,
    target: usize,
) -> Option<usize> {
    if let Some(order) = visual_order {
        order.iter().position(|&index| index == target)
    } else {
        (target < entries.len()).then_some(target)
    }
}

fn visual_position_for_key(
    entries: &[FileEntry],
    visual_order: Option<&[usize]>,
    key: &SelectionKey,
) -> Option<usize> {
    if let Some(order) = visual_order {
        order.iter().position(|&index| {
            entries
                .get(index)
                .is_some_and(|entry| key.matches(entry))
        })
    } else {
        entries.iter().position(|entry| key.matches(entry))
    }
}

/// Двигает курсор и расширяет Shift-выбор в порядке отрисованных элементов.
pub(crate) fn apply_cursor_move(
    selection: &mut BTreeSet<SelectionKey>,
    anchor: &mut Option<SelectionKey>,
    entries: &[FileEntry],
    visual_order: Option<&[usize]>,
    current: usize,
    target: usize,
    extend: bool,
) -> Option<usize> {
    let target_entry = entries.get(target)?;
    let target_position = visual_position_for_index(entries, visual_order, target)?;
    let target_key = SelectionKey::from_entry(target_entry);

    if extend {
        if anchor.is_none() {
            *anchor = entries.get(current).map(SelectionKey::from_entry);
        }
        let anchor_position =
            anchor.as_ref().and_then(|key| visual_position_for_key(entries, visual_order, key));
        if let Some(anchor_position) = anchor_position {
            let (lo, hi) = range_from(anchor_position, target_position);
            selection.clear();
            insert_visual_range(selection, entries, visual_order, lo, hi);
        } else {
            selection.clear();
            selection.insert(target_key.clone());
            *anchor = Some(target_key);
        }
    } else {
        selection.clear();
        selection.insert(target_key.clone());
        *anchor = Some(target_key);
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

    fn entry(index: usize) -> FileEntry {
        let name = format!("item-{index}.txt");
        FileEntry {
            name: name.clone(),
            path: PathBuf::from(format!("C:\\items\\{name}")),
            is_dir: false,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: Some(index as u64),
            modified: None,
            created: None,
            extension: Some("txt".to_owned()),
            archive_member: None,
        }
    }

    fn keys_for(entries: &[FileEntry], indices: &[usize]) -> BTreeSet<SelectionKey> {
        indices
            .iter()
            .map(|&index| SelectionKey::from_entry(&entries[index]))
            .collect()
    }

    #[test]
    fn plain_click_selects_single() {
        let entries: Vec<_> = (0..8).map(entry).collect();
        let (mut selection, mut anchor) = (BTreeSet::new(), None);
        assert_eq!(
            apply_click(
                &mut selection,
                &mut anchor,
                &entries,
                None,
                3,
                false,
                false,
            ),
            3
        );
        assert_eq!(selection, keys_for(&entries, &[3]));
        assert_eq!(anchor, Some(SelectionKey::from_entry(&entries[3])));
    }

    #[test]
    fn ctrl_click_toggles() {
        let entries: Vec<_> = (0..8).map(entry).collect();
        let (mut selection, mut anchor) = (BTreeSet::new(), None);
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            None,
            1,
            false,
            false,
        );
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            None,
            5,
            true,
            false,
        );
        assert_eq!(selection, keys_for(&entries, &[1, 5]));
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            None,
            5,
            true,
            false,
        );
        assert_eq!(selection, keys_for(&entries, &[1]));
    }

    #[test]
    fn shift_click_follows_visual_order_and_skips_group_headers() {
        let entries: Vec<_> = (0..6).map(entry).collect();
        // Здесь только ListRow::Item; заголовки групп намеренно пропущены.
        let visual_order = [3, 1, 2, 4, 5, 0];
        let (mut selection, mut anchor) = (BTreeSet::new(), None);
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            1,
            false,
            false,
        );
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            5,
            false,
            true,
        );
        assert_eq!(selection, keys_for(&entries, &[1, 2, 4, 5]));
    }

    #[test]
    fn reverse_shift_click_follows_visual_order() {
        let entries: Vec<_> = (0..6).map(entry).collect();
        let visual_order = [3, 1, 2, 4, 5, 0];
        let (mut selection, mut anchor) = (BTreeSet::new(), None);
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            5,
            false,
            false,
        );
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            1,
            false,
            true,
        );
        assert_eq!(selection, keys_for(&entries, &[1, 2, 4, 5]));
    }

    #[test]
    fn ctrl_shift_extends_range() {
        let entries: Vec<_> = (0..6).map(entry).collect();
        let visual_order = [3, 1, 2, 4, 5, 0];
        let (mut selection, mut anchor) = (BTreeSet::new(), None);
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            0,
            true,
            false,
        );
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            3,
            true,
            false,
        );
        apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            2,
            true,
            true,
        );
        assert_eq!(selection, keys_for(&entries, &[0, 1, 2, 3]));
    }

    #[test]
    fn select_all_ctrl_a() {
        let entries: Vec<_> = (0..50).map(entry).collect();
        let mut selection = BTreeSet::new();
        select_all_entries(&mut selection, &entries);
        assert_eq!(selection, keys_for(&entries, &(0..50).collect::<Vec<_>>()));
    }

    #[test]
    fn selection_key_keeps_the_same_entry_after_reordering() {
        let mut entries = vec![entry(0), entry(1), entry(2)];
        entries[0].name = "z.txt".to_owned();
        entries[1].name = "a.md".to_owned();
        entries[2].name = "m.log".to_owned();
        let selected = SelectionKey::from_entry(&entries[1]);
        let selection = BTreeSet::from([selected.clone()]);
        crate::core::fs::sort_entries_by(&mut entries, SortKey::Name, false);
        let selected_entry = entries
            .iter()
            .find(|entry| selection.contains(&SelectionKey::from_entry(entry)))
            .unwrap();
        assert!(selected.matches(selected_entry));
        assert_eq!(selected_entry.name, "a.md");
        assert_eq!(selection, BTreeSet::from([selected]));
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
    fn moving_visible_tab_keeps_detached_slots_out_of_the_visible_order() {
        let mut tabs = vec!["A", "detached-1", "B", "detached-2", "C"];
        let visible = [0, 2, 4];

        assert_eq!(
            move_to_visible_drop_slot(&mut tabs, &visible, 0, 2),
            Some(3)
        );
        assert_eq!(tabs, ["detached-1", "B", "detached-2", "A", "C"]);
        assert_eq!([tabs[1], tabs[3], tabs[4]], ["B", "A", "C"]);
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
        let entries: Vec<_> = (0..6).map(entry).collect();
        let visual_order = [3, 1, 2, 4, 5, 0];
        let mut selection = keys_for(&entries, &[1]);
        let mut anchor = Some(SelectionKey::from_entry(&entries[1]));
        assert_eq!(
            apply_cursor_move(
                &mut selection,
                &mut anchor,
                &entries,
                Some(&visual_order),
                1,
                4,
                true,
            ),
            Some(4)
        );
        assert_eq!(selection, keys_for(&entries, &[1, 2, 4]));

        assert_eq!(
            apply_cursor_move(
                &mut selection,
                &mut anchor,
                &entries,
                Some(&visual_order),
                4,
                3,
                true,
            ),
            Some(3)
        );
        assert_eq!(selection, keys_for(&entries, &[1, 3]));
    }

    #[test]
    fn keyboard_selection_uses_stable_keys_and_ignores_invalid_targets() {
        let entries: Vec<_> = (0..3).map(entry).collect();
        let order = [2, 0, 1];
        let mut selection = keys_for(&entries, &[1]);
        let mut anchor = Some(SelectionKey::from_entry(&entries[1]));
        assert_eq!(
            apply_cursor_move(
                &mut selection,
                &mut anchor,
                &entries,
                Some(&order),
                1,
                2,
                false,
            ),
            Some(2)
        );
        assert_eq!(selection, keys_for(&entries, &[2]));
        assert_eq!(anchor, Some(SelectionKey::from_entry(&entries[2])));

        assert_eq!(
            apply_cursor_move(
                &mut selection,
                &mut anchor,
                &entries,
                Some(&order),
                2,
                99,
                true,
            ),
            None
        );
        assert_eq!(selection, keys_for(&entries, &[2]));
    }
}
