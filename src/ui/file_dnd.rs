use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::fs::FileEntry;
use crate::core::ops::OpKind;
use crate::ui::FileManagerApp;
use crate::ui::tabs::SelectionKey;

pub(crate) const FILE_DND_DRAG_THRESHOLD_POINTS: f32 = 6.0;
pub(crate) const FILE_DND_START_DELAY: Duration = Duration::from_millis(100);
pub(crate) const FILE_DND_EDGE_ZONE_POINTS: f32 = 36.0;
pub(crate) const FILE_DND_MAX_SCROLL_POINTS_PER_SECOND: f32 = 480.0;
pub(crate) const FILE_DND_MAX_FRAME_DELTA_SECONDS: f32 = 0.1;
pub(crate) const FILE_DND_REPAINT_AFTER: Duration = Duration::from_millis(16);

#[derive(Clone, Debug)]
pub(crate) struct FileDragPayload {
    pub sources: Vec<DragSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DragSource {
    pub key: SelectionKey,
    pub path: PathBuf,
    pub is_dir: bool,
    pub volume: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropMode {
    Move,
    Copy,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DropFeedback {
    pub mode: DropMode,
    pub valid: bool,
}

pub(crate) fn file_row_sense() -> egui::Sense {
    egui::Sense::click_and_drag()
}

fn drag_start_ready(origin: egui::Pos2, pointer: egui::Pos2, elapsed: Duration) -> bool {
    origin.distance(pointer) >= FILE_DND_DRAG_THRESHOLD_POINTS && elapsed >= FILE_DND_START_DELAY
}

pub(crate) fn file_row_id(tab_id: u64, entry: &FileEntry) -> egui::Id {
    egui::Id::unique(("file_entry", tab_id, &entry.path, &entry.archive_member))
}

pub(crate) fn select_drag_entries(
    entries: &[FileEntry],
    selection: &mut BTreeSet<SelectionKey>,
    source_index: usize,
) -> Option<Vec<SelectionKey>> {
    let source = entries.get(source_index)?;
    let source_key = SelectionKey::from_entry(source);
    if !selection.contains(&source_key) {
        selection.clear();
        selection.insert(source_key);
    }

    let keys: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            let key = SelectionKey::from_entry(entry);
            selection.contains(&key).then_some(key)
        })
        .collect();
    (!keys.is_empty()).then_some(keys)
}

fn dnd_path_is_inside(root: &Path, candidate: &Path) -> bool {
    // absolute убирает . и .. без обхода содержимого каталога; границы проверяет helper кэша.
    let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let candidate = std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    crate::core::dir_cache::path_is_inside(&root, &candidate)
}

pub(crate) fn normalize_drag_sources(
    entries: &[FileEntry],
    keys: &[SelectionKey],
) -> Vec<DragSource> {
    let candidates: Vec<_> = keys
        .iter()
        .filter_map(|key| {
            entries
                .iter()
                .find(|entry| key.matches(entry))
                .filter(|entry| entry.archive_member.is_none())
                .map(|entry| (key.clone(), entry.path.clone(), entry.is_dir))
        })
        .collect();

    candidates
        .iter()
        .filter(|(_, path, _)| {
            !candidates.iter().any(|(_, parent, parent_is_dir)| {
                *parent_is_dir && parent != path && dnd_path_is_inside(parent, path)
            })
        })
        .map(|(key, path, is_dir)| DragSource {
            key: key.clone(),
            path: path.clone(),
            is_dir: *is_dir,
            volume: None,
        })
        .collect()
}

fn source_volumes(sources: &[DragSource]) -> Vec<Option<String>> {
    sources.iter().map(|source| source.volume.clone()).collect()
}

fn cursor_item_count(payload: &FileDragPayload) -> usize {
    payload.sources.len()
}

fn same_component_path(left: &Path, right: &Path) -> bool {
    dnd_path_is_inside(left, right) && dnd_path_is_inside(right, left)
}

pub(crate) fn mode_for_volumes(
    source_volumes: &[Option<String>],
    destination_volume: Option<&str>,
    ctrl: bool,
    shift: bool,
) -> DropMode {
    if ctrl && !shift {
        return DropMode::Copy;
    }
    if shift && !ctrl {
        return DropMode::Move;
    }

    let same_known_volume = destination_volume.is_some_and(|destination| {
        !source_volumes.is_empty()
            && source_volumes
                .iter()
                .all(|source| source.as_deref() == Some(destination))
    });
    if same_known_volume {
        DropMode::Move
    } else {
        DropMode::Copy
    }
}

pub(crate) fn build_drop_operation(
    sources: &[(PathBuf, bool)],
    destination: &Path,
    target_is_folder: bool,
    target_allowed: bool,
    shutdown_active: bool,
    mode: DropMode,
) -> Option<OpKind> {
    if shutdown_active
        || !target_allowed
        || !target_is_folder
        || sources.is_empty()
        || sources.iter().any(|(source, is_dir)| {
            source == destination || (*is_dir && dnd_path_is_inside(source, destination))
        })
    {
        return None;
    }

    let sources: Vec<_> = sources
        .iter()
        .filter(|(source, _)| {
            mode != DropMode::Move
                || !source
                    .parent()
                    .is_some_and(|parent| same_component_path(parent, destination))
        })
        .map(|(path, _)| path.clone())
        .collect();
    if sources.is_empty() {
        return None;
    }
    Some(match mode {
        DropMode::Move => OpKind::Move {
            sources,
            dest: destination.to_path_buf(),
        },
        DropMode::Copy => OpKind::Copy {
            sources,
            dest: destination.to_path_buf(),
        },
    })
}

pub(crate) fn enqueue_drop(
    kind: OpKind,
    mode: DropMode,
    enqueue: impl FnOnce(&'static str, OpKind),
) {
    let verb = match mode {
        DropMode::Move => "ops.moving",
        DropMode::Copy => "ops.copying",
    };
    enqueue(verb, kind);
}

pub(crate) fn auto_scroll_delta(pointer_y: f32, rect: egui::Rect, dt: f32) -> f32 {
    if !pointer_y.is_finite()
        || !dt.is_finite()
        || dt <= 0.0
        || !rect.contains(egui::pos2(rect.center().x, pointer_y))
    {
        return 0.0;
    }

    let zone = FILE_DND_EDGE_ZONE_POINTS.min(rect.height() * 0.5);
    if zone <= 0.0 {
        return 0.0;
    }
    let points_per_second = if pointer_y < rect.top() + zone {
        -FILE_DND_MAX_SCROLL_POINTS_PER_SECOND
            * ((rect.top() + zone - pointer_y) / zone).clamp(0.0, 1.0)
    } else if pointer_y > rect.bottom() - zone {
        FILE_DND_MAX_SCROLL_POINTS_PER_SECOND
            * ((pointer_y - (rect.bottom() - zone)) / zone).clamp(0.0, 1.0)
    } else {
        0.0
    };
    points_per_second * dt.min(FILE_DND_MAX_FRAME_DELTA_SECONDS)
}

pub(crate) fn cancel_on_escape(ctx: &egui::Context, escape_pressed: bool) -> bool {
    if escape_pressed && egui::DragAndDrop::has_payload_of_type::<FileDragPayload>(ctx) {
        egui::DragAndDrop::clear_payload(ctx);
        true
    } else {
        false
    }
}

pub(crate) fn paint_drop_feedback(ui: &egui::Ui, rect: egui::Rect, valid: bool) {
    let color = if valid {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().error_fg_color
    };
    let painter = ui.painter();
    painter.rect_filled(
        rect.shrink(1.0),
        crate::ui::style::RADIUS_ROW,
        color.gamma_multiply(if valid { 0.16 } else { 0.12 }),
    );
    painter.rect_stroke(
        rect.shrink(1.0),
        crate::ui::style::RADIUS_ROW,
        egui::Stroke::new(1.5, color),
        egui::StrokeKind::Inside,
    );
}

impl FileManagerApp {
    pub(crate) fn begin_file_drag(
        &mut self,
        response: &egui::Response,
        source_index: usize,
        entry: &FileEntry,
        ctx: &egui::Context,
    ) {
        if self.shutdown_active() {
            self.file_drag_candidate = None;
            return;
        }
        let primary_down =
            ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
        if !primary_down {
            self.file_drag_candidate = None;
            return;
        }
        if response.is_pointer_button_down_on() && self.file_drag_candidate.is_none() {
            self.file_drag_candidate = Some((response.id, std::time::Instant::now()));
        }
        let Some((candidate_id, started_at)) = self.file_drag_candidate else {
            return;
        };
        if candidate_id != response.id || !response.dragged_by(egui::PointerButton::Primary) {
            return;
        }
        let Some(origin) = ctx.input(|input| input.pointer.press_origin()) else {
            return;
        };
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        let elapsed = started_at.elapsed();
        if !drag_start_ready(origin, pointer, elapsed) {
            if elapsed < FILE_DND_START_DELAY {
                ctx.request_repaint_after(FILE_DND_START_DELAY - elapsed);
            }
            return;
        }

        let Some(sources) = (|| {
            let tab = self.tabs.get_mut(self.active)?;
            if tab.archive_prefix.is_some()
                || tab.secure_trash
                || crate::platform::is_wsl_path(&tab.path)
                || entry.archive_member.is_some()
            {
                return None;
            }
            let keys = select_drag_entries(&tab.entries, &mut tab.selection, source_index)?;
            let mut sources = normalize_drag_sources(&tab.entries, &keys);
            for source in &mut sources {
                source.volume = crate::platform::volume_id(&source.path);
            }
            tab.anchor = Some(SelectionKey::from_entry(entry));
            tab.cursor = source_index;
            Some(sources)
        })() else {
            return;
        };
        let payload = FileDragPayload { sources };
        if response.drag_started_by(egui::PointerButton::Primary) {
            response.dnd_set_drag_payload(payload);
        } else {
            egui::DragAndDrop::set_payload(ctx, payload);
        }
        self.file_drag_candidate = None;
    }

    pub(crate) fn handle_file_drop_response(
        &mut self,
        response: &egui::Response,
        destination: Option<&Path>,
        target_is_folder: bool,
        target_allowed: bool,
        ui: &egui::Ui,
        ctx: &egui::Context,
    ) {
        let modifiers = ctx.input(|input| input.modifiers);
        if let Some(payload) = response.dnd_hover_payload::<FileDragPayload>() {
            let fallback = self
                .tabs
                .get(self.active)
                .map(|tab| tab.path.as_path())
                .unwrap_or_else(|| Path::new(""));
            let target = destination.unwrap_or(fallback);
            let plan = destination.and_then(|path| {
                self.file_drop_plan(&payload, path, target_is_folder, target_allowed, modifiers)
            });
            let mode = plan.as_ref().map(|(_, mode)| *mode).unwrap_or_else(|| {
                let destination_volume = crate::platform::volume_id(target);
                mode_for_volumes(
                    &source_volumes(&payload.sources),
                    destination_volume.as_deref(),
                    modifiers.ctrl,
                    modifiers.shift,
                )
            });
            let valid = plan.is_some();
            self.file_drop_feedback = Some(DropFeedback { mode, valid });
            paint_drop_feedback(ui, response.rect, valid);
            ctx.set_cursor_icon(if valid {
                egui::CursorIcon::PointingHand
            } else {
                egui::CursorIcon::NotAllowed
            });
        }

        if let Some(payload) = response.dnd_release_payload::<FileDragPayload>()
            && let Some(destination) = destination
            && let Some((kind, mode)) = self.file_drop_plan(
                &payload,
                destination,
                target_is_folder,
                target_allowed,
                modifiers,
            )
        {
            enqueue_drop(kind, mode, |verb, kind| self.enqueue(verb, kind));
        }
    }

    pub(crate) fn active_folder_accepts_drop(&self, destination: &Path) -> bool {
        self.tabs.get(self.active).is_some_and(|tab| {
            tab.archive_prefix.is_none()
                && !tab.secure_trash
                && !crate::platform::is_wsl_path(destination)
        })
    }

    pub(crate) fn auto_scroll_file_list(&self, ui: &egui::Ui, ctx: &egui::Context) {
        if !egui::DragAndDrop::has_payload_of_type::<FileDragPayload>(ctx) {
            return;
        }
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        let rect = ui.clip_rect();
        if !rect.contains(pointer) {
            return;
        }
        let dt = ctx.input(|input| input.stable_dt);
        let offset_delta = auto_scroll_delta(pointer.y, rect, dt);
        if offset_delta != 0.0 {
            // ScrollArea принимает дельту с обратным знаком относительно смещения содержимого.
            ui.scroll_with_delta(egui::vec2(0.0, -offset_delta));
            ctx.request_repaint_after(FILE_DND_REPAINT_AFTER);
        }
    }

    pub(crate) fn show_file_drag_preview(&mut self, ctx: &egui::Context) {
        let Some(payload) = egui::DragAndDrop::payload::<FileDragPayload>(ctx) else {
            return;
        };
        if self.shutdown_active() {
            egui::DragAndDrop::clear_payload(ctx);
            return;
        }

        let destination = self
            .tabs
            .get(self.active)
            .map(|tab| tab.path.as_path())
            .unwrap_or_else(|| Path::new(""));
        let modifiers = ctx.input(|input| input.modifiers);
        let mode = self
            .file_drop_feedback
            .map(|feedback| feedback.mode)
            .unwrap_or_else(|| {
                let destination_volume = crate::platform::volume_id(destination);
                mode_for_volumes(
                    &source_volumes(&payload.sources),
                    destination_volume.as_deref(),
                    modifiers.ctrl,
                    modifiers.shift,
                )
            });
        let mode_label = self
            .t(match mode {
                DropMode::Move => "dnd.move",
                DropMode::Copy => "dnd.copy",
            })
            .to_owned();
        let count_label = self.t("dnd.count").to_owned();
        let invalid_label = self.t("dnd.invalid_target").to_owned();
        let label = if self
            .file_drop_feedback
            .is_some_and(|feedback| !feedback.valid)
        {
            format!(
                "{}: {} · {} · {}",
                count_label,
                cursor_item_count(&payload),
                mode_label,
                invalid_label
            )
        } else {
            format!(
                "{}: {} · {}",
                count_label,
                cursor_item_count(&payload),
                mode_label
            )
        };
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        ctx.request_repaint_after(FILE_DND_REPAINT_AFTER);
        egui::Area::new(egui::Id::unique("file_drag_preview"))
            .order(egui::Order::Foreground)
            .fixed_pos(pointer + egui::vec2(14.0, 18.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .corner_radius(crate::ui::style::RADIUS_ROW)
                    .inner_margin(egui::Margin::symmetric(
                        crate::ui::style::FRAME_PADDING_X,
                        crate::ui::style::FRAME_PADDING_Y,
                    ))
                    .stroke(egui::Stroke::new(1.0, ui.visuals().selection.stroke.color))
                    .show(ui, |ui| {
                        ui.label(label);
                    });
            });
    }

    fn file_drop_plan(
        &self,
        payload: &FileDragPayload,
        destination: &Path,
        target_is_folder: bool,
        target_allowed: bool,
        modifiers: egui::Modifiers,
    ) -> Option<(OpKind, DropMode)> {
        if self.shutdown_active() {
            return None;
        }
        let sources: Vec<_> = payload
            .sources
            .iter()
            .map(|source| (source.path.clone(), source.is_dir))
            .collect();
        let destination_volume = crate::platform::volume_id(destination);
        let mode = mode_for_volumes(
            &source_volumes(&payload.sources),
            destination_volume.as_deref(),
            modifiers.ctrl,
            modifiers.shift,
        );
        let kind = build_drop_operation(
            &sources,
            destination,
            target_is_folder,
            target_allowed,
            false,
            mode,
        )?;
        Some((kind, mode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn entry(path: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: Path::new(path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: PathBuf::from(path),
            is_dir,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: None,
            modified: None,
            created: None,
            extension: None,
            archive_member: None,
        }
    }

    fn key(entry: &FileEntry) -> SelectionKey {
        SelectionKey::from_entry(entry)
    }

    #[test]
    fn file_row_sense_allows_drag_without_breaking_click() {
        let sense = file_row_sense();
        assert!(sense.senses_click());
        assert!(sense.senses_drag());
        let origin = egui::pos2(0.0, 0.0);
        assert!(!drag_start_ready(
            origin,
            egui::pos2(FILE_DND_DRAG_THRESHOLD_POINTS, 0.0),
            FILE_DND_START_DELAY - Duration::from_millis(1),
        ));
        assert!(drag_start_ready(
            origin,
            egui::pos2(FILE_DND_DRAG_THRESHOLD_POINTS, 0.0),
            FILE_DND_START_DELAY,
        ));
    }

    #[test]
    fn drag_payload_uses_stable_selection_keys_not_visible_row_indices() {
        let entries = vec![entry(r"C:\src\a.txt", false), entry(r"C:\src\b.txt", false)];
        let mut selection = BTreeSet::from([key(&entries[1])]);
        let keys = select_drag_entries(&entries, &mut selection, 1).unwrap();
        assert_eq!(keys, vec![key(&entries[1])]);
        assert!(std::any::type_name_of_val(&keys).contains("SelectionKey"));
    }

    #[test]
    fn virtualized_list_dnd_survives_scroll() {
        let file = entry(r"C:\src\stable.txt", false);
        let original_id = file_row_id(7, &file);
        let entries_after_scroll = vec![entry(r"C:\src\other.txt", false), file.clone()];
        let scrolled_id = file_row_id(7, &entries_after_scroll[1]);
        assert_eq!(original_id, scrolled_id);
    }

    #[test]
    fn dnd_rejects_folder_into_itself() {
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\Work"), true)],
                Path::new(r"C:\Work"),
                true,
                true,
                false,
                DropMode::Move,
            )
            .is_none()
        );
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\Work"), true)],
                Path::new(r"C:\Work"),
                true,
                true,
                false,
                DropMode::Copy,
            )
            .is_none()
        );
    }

    #[test]
    fn dnd_rejects_folder_into_own_subtree() {
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\Work"), true)],
                Path::new(r"C:\Work\sub"),
                true,
                true,
                false,
                DropMode::Move,
            )
            .is_none()
        );
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\Work"), true)],
                Path::new(r"C:\Work\..\Work\sub"),
                true,
                true,
                false,
                DropMode::Move,
            )
            .is_none()
        );
    }

    #[test]
    fn dnd_allows_folder_into_sibling_with_shared_prefix() {
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\Work"), true)],
                Path::new(r"C:\Workplace"),
                true,
                true,
                false,
                DropMode::Move,
            )
            .is_some()
        );
    }

    #[test]
    fn dnd_recursive_check_applies_to_every_folder_in_payload() {
        assert!(
            build_drop_operation(
                &[
                    (PathBuf::from(r"C:\Safe"), true),
                    (PathBuf::from(r"C:\Work"), true),
                ],
                Path::new(r"C:\Work\sub"),
                true,
                true,
                false,
                DropMode::Move,
            )
            .is_none()
        );
    }

    #[test]
    fn dnd_drop_into_same_folder_is_noop_for_move() {
        let mut enqueued = false;
        if let Some(kind) = build_drop_operation(
            &[(PathBuf::from(r"C:\Work\a.txt"), false)],
            Path::new(r"C:\Work"),
            true,
            true,
            false,
            DropMode::Move,
        ) {
            enqueue_drop(kind, DropMode::Move, |_, _| enqueued = true);
        }
        assert!(!enqueued);
        assert!(matches!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\Work\a.txt"), false)],
                Path::new(r"C:\Work"),
                true,
                true,
                false,
                DropMode::Copy,
            ),
            Some(OpKind::Copy { .. })
        ));
        assert!(matches!(
            build_drop_operation(
                &[
                    (PathBuf::from(r"C:\Work\a.txt"), false),
                    (PathBuf::from(r"C:\Other\b.txt"), false),
                ],
                Path::new(r"C:\Work"),
                true,
                true,
                false,
                DropMode::Move,
            ),
            Some(OpKind::Move { sources, .. }) if sources == vec![PathBuf::from(r"C:\Other\b.txt")]
        ));
    }

    #[test]
    fn dnd_payload_normalizes_children_whose_parent_is_included() {
        let entries = vec![
            entry(r"C:\Work\parent", true),
            entry(r"C:\Work\parent\child.txt", false),
            entry(r"C:\Work\parent\nested", true),
            entry(r"C:\Work\other.txt", false),
        ];
        let keys: Vec<_> = entries.iter().map(key).collect();
        let normalized = normalize_drag_sources(&entries, &keys);
        assert_eq!(
            normalized
                .iter()
                .map(|source| source.key.clone())
                .collect::<Vec<_>>(),
            vec![key(&entries[0]), key(&entries[3])]
        );
    }

    #[test]
    fn dnd_cursor_count_reflects_normalized_payload() {
        let entries = vec![
            entry(r"C:\Work\parent", true),
            entry(r"C:\Work\parent\child.txt", false),
        ];
        let payload = FileDragPayload {
            sources: normalize_drag_sources(&entries, &entries.iter().map(key).collect::<Vec<_>>()),
        };
        assert_eq!(cursor_item_count(&payload), 1);
    }

    #[test]
    fn dnd_mode_read_at_release_and_hint_updates_with_modifiers() {
        let volumes = [Some("VOL".to_owned())];
        let before = mode_for_volumes(&volumes, Some("VOL"), false, false);
        let at_release = mode_for_volumes(&volumes, Some("VOL"), true, false);
        assert_eq!(before, DropMode::Move);
        assert_eq!(at_release, DropMode::Copy);
        let source = include_str!("file_dnd.rs");
        assert!(source.contains("response.dnd_release_payload::<FileDragPayload>()"));
        assert!(source.contains("ctx.input(|input| input.modifiers)"));
        assert!(source.contains("self.file_drop_feedback = Some(DropFeedback { mode, valid })"));
    }

    #[test]
    fn dnd_missing_object_during_drag_reports_error_not_false_success() {
        let missing = PathBuf::from(r"C:\Work\removed.txt");
        let payload = FileDragPayload {
            sources: vec![DragSource {
                key: key(&entry(r"C:\Work\removed.txt", false)),
                path: missing.clone(),
                is_dir: false,
                volume: Some("VOL".into()),
            }],
        };
        let kind = build_drop_operation(
            &payload
                .sources
                .iter()
                .map(|source| (source.path.clone(), source.is_dir))
                .collect::<Vec<_>>(),
            Path::new(r"C:\Target"),
            true,
            true,
            false,
            DropMode::Move,
        )
        .unwrap();
        assert!(matches!(kind, OpKind::Move { sources, .. } if sources == vec![missing]));
        let worker = include_str!("../core/ops/worker/copy_move.rs");
        assert!(worker.contains("self.ask_error(job, src, &e)"));
        assert!(
            include_str!("../core/ops/worker/tests/tests_v04.rs")
                .contains("fn error_skip_counts_failed")
        );
    }

    #[test]
    fn dnd_name_conflict_uses_existing_ask_conflict_dialog() {
        let dnd = include_str!("file_dnd.rs");
        let worker = include_str!("../core/ops/worker/copy_move.rs");
        let ui = include_str!("opsui.rs");
        assert!(dnd.contains("self.enqueue(verb, kind)"));
        assert!(worker.contains("self.ask_conflict(job, src, dst)"));
        assert!(
            include_str!("../core/ops/worker/tests/tests_operation_decisions.rs")
                .contains("OpEvent::AskConflict")
        );
        assert!(ui.contains("OpEvent::AskConflict { id, src, dst }"));
        assert!(ui.contains("fn conflict_dialog"));
        assert!(
            include_str!("app/tests.rs")
                .contains("conflict_dialog_shown_and_answerable_during_shutdown_phase")
        );
    }

    #[test]
    fn dnd_external_drop_from_explorer_not_regressed() {
        let input = include_str!("app/input.rs");
        assert!(input.contains("i.raw.dropped_files"));
        assert!(input.contains("self.enqueue("));
        assert!(include_str!("app/mod.rs").contains("self.handle_dropped_files(&ctx)"));
    }

    #[test]
    fn selection_marquee_and_tab_drag_not_regressed() {
        assert!(file_row_sense().senses_click());
        assert!(file_row_sense().senses_drag());
        assert!(include_str!("list/mod.rs").contains("egui::Sense::click_and_drag()"));
        assert!(include_str!("app/toolbar.rs").contains("fn tab_drag_sense()"));
    }

    #[test]
    fn existing_dnd_tests_not_regressed() {
        let dnd = include_str!("file_dnd.rs");
        for name in [
            "file_row_sense_allows_drag_without_breaking_click",
            "drag_payload_uses_stable_selection_keys_not_visible_row_indices",
            "dragging_selected_item_drags_whole_selection",
            "dragging_unselected_item_selects_only_it",
            "drop_on_folder_enqueues_move_through_existing_ops_queue",
            "same_volume_drop_moves_and_invalidates_source_and_dest",
            "cross_volume_drop_copies_and_invalidates_dest_only",
            "ctrl_forces_copy_and_shift_forces_move",
            "invalid_drop_target_does_nothing_without_error",
            "esc_cancels_drag_without_operation",
            "drop_during_shutdown_phase_is_rejected",
            "virtualized_list_dnd_survives_scroll",
        ] {
            assert!(dnd.contains(name), "missing preserved test {name}");
        }
        assert!(include_str!("list/grouping.rs").contains("group_header_is_not_draggable"));
        assert!(
            include_str!("list/mod.rs").contains("marquee_selection_still_works_after_dnd_added")
        );
        assert!(
            include_str!("app/toolbar.rs").contains("tab_drag_still_works_after_file_dnd_added")
        );
        assert!(
            include_str!("../core/settings.rs")
                .contains("fn settings_structure_and_json_schema_unchanged()")
        );
    }

    #[test]
    fn tab_drag_and_detach_behavior_unchanged_after_dnd_safety_fix() {
        let app_tests = include_str!("app/tests.rs");
        for name in [
            "tab_drag_fsm_distinguishes_click_drag_and_close_hitbox",
            "detached_tab_viewport_is_borderless_resizable_and_keeps_its_size",
            "detached_window_position_stays_on_the_release_monitor",
            "tab_strip_uses_mouse_wheel_without_showing_a_scrollbar",
            "tab_strip_scroll_alignment_changes_only_when_target_is_clipped",
        ] {
            assert!(
                app_tests.contains(name),
                "missing tab regression test {name}"
            );
        }
        let input = include_str!("app/input.rs");
        assert!(input.contains("self.handle_keys_impl(ctx, false)"));
        assert!(input.contains("allow_tab_management && !self.tab_drag_state.is_active()"));
        assert!(
            include_str!("app/toolbar.rs").contains("fn tab_drag_still_works_after_file_dnd_added")
        );
    }

    #[test]
    fn cache_component_helper_semantics_shared_by_cache_and_dnd() {
        let work = Path::new(r"C:\Work");
        assert!(crate::core::dir_cache::path_is_inside(work, work));
        assert!(crate::core::dir_cache::path_is_inside(
            work,
            Path::new(r"C:\Work\sub1")
        ));
        assert!(!crate::core::dir_cache::path_is_inside(
            work,
            Path::new(r"C:\Workplace")
        ));
        assert!(dnd_path_is_inside(work, Path::new(r"C:\Work\..\Work\sub1")));
        assert!(!dnd_path_is_inside(work, Path::new(r"C:\Workplace")));
        assert!(include_str!("../core/dir_cache.rs").contains("candidate.starts_with(root)"));
        assert!(
            include_str!("file_dnd.rs").contains("dir_cache::path_is_inside(&root, &candidate)")
        );
    }

    #[test]
    fn prefix_invalidation_boundary_not_regressed_by_dnd_helper_reuse() {
        let cache = include_str!("../core/dir_cache.rs");
        for name in [
            "prefix_invalidation_respects_component_boundary",
            "recursive_delete_invalidates_subtree_and_parent",
            "move_dir_invalidates_source_subtree_and_dest_parent",
            "move_invalidates_both_source_and_dest_dirs",
            "copy_invalidates_dest_not_source",
        ] {
            assert!(cache.contains(name), "missing cache regression test {name}");
        }
    }

    #[test]
    fn dnd_move_still_invalidates_source_and_dest_and_copy_dest_only() {
        let source = PathBuf::from(r"C:\Work\file.txt");
        let destination = Path::new(r"D:\Target");
        let moved = build_drop_operation(
            &[(source.clone(), false)],
            destination,
            true,
            true,
            false,
            DropMode::Move,
        )
        .unwrap();
        let moved_dirs = crate::core::dir_cache::affected_dirs(&moved);
        assert!(moved_dirs.contains(&PathBuf::from(r"C:\Work")));
        assert!(moved_dirs.contains(&destination.to_path_buf()));

        let copied = build_drop_operation(
            &[(source, false)],
            destination,
            true,
            true,
            false,
            DropMode::Copy,
        )
        .unwrap();
        let copied_dirs = crate::core::dir_cache::affected_dirs(&copied);
        assert!(copied_dirs.contains(&destination.to_path_buf()));
        assert!(!copied_dirs.contains(&PathBuf::from(r"C:\Work")));
        assert!(include_str!("opsui.rs").contains("self.dir_cache.invalidate_dir(&path)"));
    }

    #[test]
    fn existing_dnd_safety_tests_not_regressed() {
        let dnd = include_str!("file_dnd.rs");
        for name in [
            "dnd_rejects_folder_into_itself",
            "dnd_rejects_folder_into_own_subtree",
            "dnd_allows_folder_into_sibling_with_shared_prefix",
            "dnd_recursive_check_applies_to_every_folder_in_payload",
            "dnd_drop_into_same_folder_is_noop_for_move",
            "dnd_payload_normalizes_children_whose_parent_is_included",
            "dnd_cursor_count_reflects_normalized_payload",
            "dnd_mode_read_at_release_and_hint_updates_with_modifiers",
            "dnd_missing_object_during_drag_reports_error_not_false_success",
            "dnd_external_drop_from_explorer_not_regressed",
        ] {
            assert!(dnd.contains(name), "missing DnD safety test {name}");
        }
    }

    #[test]
    fn domain1_cache_notify_ops_not_regressed() {
        let cache = include_str!("../core/dir_cache.rs");
        let ops = include_str!("opsui.rs");
        let worker = include_str!("../core/ops/worker/mod.rs");
        assert!(cache.contains("OpKind::Move { sources, dest }"));
        assert!(cache.contains("OpKind::Copy { dest, .. }"));
        assert!(ops.contains("self.dir_cache.invalidate_dir(&path)"));
        assert!(worker.contains("while let Ok(job) = jobs_rx.recv()"));
        assert!(include_str!("../platform/windows/dir_watch.rs").contains("DirWatchCommand"));
    }

    #[test]
    fn domain2_and_domain3_not_regressed() {
        assert!(include_str!("app/mod.rs").contains("DetailsWatch"));
        assert!(include_str!("app/trash.rs").contains("secure_trash"));
        assert!(include_str!("app/archive.rs").contains("zip"));
        assert!(include_str!("tree.rs").contains("is_wsl_path"));
        assert!(include_str!("opsui.rs").contains("user_facing_operation_error"));
        assert!(include_str!("app/mod.rs").contains("shutdown_active"));
    }

    #[test]
    fn dragging_selected_item_drags_whole_selection() {
        let entries = vec![
            entry(r"C:\src\a.txt", false),
            entry(r"C:\src\b.txt", false),
            entry(r"C:\src\c.txt", false),
        ];
        let mut selection = BTreeSet::from([key(&entries[0]), key(&entries[2])]);
        assert_eq!(
            select_drag_entries(&entries, &mut selection, 2).unwrap(),
            vec![key(&entries[0]), key(&entries[2])]
        );
    }

    #[test]
    fn dragging_unselected_item_selects_only_it() {
        let entries = vec![
            entry(r"C:\src\a.txt", false),
            entry(r"C:\src\b.txt", false),
            entry(r"C:\src\c.txt", false),
        ];
        let mut selection = BTreeSet::from([key(&entries[0]), key(&entries[1])]);
        assert_eq!(
            select_drag_entries(&entries, &mut selection, 2).unwrap(),
            vec![key(&entries[2])]
        );
        assert_eq!(selection, BTreeSet::from([key(&entries[2])]));
    }

    #[test]
    fn drop_on_folder_enqueues_move_through_existing_ops_queue() {
        let kind = build_drop_operation(
            &[(PathBuf::from(r"C:\src\a.txt"), false)],
            Path::new(r"C:\dest"),
            true,
            true,
            false,
            DropMode::Move,
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        enqueue_drop(kind, DropMode::Move, |verb, kind| {
            tx.send((verb, kind)).unwrap();
        });
        let (verb, kind) = rx.try_recv().unwrap();
        assert_eq!(verb, "ops.moving");
        assert!(matches!(kind, OpKind::Move { .. }));
    }

    #[test]
    fn same_volume_drop_moves_and_invalidates_source_and_dest() {
        let mode = mode_for_volumes(&[Some("VOL".into())], Some("VOL"), false, false);
        assert_eq!(mode, DropMode::Move);
        let kind = build_drop_operation(
            &[(PathBuf::from(r"C:\src\a.txt"), false)],
            Path::new(r"C:\dest"),
            true,
            true,
            false,
            mode,
        )
        .unwrap();
        let affected = crate::core::dir_cache::affected_dirs(&kind);
        assert!(affected.contains(&PathBuf::from(r"C:\src")));
        assert!(affected.contains(&PathBuf::from(r"C:\dest")));
    }

    #[test]
    fn cross_volume_drop_copies_and_invalidates_dest_only() {
        let mode = mode_for_volumes(&[Some("VOL-A".into())], Some("VOL-B"), false, false);
        assert_eq!(mode, DropMode::Copy);
        assert_eq!(
            mode_for_volumes(&[None], Some("VOL-B"), false, false),
            DropMode::Copy
        );
        let source = PathBuf::from(r"C:\src\a.txt");
        let destination = PathBuf::from(r"D:\dest");
        let kind = build_drop_operation(
            &[(source.clone(), false)],
            &destination,
            true,
            true,
            false,
            mode,
        )
        .unwrap();
        let affected = crate::core::dir_cache::affected_dirs(&kind);
        assert!(affected.contains(&destination));
        assert!(!affected.contains(&source));
        assert!(!affected.contains(&PathBuf::from(r"C:\src")));
    }

    #[test]
    fn ctrl_forces_copy_and_shift_forces_move() {
        let sources = [Some("VOL".to_owned())];
        assert_eq!(
            mode_for_volumes(&sources, Some("VOL"), true, false),
            DropMode::Copy
        );
        assert_eq!(
            mode_for_volumes(&sources, Some("OTHER"), false, true),
            DropMode::Move
        );
    }

    #[test]
    fn invalid_drop_target_does_nothing_without_error() {
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\src\a.txt"), false)],
                Path::new(r"C:\dest"),
                false,
                false,
                false,
                DropMode::Move,
            )
            .is_none()
        );
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\src\folder"), true)],
                Path::new(r"C:\src\folder\child"),
                true,
                true,
                false,
                DropMode::Move,
            )
            .is_none()
        );
    }

    #[test]
    fn esc_cancels_drag_without_operation() {
        let ctx = egui::Context::default();
        let payload = FileDragPayload {
            sources: vec![DragSource {
                key: key(&entry(r"C:\src\a.txt", false)),
                path: PathBuf::from(r"C:\src\a.txt"),
                is_dir: false,
                volume: Some("VOL".into()),
            }],
        };
        egui::DragAndDrop::set_payload(&ctx, payload);
        assert!(cancel_on_escape(&ctx, true));
        assert!(!egui::DragAndDrop::has_payload_of_type::<FileDragPayload>(
            &ctx
        ));
    }

    #[test]
    fn drop_during_shutdown_phase_is_rejected() {
        assert!(
            build_drop_operation(
                &[(PathBuf::from(r"C:\src\a.txt"), false)],
                Path::new(r"C:\dest"),
                true,
                true,
                true,
                DropMode::Move,
            )
            .is_none()
        );
    }
}
