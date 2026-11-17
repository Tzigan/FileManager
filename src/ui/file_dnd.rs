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
    pub source_tab_id: u64,
    pub keys: Vec<SelectionKey>,
    pub source_volumes: Vec<Option<String>>,
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
    egui::Id::unique((
        "file_entry",
        tab_id,
        &entry.path,
        &entry.archive_member,
    ))
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
            source == destination
                || (mode == DropMode::Move && source.parent() == Some(destination))
                || (*is_dir && destination.starts_with(source))
        })
    {
        return None;
    }

    let sources = sources.iter().map(|(path, _)| path.clone()).collect();
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
        let primary_down = ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
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

        let Some((source_tab_id, keys)) = (|| {
            let tab = self.tabs.get_mut(self.active)?;
            if tab.archive_prefix.is_some()
                || tab.secure_trash
                || crate::platform::is_wsl_path(&tab.path)
                || entry.archive_member.is_some()
            {
                return None;
            }
            let keys = select_drag_entries(&tab.entries, &mut tab.selection, source_index)?;
            tab.anchor = Some(SelectionKey::from_entry(entry));
            tab.cursor = source_index;
            Some((tab.id, keys))
        })() else {
            return;
        };

        let source_volumes = keys
            .iter()
            .map(|key| crate::platform::volume_id(key.path()))
            .collect();
        let payload = FileDragPayload {
            source_tab_id,
            keys,
            source_volumes,
        };
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
                self.file_drop_plan(
                    &payload,
                    path,
                    target_is_folder,
                    target_allowed,
                    modifiers,
                )
            });
            let mode = plan.as_ref().map(|(_, mode)| *mode).unwrap_or_else(|| {
                let destination_volume = crate::platform::volume_id(target);
                mode_for_volumes(
                    &payload.source_volumes,
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
                    &payload.source_volumes,
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
                payload.keys.len(),
                mode_label,
                invalid_label
            )
        } else {
            format!("{}: {} · {}", count_label, payload.keys.len(), mode_label)
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
                    .stroke(egui::Stroke::new(
                        1.0,
                        ui.visuals().selection.stroke.color,
                    ))
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
        let source_tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == payload.source_tab_id)?;
        if source_tab.archive_prefix.is_some()
            || source_tab.secure_trash
            || crate::platform::is_wsl_path(&source_tab.path)
        {
            return None;
        }
        let sources: Vec<_> = payload
            .keys
            .iter()
            .map(|key| {
                source_tab
                    .entries
                    .iter()
                    .find(|entry| key.matches(entry))
                    .filter(|entry| entry.archive_member.is_none())
                    .map(|entry| (entry.path.clone(), entry.is_dir))
            })
            .collect::<Option<_>>()?;
        let destination_volume = crate::platform::volume_id(destination);
        let mode = mode_for_volumes(
            &payload.source_volumes,
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
            source_tab_id: 1,
            keys: vec![key(&entry(r"C:\src\a.txt", false))],
            source_volumes: vec![Some("VOL".into())],
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
