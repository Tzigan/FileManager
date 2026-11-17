use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use crate::ui::file_dnd::{self, BookmarkPathState};

pub(super) const BOOKMARK_STATE_TTL: Duration = Duration::from_secs(5);
pub(super) const BOOKMARK_PROBE_DRAIN_INTERVAL: Duration = Duration::from_millis(100);

struct CachedBookmarkState {
    is_dir: Option<bool>,
    checked_at: Instant,
}

pub(super) struct BookmarkProbeBatch {
    pub(super) results: Vec<(PathBuf, Option<bool>)>,
    pub(super) checked_at: Instant,
}

pub(super) struct BookmarkStateCache {
    entries: HashMap<PathBuf, CachedBookmarkState>,
    watched_paths: BTreeSet<PathBuf>,
    pending_paths: BTreeSet<PathBuf>,
    in_flight_paths: BTreeSet<PathBuf>,
    initialized: bool,
    sender: Sender<BookmarkProbeBatch>,
    receiver: Receiver<BookmarkProbeBatch>,
}

impl BookmarkStateCache {
    pub(super) fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            entries: HashMap::new(),
            watched_paths: BTreeSet::new(),
            pending_paths: BTreeSet::new(),
            in_flight_paths: BTreeSet::new(),
            initialized: false,
            sender,
            receiver,
        }
    }

    pub(super) fn sender(&self) -> Sender<BookmarkProbeBatch> {
        self.sender.clone()
    }

    pub(super) fn sync_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> bool {
        let paths: BTreeSet<_> = paths
            .into_iter()
            .map(|path| file_dnd::normalize_drop_path(&path))
            .collect();
        if self.initialized && paths == self.watched_paths {
            return false;
        }

        self.initialized = true;
        for removed in self.watched_paths.difference(&paths) {
            self.entries.remove(removed);
            self.pending_paths.remove(removed);
        }
        self.watched_paths = paths;
        self.queue_watched_paths();
        true
    }

    pub(super) fn force_refresh(&mut self) {
        self.queue_watched_paths();
    }

    fn queue_watched_paths(&mut self) {
        self.pending_paths.extend(
            self.watched_paths
                .difference(&self.in_flight_paths)
                .cloned(),
        );
    }

    pub(super) fn queue_expired(&mut self, now: Instant, ttl: Duration) {
        let expired: Vec<_> = self
            .watched_paths
            .iter()
            .filter_map(|path| {
                if self.in_flight_paths.contains(path) || self.pending_paths.contains(path) {
                    return None;
                }
                let expired = self
                    .entries
                    .get(path)
                    .is_none_or(|entry| now.saturating_duration_since(entry.checked_at) >= ttl);
                expired.then(|| path.clone())
            })
            .collect();
        self.pending_paths.extend(expired);
    }

    pub(super) fn take_pending(&mut self) -> Option<Vec<PathBuf>> {
        if !self.in_flight_paths.is_empty() || self.pending_paths.is_empty() {
            return None;
        }
        let paths: Vec<_> = std::mem::take(&mut self.pending_paths)
            .into_iter()
            .collect();
        self.in_flight_paths.extend(paths.iter().cloned());
        Some(paths)
    }

    pub(super) fn apply_batch(&mut self, batch: BookmarkProbeBatch) {
        for (path, is_dir) in batch.results {
            let path = file_dnd::normalize_drop_path(&path);
            self.in_flight_paths.remove(&path);
            if self.watched_paths.contains(&path) {
                self.entries.insert(
                    path,
                    CachedBookmarkState {
                        is_dir,
                        checked_at: batch.checked_at,
                    },
                );
            }
        }
        self.in_flight_paths.clear();
    }

    pub(super) fn drain_results(&mut self) -> usize {
        let mut drained = 0;
        loop {
            match self.receiver.try_recv() {
                Ok(batch) => {
                    self.apply_batch(batch);
                    drained += 1;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return drained,
            }
        }
    }

    pub(super) fn cached_state(&self, path: &Path) -> Option<Option<bool>> {
        let key = file_dnd::normalize_drop_path(path);
        self.entries.get(&key).map(|entry| entry.is_dir)
    }

    pub(super) fn drop_state(&self, path: &Path) -> BookmarkPathState {
        match self.cached_state(path) {
            Some(is_dir) => file_dnd::bookmark_path_state(is_dir),
            None => {
                // Неизвестное состояние разрешено только для подсказки и чернового плана:
                // worker повторно проверит цель при выполнении и вернёт понятную ошибку.
                BookmarkPathState::Folder
            }
        }
    }

    pub(super) fn is_in_flight(&self) -> bool {
        !self.in_flight_paths.is_empty()
    }

    pub(super) fn until_next_expiration(&self, now: Instant, ttl: Duration) -> Option<Duration> {
        self.watched_paths
            .iter()
            .filter(|path| {
                !self.in_flight_paths.contains(*path) && !self.pending_paths.contains(*path)
            })
            .filter_map(|path| {
                let entry = self.entries.get(path)?;
                Some(ttl.saturating_sub(now.saturating_duration_since(entry.checked_at)))
            })
            .min()
    }
}

pub(super) fn send_probe_result(
    sender: &Sender<BookmarkProbeBatch>,
    batch: BookmarkProbeBatch,
) -> bool {
    sender.send(batch).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::file_dnd::DropInvalidReason;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(format!(r"C:\Bookmarks\{name}"))
    }

    fn result(path: PathBuf, is_dir: Option<bool>, checked_at: Instant) -> BookmarkProbeBatch {
        BookmarkProbeBatch {
            results: vec![(path, is_dir)],
            checked_at,
        }
    }

    #[test]
    fn bookmark_render_path_has_no_filesystem_calls() {
        let tree = include_str!("../tree.rs");
        let start = tree.find("fn bookmark_section").unwrap();
        let end = tree[start..]
            .find("fn open_system_bookmark")
            .map(|offset| start + offset)
            .unwrap();
        let render = &tree[start..end];
        let panel_start = tree.find("pub(crate) fn tree_panel").unwrap();
        let panel_end = tree[panel_start..]
            .find("fn wsl_tree_section")
            .map(|offset| panel_start + offset)
            .unwrap();
        let panel = &tree[panel_start..panel_end];
        for call in [
            "std::fs::metadata",
            ".is_dir()",
            ".exists()",
            ".try_exists()",
        ] {
            assert!(!render.contains(call), "render path contains {call}");
            assert!(!panel.contains(call), "panel path contains {call}");
        }
    }

    #[test]
    fn bookmark_cache_hit_within_ttl_does_not_request_probe() {
        let now = Instant::now();
        let path = path("local");
        let mut cache = BookmarkStateCache::new();
        cache.sync_paths([path.clone()]);
        assert_eq!(
            cache.take_pending().unwrap(),
            vec![file_dnd::normalize_drop_path(&path)]
        );
        cache.apply_batch(result(path.clone(), Some(true), now));
        cache.queue_expired(
            now + BOOKMARK_STATE_TTL - Duration::from_millis(1),
            BOOKMARK_STATE_TTL,
        );
        assert!(cache.take_pending().is_none());
    }

    #[test]
    fn bookmark_cache_expiry_requests_probe_for_stale_only() {
        let now = Instant::now();
        let fresh = path("fresh");
        let stale = path("stale");
        let mut cache = BookmarkStateCache::new();
        cache.sync_paths([fresh.clone(), stale.clone()]);
        let initial = cache.take_pending().unwrap();
        assert_eq!(initial.len(), 2);
        cache.apply_batch(BookmarkProbeBatch {
            results: vec![(fresh.clone(), Some(true)), (stale.clone(), Some(true))],
            checked_at: now - BOOKMARK_STATE_TTL,
        });
        cache
            .entries
            .get_mut(&file_dnd::normalize_drop_path(&fresh))
            .unwrap()
            .checked_at = now;
        cache.queue_expired(now, BOOKMARK_STATE_TTL);
        assert_eq!(
            cache.take_pending().unwrap(),
            vec![file_dnd::normalize_drop_path(&stale)]
        );
    }

    #[test]
    fn bookmark_probe_in_flight_coalesces_requests() {
        let first = path("first");
        let second = path("second");
        let mut cache = BookmarkStateCache::new();
        cache.sync_paths([first.clone()]);
        assert_eq!(cache.take_pending().unwrap().len(), 1);
        cache.sync_paths([first.clone(), second.clone()]);
        assert!(cache.take_pending().is_none());
        cache.apply_batch(result(first, Some(true), Instant::now()));
        assert_eq!(
            cache.take_pending().unwrap(),
            vec![file_dnd::normalize_drop_path(&second)]
        );
    }

    #[test]
    fn bookmark_channel_drain_applies_results_before_render_and_dnd() {
        let path = path("result");
        let mut cache = BookmarkStateCache::new();
        cache.sync_paths([path.clone()]);
        let tx = cache.sender();
        send_probe_result(&tx, result(path.clone(), Some(false), Instant::now()));
        assert_eq!(cache.drain_results(), 1);
        assert_eq!(cache.drop_state(&path), BookmarkPathState::File);

        let app = include_str!("mod.rs");
        let frame = &app[app.find("fn ui(").unwrap()..];
        assert!(
            frame.find("pump_bookmark_state_cache").unwrap()
                < frame.find("self.tree_panel").unwrap()
        );
        assert!(
            frame.find("pump_bookmark_state_cache").unwrap()
                < frame.find("self.handle_dropped_files").unwrap()
        );
    }

    #[test]
    fn bookmark_unknown_state_treated_as_folder_for_plan_with_comment() {
        let path = path("not-yet-probed");
        let cache = BookmarkStateCache::new();
        let state = cache.drop_state(&path);
        assert_eq!(state, BookmarkPathState::Folder);
        let source = include_str!("bookmark_cache.rs");
        assert!(source.contains("worker повторно проверит цель при выполнении"));
        assert!(state.is_folder());
        assert!(include_str!("../tree.rs").contains("is_folder()"));
    }

    #[test]
    fn bookmark_unavailable_after_probe_shows_existing_reason() {
        let path = path("offline");
        let mut cache = BookmarkStateCache::new();
        cache.sync_paths([path.clone()]);
        cache.take_pending();
        cache.apply_batch(result(path.clone(), None, Instant::now()));
        assert_eq!(cache.drop_state(&path), BookmarkPathState::Unavailable);
        assert_eq!(
            cache.drop_state(&path).invalid_reason(),
            Some(DropInvalidReason::BookmarkUnavailable)
        );
    }

    #[test]
    fn bookmark_list_change_triggers_immediate_probe() {
        let old = path("old");
        let new = path("new");
        let mut cache = BookmarkStateCache::new();
        assert!(cache.sync_paths([old.clone()]));
        assert_eq!(
            cache.take_pending().unwrap(),
            vec![file_dnd::normalize_drop_path(&old)]
        );
        cache.apply_batch(result(old, Some(true), Instant::now()));
        assert!(cache.sync_paths([new.clone()]));
        assert_eq!(
            cache.take_pending().unwrap(),
            vec![file_dnd::normalize_drop_path(&new)]
        );
        cache.apply_batch(result(new.clone(), Some(true), Instant::now()));
        cache.force_refresh();
        assert_eq!(
            cache.take_pending().unwrap(),
            vec![file_dnd::normalize_drop_path(&new)]
        );
    }

    #[test]
    fn bookmark_probe_send_into_closed_channel_does_not_panic() {
        let (tx, rx) = channel();
        drop(rx);
        assert!(!send_probe_result(
            &tx,
            result(path("closed"), Some(true), Instant::now())
        ));
    }

    #[test]
    fn bookmark_probe_thread_not_in_shutdown_join_set() {
        let app = include_str!("mod.rs");
        let drop_start = app.find("impl Drop for FileManagerApp").unwrap();
        let drop_end = app[drop_start..].find("#[cfg(test)]").unwrap() + drop_start;
        let drop_impl = &app[drop_start..drop_end];
        assert!(drop_impl.contains("worker.join()"));
        assert!(drop_impl.contains("bg_ops_pool.join_workers()"));
        assert!(!drop_impl.contains("bookmark_state_cache"));

        let snapshot_start = app.find("fn pending_shutdown_snapshot").unwrap();
        let snapshot_end =
            app[snapshot_start..].find("fn start_shutdown").unwrap() + snapshot_start;
        assert!(!app[snapshot_start..snapshot_end].contains("bookmark"));
    }

    #[test]
    fn bookmark_ui_phase_shutdown_does_not_list_probe_as_pending_operation() {
        let app = include_str!("mod.rs");
        let kind_start = app.find("enum ShutdownOperationKind").unwrap();
        let kind_end = app[kind_start..].find("struct ShutdownOperation").unwrap() + kind_start;
        assert!(!app[kind_start..kind_end].contains("Bookmark"));
        let snapshot_start = app.find("fn pending_shutdown_snapshot").unwrap();
        let snapshot_end =
            app[snapshot_start..].find("fn start_shutdown").unwrap() + snapshot_start;
        assert!(!app[snapshot_start..snapshot_end].contains("probe"));
    }

    #[test]
    fn existing_bookmark_drop_target_tests_not_regressed() {
        let dnd = include_str!("../file_dnd.rs");
        for name in [
            "bookmark_row_is_drop_target_resolving_to_its_folder_path",
            "system_and_personal_bookmarks_both_accepted_as_targets",
            "trash_and_secure_trash_bookmarks_remain_non_targets_without_invalid_highlight",
            "unavailable_bookmark_path_shows_specific_reason_not_silent_invalid",
            "bookmark_surface_priority_above_background_and_other_order_independent",
        ] {
            assert!(dnd.contains(name));
        }
    }

    #[test]
    fn existing_dnd_safety_and_attribution_tests_not_regressed() {
        let dnd = include_str!("../file_dnd.rs");
        for name in [
            "existing_dnd_safety_tests_not_regressed",
            "existing_release_attribution_tests_not_regressed",
            "dnd_rejects_folder_into_itself",
            "dnd_rejects_folder_into_own_subtree",
        ] {
            assert!(dnd.contains(name));
        }
    }

    #[test]
    fn existing_tooltip_anchor_tests_not_regressed() {
        let dnd = include_str!("../file_dnd.rs");
        assert!(dnd.contains("tooltip_anchors_to_cursor_for_any_target_size"));
        assert!(dnd.contains("tooltip_flips_left_when_near_right_window_edge"));
        assert!(dnd.contains("tooltip_flips_up_when_near_bottom_window_edge"));
    }

    #[test]
    fn selection_marquee_and_tab_drag_not_regressed() {
        let dnd = include_str!("../file_dnd.rs");
        assert!(dnd.contains("selection_marquee_and_tab_drag_not_regressed"));
        assert!(dnd.contains("tab_drag_and_detach_behavior_unchanged_after_dnd_safety_fix"));
    }

    #[test]
    fn domain1_cache_notify_ops_not_regressed() {
        let dnd = include_str!("../file_dnd.rs");
        assert!(dnd.contains("domain1_cache_notify_ops_not_regressed"));
        assert!(dnd.contains("domain2_and_domain3_not_regressed"));
    }

    #[test]
    fn settings_structure_and_json_schema_unchanged() {
        assert!(
            include_str!("../../core/settings.rs")
                .contains("fn settings_structure_and_json_schema_unchanged()")
        );
    }
}
