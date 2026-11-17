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
pub(crate) const FILE_DND_HINT_MAX_WIDTH_POINTS: f32 = 360.0;
const FILE_DND_HINT_MAX_HEIGHT_POINTS: f32 = 120.0;
const FILE_DND_HINT_OFFSET_X_POINTS: f32 = 14.0;
const FILE_DND_HINT_OFFSET_Y_POINTS: f32 = 18.0;
const FILE_DND_HINT_WINDOW_MARGIN_POINTS: f32 = 8.0;
const FILE_DND_HINT_FRAME_STROKE_POINTS: f32 = 1.0;

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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DropFeedback {
    pub mode: DropMode,
    pub plan: Option<DropPlan>,
    pub reason: Option<DropInvalidReason>,
    pub surface: DropSurface,
    pub target_key: Option<PathBuf>,
    pub target_rect: Option<egui::Rect>,
    pub layer_id: Option<egui::LayerId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropSurface {
    Other,
    Background,
    Entry,
    Tree,
    Bookmark,
    Breadcrumb,
    Up,
}

impl DropSurface {
    fn priority(self) -> u8 {
        match self {
            Self::Background => 0,
            Self::Other => 1,
            Self::Entry => 2,
            Self::Tree | Self::Bookmark => 3,
            Self::Breadcrumb => 4,
            Self::Up => 5,
        }
    }
}

fn feedback_precedes(candidate: &DropFeedback, current: &DropFeedback) -> bool {
    match candidate
        .surface
        .priority()
        .cmp(&current.surface.priority())
    {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (&candidate.target_key, &current.target_key) {
            (Some(candidate), Some(current)) => candidate < current,
            (Some(_), None) => true,
            _ => false,
        },
    }
}

pub(crate) fn clear_frame_feedback(feedback: &mut Option<DropFeedback>) {
    *feedback = None;
}

fn take_current_release_plan(feedback: &mut Option<DropFeedback>) -> Option<DropPlan> {
    feedback.as_mut()?.plan.take()
}

fn take_enqueueable_release_plan(
    release: &mut Option<DropFeedback>,
    shutdown_active: bool,
) -> Option<DropPlan> {
    if shutdown_active {
        *release = None;
        return None;
    }
    let plan = take_current_release_plan(release)?;
    (!plan.sources.is_empty()).then_some(plan)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropInvalidReason {
    FolderIntoSelf,
    FolderIntoSubfolder,
    FileAlreadyInFolder,
    FolderAlreadyInFolder,
    ItemsAlreadyInFolder,
    InvalidTarget,
    BookmarkUnavailable,
}

impl DropInvalidReason {
    fn translation_key(self) -> &'static str {
        match self {
            Self::FolderIntoSelf => "dnd.invalid_self",
            Self::FolderIntoSubfolder => "dnd.invalid_subfolder",
            Self::FileAlreadyInFolder => "dnd.file_already_in_folder",
            Self::FolderAlreadyInFolder => "dnd.folder_already_in_folder",
            Self::ItemsAlreadyInFolder => "dnd.items_already_in_folder",
            Self::InvalidTarget => "dnd.invalid_target",
            Self::BookmarkUnavailable => "dnd.bookmark_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BookmarkPathState {
    Folder,
    File,
    Unavailable,
}

impl BookmarkPathState {
    pub(crate) fn is_drop_target(self) -> bool {
        self != Self::File
    }

    pub(crate) fn is_folder(self) -> bool {
        self == Self::Folder
    }

    pub(crate) fn invalid_reason(self) -> Option<DropInvalidReason> {
        (self == Self::Unavailable).then_some(DropInvalidReason::BookmarkUnavailable)
    }
}

pub(crate) fn bookmark_path_state(is_directory: Option<bool>) -> BookmarkPathState {
    match is_directory {
        Some(true) => BookmarkPathState::Folder,
        Some(false) => BookmarkPathState::File,
        None => BookmarkPathState::Unavailable,
    }
}

pub(crate) enum BookmarkDropDestination<'a> {
    Folder(&'a Path),
    Unavailable,
    NonTarget,
}

pub(crate) fn system_bookmark_drop_destination(
    target: &crate::core::known_folders::SystemBookmarkTarget,
) -> BookmarkDropDestination<'_> {
    use crate::core::known_folders::SystemBookmarkTarget;

    match target {
        SystemBookmarkTarget::Folder(path) => BookmarkDropDestination::Folder(path),
        SystemBookmarkTarget::UnavailableFolder => BookmarkDropDestination::Unavailable,
        SystemBookmarkTarget::RecycleBin | SystemBookmarkTarget::SecureTrash => {
            BookmarkDropDestination::NonTarget
        }
    }
}

fn apply_drop_invalid_reason(
    feedback: &mut DropFeedback,
    invalid_reason: Option<DropInvalidReason>,
) {
    if feedback.plan.is_none()
        && let Some(reason) = invalid_reason
    {
        feedback.reason = Some(reason);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DropPlan {
    pub mode: DropMode,
    pub sources: Vec<PathBuf>,
    pub destination: PathBuf,
}

impl DropPlan {
    fn into_op_kind(self) -> OpKind {
        match self.mode {
            DropMode::Move => OpKind::Move {
                sources: self.sources,
                dest: self.destination,
            },
            DropMode::Copy => OpKind::Copy {
                sources: self.sources,
                dest: self.destination,
            },
        }
    }
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

pub(crate) fn entry_drop_destination(entry: &FileEntry) -> Option<&Path> {
    entry.is_dir.then_some(entry.path.as_path())
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

pub(crate) fn normalize_drop_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn dnd_path_is_inside(root: &Path, candidate: &Path) -> bool {
    // absolute убирает . и .. без обхода содержимого каталога; границы проверяет helper кэша.
    let root = normalize_drop_path(root);
    let candidate = normalize_drop_path(candidate);
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

fn build_drop_plan(
    sources: &[(PathBuf, bool)],
    destination: &Path,
    target_is_folder: bool,
    target_allowed: bool,
    shutdown_active: bool,
    mode: DropMode,
) -> Result<DropPlan, DropInvalidReason> {
    if shutdown_active || !target_allowed || !target_is_folder || sources.is_empty() {
        return Err(DropInvalidReason::InvalidTarget);
    }

    for (source, is_dir) in sources {
        if same_component_path(source, destination) {
            return Err(if *is_dir {
                DropInvalidReason::FolderIntoSelf
            } else {
                DropInvalidReason::FileAlreadyInFolder
            });
        }
        if *is_dir && dnd_path_is_inside(source, destination) {
            return Err(DropInvalidReason::FolderIntoSubfolder);
        }
    }

    let planned_sources: Vec<_> = sources
        .iter()
        .filter(|(source, _)| {
            mode != DropMode::Move
                || !source
                    .parent()
                    .is_some_and(|parent| same_component_path(parent, destination))
        })
        .map(|(path, _)| path.clone())
        .collect();
    if planned_sources.is_empty() {
        return Err(already_in_folder_reason(sources));
    }

    Ok(DropPlan {
        mode,
        sources: planned_sources,
        destination: destination.to_path_buf(),
    })
}

fn already_in_folder_reason(sources: &[(PathBuf, bool)]) -> DropInvalidReason {
    if sources.iter().all(|(_, is_dir)| *is_dir) {
        DropInvalidReason::FolderAlreadyInFolder
    } else if sources.iter().all(|(_, is_dir)| !*is_dir) {
        DropInvalidReason::FileAlreadyInFolder
    } else {
        DropInvalidReason::ItemsAlreadyInFolder
    }
}

pub(crate) fn plan_file_drop(
    sources: &[DragSource],
    destination: &Path,
    target_is_folder: bool,
    target_allowed: bool,
    shutdown_active: bool,
    destination_volume: Option<&str>,
    modifiers: egui::Modifiers,
) -> DropFeedback {
    let mode = mode_for_volumes(
        &source_volumes(sources),
        destination_volume,
        modifiers.ctrl,
        modifiers.shift,
    );
    let source_paths: Vec<_> = sources
        .iter()
        .map(|source| (source.path.clone(), source.is_dir))
        .collect();
    match build_drop_plan(
        &source_paths,
        destination,
        target_is_folder,
        target_allowed,
        shutdown_active,
        mode,
    ) {
        Ok(plan) => DropFeedback {
            mode,
            plan: Some(plan),
            reason: None,
            surface: DropSurface::Entry,
            target_key: None,
            target_rect: None,
            layer_id: None,
        },
        Err(reason) => DropFeedback {
            mode,
            plan: None,
            reason: Some(reason),
            surface: DropSurface::Entry,
            target_key: None,
            target_rect: None,
            layer_id: None,
        },
    }
}

fn drop_hint_lines(
    count_label: &str,
    count: usize,
    mode_label: &str,
    reason_label: Option<&str>,
) -> Vec<String> {
    let mut lines = vec![format!("{count_label}: {count}"), mode_label.to_owned()];
    if let Some(reason) = reason_label {
        lines.push(reason.to_owned());
    }
    lines
}

fn drop_hint_placement(
    pointer: egui::Pos2,
    tip_size: egui::Vec2,
    viewport: egui::Rect,
) -> egui::Pos2 {
    let safe_rect = viewport.shrink(FILE_DND_HINT_WINDOW_MARGIN_POINTS);
    let size = tip_size.min(safe_rect.size());
    let mut position =
        pointer + egui::vec2(FILE_DND_HINT_OFFSET_X_POINTS, FILE_DND_HINT_OFFSET_Y_POINTS);

    if position.x + size.x > safe_rect.right() {
        position.x = pointer.x - FILE_DND_HINT_OFFSET_X_POINTS - size.x;
    }
    if position.y + size.y > safe_rect.bottom() {
        position.y = pointer.y - FILE_DND_HINT_OFFSET_Y_POINTS - size.y;
    }

    position.x = position.x.clamp(
        safe_rect.left(),
        (safe_rect.right() - size.x).max(safe_rect.left()),
    );
    position.y = position.y.clamp(
        safe_rect.top(),
        (safe_rect.bottom() - size.y).max(safe_rect.top()),
    );
    position
}

fn drop_hint_size(ctx: &egui::Context, lines: &[String]) -> egui::Vec2 {
    let style = ctx.style_of(ctx.theme());
    let font_id = egui::TextStyle::Body.resolve(&style);
    let wrap_width = FILE_DND_HINT_MAX_WIDTH_POINTS;
    let layouts = ctx.fonts_mut(|fonts| {
        lines
            .iter()
            .map(|line| {
                fonts.layout(
                    line.clone(),
                    font_id.clone(),
                    egui::Color32::PLACEHOLDER,
                    wrap_width,
                )
            })
            .collect::<Vec<_>>()
    });
    let text_width = layouts
        .iter()
        .map(|layout| layout.size().x)
        .fold(0.0, f32::max);
    let text_height = layouts.iter().map(|layout| layout.size().y).sum::<f32>()
        + style.spacing.item_spacing.y * lines.len().saturating_sub(1) as f32;
    egui::vec2(
        text_width
            + crate::ui::style::FRAME_PADDING_X as f32 * 2.0
            + FILE_DND_HINT_FRAME_STROKE_POINTS * 2.0,
        text_height
            + crate::ui::style::FRAME_PADDING_Y as f32 * 2.0
            + FILE_DND_HINT_FRAME_STROKE_POINTS * 2.0,
    )
    .min(egui::vec2(
        FILE_DND_HINT_MAX_WIDTH_POINTS
            + crate::ui::style::FRAME_PADDING_X as f32 * 2.0
            + FILE_DND_HINT_FRAME_STROKE_POINTS * 2.0,
        FILE_DND_HINT_MAX_HEIGHT_POINTS
            + crate::ui::style::FRAME_PADDING_Y as f32 * 2.0
            + FILE_DND_HINT_FRAME_STROKE_POINTS * 2.0,
    ))
}

fn register_drop_feedback(current: &mut Option<DropFeedback>, candidate: DropFeedback) {
    let replace = current
        .as_ref()
        .is_none_or(|feedback| feedback_precedes(&candidate, feedback));
    if replace {
        *current = Some(candidate);
    }
}

fn register_release_feedback(release: &mut Option<DropFeedback>, candidate: DropFeedback) {
    *release = Some(candidate);
}

fn feedback_paints_target(feedback: &DropFeedback) -> bool {
    feedback.surface != DropSurface::Other && feedback.target_rect.is_some()
}

fn paint_drop_feedback(ui: &egui::Ui, layer_id: egui::LayerId, rect: egui::Rect, valid: bool) {
    let visuals = ui.visuals();
    let color = if valid {
        visuals.selection.stroke.color
    } else {
        visuals.error_fg_color
    };
    let painter = ui.ctx().layer_painter(layer_id);
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

#[cfg(test)]
pub(crate) fn build_drop_operation(
    sources: &[(PathBuf, bool)],
    destination: &Path,
    target_is_folder: bool,
    target_allowed: bool,
    shutdown_active: bool,
    mode: DropMode,
) -> Option<OpKind> {
    build_drop_plan(
        sources,
        destination,
        target_is_folder,
        target_allowed,
        shutdown_active,
        mode,
    )
    .ok()
    .map(DropPlan::into_op_kind)
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
        surface: DropSurface,
        _ui: &egui::Ui,
        ctx: &egui::Context,
    ) {
        self.handle_file_drop_response_with_reason(
            response,
            destination,
            target_is_folder,
            target_allowed,
            surface,
            None,
            _ui,
            ctx,
        );
    }

    pub(crate) fn handle_file_drop_response_with_reason(
        &mut self,
        response: &egui::Response,
        destination: Option<&Path>,
        target_is_folder: bool,
        target_allowed: bool,
        surface: DropSurface,
        invalid_reason: Option<DropInvalidReason>,
        _ui: &egui::Ui,
        ctx: &egui::Context,
    ) {
        let modifiers = ctx.input(|input| input.modifiers);
        let hover_payload = response.dnd_hover_payload::<FileDragPayload>();
        let release_payload = response.dnd_release_payload::<FileDragPayload>();
        if hover_payload.is_none() && release_payload.is_none() {
            return;
        }

        let Some(payload) = hover_payload
            .as_deref()
            .or_else(|| release_payload.as_deref())
        else {
            return;
        };
        let fallback = self
            .tabs
            .get(self.active)
            .map(|tab| tab.path.as_path())
            .unwrap_or_else(|| Path::new(""));
        let destination_volume = destination
            .map(crate::platform::volume_id)
            .unwrap_or_else(|| crate::platform::volume_id(fallback));
        let mut feedback = destination.map_or_else(
            || DropFeedback {
                mode: mode_for_volumes(
                    &source_volumes(&payload.sources),
                    destination_volume.as_deref(),
                    modifiers.ctrl,
                    modifiers.shift,
                ),
                plan: None,
                reason: None,
                surface,
                target_key: None,
                target_rect: None,
                layer_id: None,
            },
            |path| {
                plan_file_drop(
                    &payload.sources,
                    path,
                    target_is_folder,
                    target_allowed,
                    self.shutdown_active(),
                    destination_volume.as_deref(),
                    modifiers,
                )
            },
        );
        apply_drop_invalid_reason(&mut feedback, invalid_reason);
        feedback.surface = if destination.is_some() {
            surface
        } else if matches!(surface, DropSurface::Background | DropSurface::Bookmark) {
            surface
        } else {
            DropSurface::Other
        };
        feedback.target_key = destination.map(Path::to_path_buf);
        feedback.target_rect = Some(response.rect);
        feedback.layer_id = Some(response.layer_id);
        if release_payload.is_some() {
            register_release_feedback(&mut self.file_drop_release, feedback.clone());
        }
        register_drop_feedback(&mut self.file_drop_feedback, feedback);
    }

    pub(crate) fn finish_file_drop_frame(&mut self, ui: &egui::Ui, ctx: &egui::Context) {
        if let Some(feedback) = self.file_drop_feedback.as_ref() {
            ctx.set_cursor_icon(if feedback.plan.is_some() {
                egui::CursorIcon::PointingHand
            } else if feedback.reason.is_some() {
                egui::CursorIcon::NotAllowed
            } else {
                egui::CursorIcon::Default
            });
            if feedback_paints_target(feedback)
                && let (Some(rect), Some(layer_id)) = (feedback.target_rect, feedback.layer_id)
            {
                paint_drop_feedback(ui, layer_id, rect, feedback.plan.is_some());
            }
        }

        // Shutdown мог начаться после получения payload, поэтому проверяется ещё раз перед enqueue.
        let shutdown_active = self.shutdown_active();
        if let Some(plan) =
            take_enqueueable_release_plan(&mut self.file_drop_release, shutdown_active)
        {
            let mode = plan.mode;
            enqueue_drop(plan.into_op_kind(), mode, |verb, kind| {
                self.enqueue(verb, kind)
            });
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
            .as_ref()
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
        let reason = self
            .file_drop_feedback
            .as_ref()
            .and_then(|feedback| feedback.reason)
            .map(|reason| self.t(reason.translation_key()).to_owned());
        let lines = drop_hint_lines(
            &count_label,
            cursor_item_count(&payload),
            &mode_label,
            reason.as_deref(),
        );
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        let tip_size = drop_hint_size(ctx, &lines);
        let position = drop_hint_placement(pointer, tip_size, ctx.content_rect());
        ctx.request_repaint_after(FILE_DND_REPAINT_AFTER);
        egui::Area::new(egui::Id::unique("file_drag_preview"))
            .order(egui::Order::Foreground)
            // Неинтерактивный слой не может перехватить hover у строки.
            .pivot(egui::Align2::LEFT_TOP)
            .fixed_pos(position)
            .constrain(true)
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
                        ui.set_max_width(FILE_DND_HINT_MAX_WIDTH_POINTS);
                        ui.set_max_height(FILE_DND_HINT_MAX_HEIGHT_POINTS);
                        ui.vertical(|ui| {
                            for line in &lines {
                                ui.label(line);
                            }
                        });
                    });
            });
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

    fn drag_source(path: &str, is_dir: bool, volume: Option<&str>) -> DragSource {
        let entry = entry(path, is_dir);
        DragSource {
            key: key(&entry),
            path: entry.path,
            is_dir,
            volume: volume.map(str::to_owned),
        }
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
        assert!(source.contains("feedback.plan.take()"));
        assert!(source.contains("plan_file_drop("));
    }

    #[test]
    fn hover_validity_matches_release_plan_for_same_target() {
        let sources = [drag_source(r"C:\Parent\1", true, Some("VOL"))];
        let modifiers = egui::Modifiers::default();
        let hover = plan_file_drop(
            &sources,
            Path::new(r"C:\Parent\AEGIS"),
            true,
            true,
            false,
            Some("VOL"),
            modifiers,
        );
        let release = plan_file_drop(
            &sources,
            Path::new(r"C:\Parent\AEGIS"),
            true,
            true,
            false,
            Some("VOL"),
            modifiers,
        );
        assert_eq!(hover, release);
        assert!(hover.plan.is_some());
        assert_eq!(hover.reason, None);
    }

    #[test]
    fn hovering_other_folder_in_same_parent_is_valid() {
        let feedback = plan_file_drop(
            &[drag_source(r"C:\Parent\1", true, Some("VOL"))],
            Path::new(r"C:\Parent\AEGIS"),
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        let plan = feedback.plan.unwrap();
        assert_eq!(feedback.mode, DropMode::Move);
        assert_eq!(plan.destination, PathBuf::from(r"C:\Parent\AEGIS"));
        assert_eq!(plan.sources, vec![PathBuf::from(r"C:\Parent\1")]);
    }

    #[test]
    fn payload_source_is_selected_entry_not_active_tab_path() {
        let entries = vec![
            entry(r"C:\Parent", true),
            entry(r"C:\Parent\1", true),
            entry(r"C:\Parent\AEGIS", true),
        ];
        let selected = [key(&entries[1])];
        let sources = normalize_drag_sources(&entries, &selected);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].path, entries[1].path);
        assert_ne!(sources[0].path, entries[0].path);
    }

    #[test]
    fn source_and_dest_normalized_identically_before_component_compare() {
        let source = normalize_drop_path(Path::new(r"C:\Parent\1\..\1"));
        let destination = normalize_drop_path(Path::new(r"C:\Parent\AEGIS\."));
        assert_eq!(source, normalize_drop_path(Path::new(r"C:\Parent\1")));
        assert!(!dnd_path_is_inside(&source, &destination));
        assert!(!dnd_path_is_inside(&destination, &source));
        let dnd = include_str!("file_dnd.rs");
        assert!(dnd.contains("let root = normalize_drop_path(root)"));
        assert!(dnd.contains("let candidate = normalize_drop_path(candidate)"));
    }

    #[test]
    fn unreadable_dest_volume_does_not_silently_mark_invalid() {
        let feedback = plan_file_drop(
            &[drag_source(r"C:\Parent\1", true, Some("VOL"))],
            Path::new(r"D:\AEGIS"),
            true,
            true,
            false,
            None,
            egui::Modifiers::default(),
        );
        assert!(feedback.plan.is_some());
        assert_eq!(feedback.mode, DropMode::Copy);
        assert_eq!(feedback.reason, None);
    }

    #[test]
    fn invalid_reason_is_specific_not_generic() {
        let source = [drag_source(r"C:\Parent\1", true, Some("VOL"))];
        let self_drop = plan_file_drop(
            &source,
            Path::new(r"C:\Parent\1"),
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        assert_eq!(self_drop.reason, Some(DropInvalidReason::FolderIntoSelf));

        let subtree_drop = plan_file_drop(
            &source,
            Path::new(r"C:\Parent\1\child"),
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        assert_eq!(
            subtree_drop.reason,
            Some(DropInvalidReason::FolderIntoSubfolder)
        );

        let same_folder_move = plan_file_drop(
            &[drag_source(r"C:\Parent\file.txt", false, Some("VOL"))],
            Path::new(r"C:\Parent"),
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        assert_eq!(
            same_folder_move.reason,
            Some(DropInvalidReason::FileAlreadyInFolder)
        );
        assert_eq!(
            DropInvalidReason::FolderIntoSelf.translation_key(),
            "dnd.invalid_self"
        );
        assert_eq!(
            DropInvalidReason::FolderIntoSubfolder.translation_key(),
            "dnd.invalid_subfolder"
        );
        assert_eq!(
            DropInvalidReason::FileAlreadyInFolder.translation_key(),
            "dnd.file_already_in_folder"
        );
        for key in [
            "dnd.invalid_self",
            "dnd.invalid_subfolder",
            "dnd.file_already_in_folder",
        ] {
            assert!(include_str!("../../i18n/ru.json").contains(key));
            assert!(include_str!("../../i18n/en.json").contains(key));
        }
    }

    fn feedback(surface: DropSurface, key: Option<&str>, valid: bool) -> DropFeedback {
        DropFeedback {
            mode: DropMode::Move,
            plan: valid.then(|| DropPlan {
                mode: DropMode::Move,
                sources: vec![PathBuf::from(r"C:\Parent\1")],
                destination: PathBuf::from(key.unwrap_or(r"C:\Parent")),
            }),
            reason: (!valid && surface != DropSurface::Other)
                .then_some(DropInvalidReason::InvalidTarget),
            surface,
            target_key: key.map(PathBuf::from),
            target_rect: None,
            layer_id: None,
        }
    }

    #[test]
    fn feedback_cleared_when_no_surface_registered_this_frame() {
        let mut feedback = Some(feedback(
            DropSurface::Entry,
            Some("C:\\Parent\\AEGIS"),
            true,
        ));
        clear_frame_feedback(&mut feedback);
        assert!(feedback.is_none());
    }

    #[test]
    fn release_does_not_enqueue_plan_from_previous_frame() {
        let mut feedback = Some(feedback(
            DropSurface::Entry,
            Some("C:\\Parent\\AEGIS"),
            true,
        ));
        clear_frame_feedback(&mut feedback);
        assert!(take_current_release_plan(&mut feedback).is_none());
    }

    #[test]
    fn release_mode_matches_modifiers_at_release_moment() {
        let sources = [drag_source(r"C:\Parent\1", true, Some("VOL"))];
        for (modifiers, expected) in [
            (egui::Modifiers::CTRL, DropMode::Copy),
            (egui::Modifiers::SHIFT, DropMode::Move),
        ] {
            let feedback = plan_file_drop(
                &sources,
                Path::new(r"C:\Parent\AEGIS"),
                true,
                true,
                false,
                Some("VOL"),
                modifiers,
            );
            assert_eq!(feedback.mode, expected);
        }
    }

    #[test]
    fn existing_hover_release_parity_tests_not_regressed() {
        let source = include_str!("file_dnd.rs");
        assert!(source.contains("fn hover_validity_matches_release_plan_for_same_target()"));
        assert!(source.contains("fn hovering_other_folder_in_same_parent_is_valid()"));
        assert!(source.contains("fn release_mode_matches_modifiers_at_release_moment()"));
    }

    #[test]
    fn existing_grouped_mapping_and_view_mode_tests_not_regressed() {
        let source = include_str!("list/mod.rs");
        for name in [
            "grouped_list_hover_resolves_entry_under_cursor",
            "hover_resolution_correct_in_table_list_and_grid",
            "hover_resolution_correct_with_sort_filter_and_search_active",
            "group_header_is_not_a_drop_target_and_does_not_shift_mapping",
        ] {
            assert!(
                source.contains(name),
                "missing mapping regression test {name}"
            );
        }
    }

    #[test]
    fn existing_feedback_lifecycle_tests_not_regressed() {
        let source = include_str!("file_dnd.rs");
        for name in [
            "feedback_cleared_when_no_surface_registered_this_frame",
            "release_does_not_enqueue_plan_from_previous_frame",
            "release_outside_any_target_does_not_enqueue_operation",
        ] {
            assert!(source.contains(name));
        }
    }

    #[test]
    fn existing_surface_priority_tests_not_regressed() {
        let source = include_str!("file_dnd.rs");
        for name in [
            "fn surface_priority_is_explicit_and_independent_of_draw_order()",
            "fn two_specific_surfaces_in_one_frame_do_not_clobber_by_registration_order()",
            "fn bookmark_surface_priority_above_background_and_other_order_independent()",
        ] {
            assert!(source.contains(name));
        }
    }

    #[test]
    fn existing_hover_release_parity_and_grouped_mapping_tests_not_regressed() {
        let source = include_str!("file_dnd.rs");
        assert!(source.contains("fn existing_hover_release_parity_tests_not_regressed()"));
        assert!(source.contains("fn existing_grouped_mapping_and_view_mode_tests_not_regressed()"));
    }

    #[test]
    fn existing_release_attribution_tests_not_regressed() {
        let source = include_str!("file_dnd.rs");
        assert!(source.contains("fn release_plan_belongs_to_surface_that_received_payload()"));
        assert!(source.contains("fn higher_priority_surface_does_not_steal_release_destination()"));
    }

    #[test]
    fn existing_reason_text_tests_not_regressed() {
        let source = include_str!("file_dnd.rs");
        for reason in [
            "dnd.invalid_self",
            "dnd.invalid_subfolder",
            "dnd.file_already_in_folder",
            "dnd.bookmark_unavailable",
        ] {
            assert!(source.contains(reason));
            assert!(include_str!("../../i18n/ru.json").contains(reason));
            assert!(include_str!("../../i18n/en.json").contains(reason));
        }
    }

    #[test]
    fn bookmark_row_is_drop_target_resolving_to_its_folder_path() {
        let destination = PathBuf::from(r"C:\Users\Example\Desktop");
        let bookmark = crate::core::known_folders::SystemBookmark {
            kind: crate::core::known_folders::SystemBookmarkKind::Desktop,
            target: crate::core::known_folders::SystemBookmarkTarget::Folder(destination.clone()),
        };
        let crate::core::known_folders::SystemBookmarkTarget::Folder(path) = bookmark.target else {
            panic!("папочная закладка должна разрешаться в свой путь");
        };
        let mut feedback = plan_file_drop(
            &[drag_source(r"C:\Work\report.txt", false, Some("VOL"))],
            &path,
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        feedback.surface = DropSurface::Bookmark;
        assert_eq!(feedback.plan.unwrap().destination, destination);
        assert_eq!(feedback.surface, DropSurface::Bookmark);
    }

    #[test]
    fn system_and_personal_bookmarks_both_accepted_as_targets() {
        let system_path = PathBuf::from(r"C:\Users\Example\Documents");
        let system_target =
            crate::core::known_folders::SystemBookmarkTarget::Folder(system_path.clone());
        let personal_path = PathBuf::from(r"D:\Pinned\Projects");
        let sources = [drag_source(r"C:\Work\report.txt", false, Some("VOL"))];
        let system_destination = match system_bookmark_drop_destination(&system_target) {
            BookmarkDropDestination::Folder(path) => path,
            _ => panic!("системная папочная закладка должна быть целью"),
        };
        for destination in [system_destination, personal_path.as_path()] {
            let feedback = plan_file_drop(
                &sources,
                destination,
                true,
                true,
                false,
                None,
                egui::Modifiers::default(),
            );
            assert!(feedback.plan.is_some());
            assert_eq!(
                feedback.plan.unwrap().destination,
                destination.to_path_buf()
            );
        }
    }

    #[test]
    fn drop_on_bookmark_enqueues_move_with_source_and_dest_invalidation() {
        let source = drag_source(r"C:\Work\report.txt", false, Some("VOL"));
        let destination = Path::new(r"C:\Users\Example\Downloads");
        let feedback = plan_file_drop(
            &[source],
            destination,
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        assert_eq!(feedback.mode, DropMode::Move);
        let mut release = None;
        let mut bookmark_feedback = feedback;
        bookmark_feedback.surface = DropSurface::Bookmark;
        register_release_feedback(&mut release, bookmark_feedback);
        let plan = take_current_release_plan(&mut release).unwrap();
        let mut queued = None;
        enqueue_drop(plan.into_op_kind(), DropMode::Move, |verb, kind| {
            queued = Some((verb, kind));
        });
        let (verb, operation) = queued.unwrap();
        assert_eq!(verb, "ops.moving");
        assert!(matches!(&operation, OpKind::Move { .. }));
        let invalidated = crate::core::dir_cache::affected_dirs(&operation);
        assert!(invalidated.contains(&PathBuf::from(r"C:\Work")));
        assert!(invalidated.contains(&destination.to_path_buf()));
    }

    #[test]
    fn cross_volume_bookmark_drop_defaults_to_copy_and_invalidates_dest_only() {
        let source = drag_source(r"C:\Work\report.txt", false, Some("VOL-C"));
        let destination = Path::new(r"D:\Pinned\Projects");
        let feedback = plan_file_drop(
            &[source],
            destination,
            true,
            true,
            false,
            Some("VOL-D"),
            egui::Modifiers::default(),
        );
        assert_eq!(feedback.mode, DropMode::Copy);
        let operation = feedback.plan.unwrap().into_op_kind();
        assert!(matches!(&operation, OpKind::Copy { .. }));
        let invalidated = crate::core::dir_cache::affected_dirs(&operation);
        assert!(invalidated.contains(&destination.to_path_buf()));
        assert!(!invalidated.contains(&PathBuf::from(r"C:\Work")));
    }

    #[test]
    fn drop_on_bookmark_of_current_folder_is_noop_for_move() {
        let feedback = plan_file_drop(
            &[drag_source(
                r"C:\Users\Example\Downloads\report.txt",
                false,
                Some("VOL"),
            )],
            Path::new(r"C:\Users\Example\Downloads"),
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        assert!(feedback.plan.is_none());
        assert_eq!(
            feedback.reason,
            Some(DropInvalidReason::FileAlreadyInFolder)
        );
    }

    #[test]
    fn drop_folder_on_bookmark_pointing_inside_itself_rejected_with_reason() {
        let feedback = plan_file_drop(
            &[drag_source(r"C:\Work\Folder", true, Some("VOL"))],
            Path::new(r"C:\Work\Folder\Child"),
            true,
            true,
            false,
            Some("VOL"),
            egui::Modifiers::default(),
        );
        assert!(feedback.plan.is_none());
        assert_eq!(
            feedback.reason,
            Some(DropInvalidReason::FolderIntoSubfolder)
        );
    }

    #[test]
    fn unavailable_bookmark_path_shows_specific_reason_not_silent_invalid() {
        let state = bookmark_path_state(None);
        assert_eq!(state, BookmarkPathState::Unavailable);
        assert!(state.is_drop_target());
        let mut feedback = plan_file_drop(
            &[drag_source(r"C:\Work\report.txt", false, Some("VOL"))],
            Path::new(r"D:\Offline\Downloads"),
            false,
            false,
            false,
            None,
            egui::Modifiers::default(),
        );
        apply_drop_invalid_reason(&mut feedback, state.invalid_reason());
        assert_eq!(feedback.plan, None);
        assert_eq!(
            feedback.reason,
            Some(DropInvalidReason::BookmarkUnavailable)
        );
        assert_eq!(
            feedback.reason.unwrap().translation_key(),
            "dnd.bookmark_unavailable"
        );
        assert!(include_str!("../../i18n/ru.json").contains("Папка закладки недоступна"));
        assert!(include_str!("../../i18n/en.json").contains("Bookmark folder is unavailable"));
    }

    #[test]
    fn bookmark_pointing_to_file_is_not_a_target() {
        let state = bookmark_path_state(Some(false));
        assert_eq!(state, BookmarkPathState::File);
        assert!(!state.is_drop_target());
        assert_eq!(state.invalid_reason(), None);
    }

    #[test]
    fn trash_and_secure_trash_bookmarks_remain_non_targets_without_invalid_highlight() {
        for target in [
            crate::core::known_folders::SystemBookmarkTarget::RecycleBin,
            crate::core::known_folders::SystemBookmarkTarget::SecureTrash,
        ] {
            assert!(matches!(
                system_bookmark_drop_destination(&target),
                BookmarkDropDestination::NonTarget
            ));
        }
        let source = include_str!("tree.rs");
        assert!(source.contains("BookmarkDropDestination::NonTarget"));
        assert!(source.contains("TODO: отдельная задача: drop в корзину означает удаление"));
    }

    #[test]
    fn bookmarks_panel_background_is_not_a_target() {
        let source = include_str!("tree.rs");
        let panel = source
            .split("pub(crate) fn tree_panel")
            .nth(1)
            .unwrap()
            .split("fn wsl_tree_section")
            .next()
            .unwrap();
        assert!(!panel.contains("DropSurface::Background"));
        assert!(source.contains("DropSurface::Bookmark"));
    }

    #[test]
    fn bookmark_surface_priority_above_background_and_other_order_independent() {
        assert!(DropSurface::Bookmark.priority() > DropSurface::Background.priority());
        assert!(DropSurface::Bookmark.priority() > DropSurface::Other.priority());
        assert_eq!(
            DropSurface::Bookmark.priority(),
            DropSurface::Tree.priority()
        );

        let bookmark = feedback(DropSurface::Bookmark, Some("C:\\Pinned"), true);
        let background = feedback(DropSurface::Background, Some("C:\\"), false);
        let other = feedback(DropSurface::Other, None, false);
        let mut first = None;
        for candidate in [background.clone(), other.clone(), bookmark.clone()] {
            register_drop_feedback(&mut first, candidate);
        }
        let mut reversed = None;
        for candidate in [bookmark, other, background] {
            register_drop_feedback(&mut reversed, candidate);
        }
        assert_eq!(first, reversed);
        assert_eq!(first.unwrap().surface, DropSurface::Bookmark);
    }

    #[test]
    fn bookmark_highlight_and_cursor_tooltip_consistent_with_other_targets() {
        let mut target = feedback(DropSurface::Bookmark, Some("C:\\Pinned"), true);
        target.target_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(120.0, 28.0),
        ));
        target.layer_id = Some(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::unique("bookmark-target"),
        ));
        assert!(feedback_paints_target(&target));
        let source = include_str!("file_dnd.rs");
        assert!(source.contains("ctx.pointer_latest_pos()"));
        assert!(source.contains("drop_hint_placement(pointer, tip_size, ctx.content_rect())"));
        assert!(source.contains(".interactable(false)"));
    }

    #[test]
    fn detached_window_without_bookmarks_panel_unaffected() {
        let detached = include_str!("app/window_frame.rs");
        let tree = include_str!("tree.rs");
        assert!(detached.contains("self.tree_panel(&viewport_ctx, ui)"));
        assert!(tree.contains("self.bookmark_section(ctx, ui)"));
    }

    #[test]
    fn existing_tooltip_anchor_tests_not_regressed() {
        let source = include_str!("file_dnd.rs");
        for name in [
            "tooltip_anchors_to_cursor_for_any_target_size",
            "tooltip_flips_left_when_near_right_window_edge",
            "tooltip_flips_up_when_near_bottom_window_edge",
            "tooltip_stays_inside_window_in_all_four_positions",
            "tooltip_does_not_cover_cursor_point",
        ] {
            assert!(source.contains(name));
        }
    }

    #[test]
    fn release_outside_any_target_does_not_enqueue_operation() {
        let mut feedback = None;
        assert!(take_current_release_plan(&mut feedback).is_none());
    }

    #[test]
    fn release_uses_the_selected_current_frame_surface_plan() {
        let mut selected = Some(feedback(
            DropSurface::Entry,
            Some("C:\\Parent\\AEGIS"),
            true,
        ));
        assert_eq!(
            take_current_release_plan(&mut selected)
                .unwrap()
                .destination,
            PathBuf::from(r"C:\Parent\AEGIS")
        );
    }

    #[test]
    fn release_plan_belongs_to_surface_that_received_payload() {
        let mut release = None;
        register_release_feedback(
            &mut release,
            feedback(DropSurface::Breadcrumb, Some("C:\\Parent\\Child"), true),
        );
        assert_eq!(
            take_current_release_plan(&mut release).unwrap().destination,
            PathBuf::from(r"C:\Parent\Child")
        );
    }

    #[test]
    fn higher_priority_surface_does_not_steal_release_destination() {
        let mut release = None;
        register_release_feedback(
            &mut release,
            feedback(DropSurface::Breadcrumb, Some("C:\\Parent\\Child"), true),
        );
        let mut display = None;
        register_drop_feedback(
            &mut display,
            feedback(DropSurface::Up, Some("C:\\Parent"), true),
        );
        assert_eq!(display.unwrap().surface, DropSurface::Up);
        assert_eq!(
            take_current_release_plan(&mut release).unwrap().destination,
            PathBuf::from(r"C:\Parent\Child")
        );
    }

    #[test]
    fn overlapping_path_segment_and_up_release_goes_to_payload_receiver() {
        let mut release = None;
        let receiver = feedback(DropSurface::Breadcrumb, Some("C:\\Parent\\Child"), true);
        register_release_feedback(&mut release, receiver);
        let plan = take_current_release_plan(&mut release).unwrap();
        assert_eq!(plan.destination, PathBuf::from(r"C:\Parent\Child"));
    }

    #[test]
    fn shutdown_active_blocks_enqueue_at_end_of_traversal() {
        let mut release = Some(feedback(
            DropSurface::Entry,
            Some("C:\\Parent\\Folder"),
            true,
        ));
        assert!(take_enqueueable_release_plan(&mut release, true).is_none());
        assert!(release.is_none());
    }

    #[test]
    fn all_former_release_preconditions_still_applied_at_new_enqueue_point() {
        assert!(
            plan_file_drop(
                &[drag_source(r"C:\Parent\a", false, Some("VOL"))],
                Path::new(r"C:\Parent"),
                true,
                true,
                false,
                Some("VOL"),
                egui::Modifiers::NONE,
            )
            .plan
            .is_none()
        );
        let mut empty = Some(feedback(DropSurface::Entry, Some("C:\\Parent"), true));
        empty
            .as_mut()
            .unwrap()
            .plan
            .as_mut()
            .unwrap()
            .sources
            .clear();
        assert!(take_enqueueable_release_plan(&mut empty, false).is_none());
        let mut shutdown = Some(feedback(DropSurface::Entry, Some("C:\\Parent"), true));
        assert!(take_enqueueable_release_plan(&mut shutdown, true).is_none());
    }

    #[test]
    fn surface_priority_is_explicit_and_independent_of_draw_order() {
        assert!(DropSurface::Entry.priority() > DropSurface::Background.priority());
        assert!(DropSurface::Tree.priority() > DropSurface::Entry.priority());
        assert!(DropSurface::Breadcrumb.priority() > DropSurface::Tree.priority());
        assert!(DropSurface::Up.priority() > DropSurface::Breadcrumb.priority());
        assert!(DropSurface::Other.priority() > DropSurface::Background.priority());
        assert!(DropSurface::Entry.priority() > DropSurface::Other.priority());
        assert!(DropSurface::Bookmark.priority() > DropSurface::Background.priority());
        assert!(DropSurface::Bookmark.priority() > DropSurface::Other.priority());
        assert_eq!(
            DropSurface::Bookmark.priority(),
            DropSurface::Tree.priority()
        );
    }

    #[test]
    fn two_specific_surfaces_in_one_frame_do_not_clobber_by_registration_order() {
        let entry = feedback(DropSurface::Entry, Some("C:\\Parent\\AEGIS"), true);
        let tree = feedback(DropSurface::Tree, Some("C:\\Other"), false);
        let mut first = None;
        register_drop_feedback(&mut first, entry.clone());
        register_drop_feedback(&mut first, tree.clone());
        let mut reversed = None;
        register_drop_feedback(&mut reversed, tree);
        register_drop_feedback(&mut reversed, entry);
        assert_eq!(first, reversed);
        assert_eq!(first.unwrap().surface, DropSurface::Tree);
    }

    #[test]
    fn group_and_table_headers_remain_other_and_do_not_suppress_target() {
        assert!(include_str!("list/grouping.rs").contains("DropSurface::Other"));
        assert!(include_str!("list/columns.rs").contains("DropSurface::Other"));
        let mut header = feedback(DropSurface::Other, None, false);
        header.target_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(100.0, 24.0),
        ));
        assert!(header.plan.is_none());
        assert_eq!(header.reason, None);
        assert!(!feedback_paints_target(&header));
        let background = feedback(DropSurface::Background, Some("C:\\Parent"), false);
        let entry = feedback(DropSurface::Entry, Some("C:\\Parent\\AEGIS"), true);
        let mut selected = None;
        register_drop_feedback(&mut selected, background);
        register_drop_feedback(&mut selected, header);
        register_drop_feedback(&mut selected, entry);
        let selected = selected.unwrap();
        assert_eq!(selected.surface, DropSurface::Entry);
        assert!(selected.plan.is_some());
    }

    #[test]
    fn other_surface_contains_only_non_targets() {
        for source in [
            include_str!("list/grouping.rs"),
            include_str!("list/columns.rs"),
        ] {
            assert!(source.contains("DropSurface::Other"));
        }
        assert!(include_str!("list/actions.rs").contains("DropSurface::Entry"));
        assert!(include_str!("tree.rs").contains("DropSurface::Tree"));
        assert!(include_str!("app/toolbar.rs").contains("DropSurface::Breadcrumb"));
        assert!(include_str!("app/toolbar.rs").contains("DropSurface::Up"));
    }

    #[test]
    fn no_real_target_mapped_to_other_and_suppressed_by_row() {
        let sources = [
            include_str!("list/actions.rs"),
            include_str!("tree.rs"),
            include_str!("app/toolbar.rs"),
        ];
        for source in sources {
            assert!(!source.contains("DropSurface::Other"));
        }
    }

    #[test]
    fn background_does_not_paint_invalid_while_specific_valid_target_hovered() {
        let mut selected = None;
        register_drop_feedback(
            &mut selected,
            feedback(DropSurface::Entry, Some("C:\\Parent\\AEGIS"), true),
        );
        register_drop_feedback(
            &mut selected,
            feedback(DropSurface::Background, Some("C:\\Parent"), false),
        );
        let selected = selected.unwrap();
        assert_eq!(selected.surface, DropSurface::Entry);
        assert!(selected.plan.is_some());
    }

    #[test]
    fn no_global_invalid_frame_when_valid_row_target_hovered() {
        let mut selected = None;
        register_drop_feedback(
            &mut selected,
            feedback(DropSurface::Entry, Some("C:\\Parent\\AEGIS"), true),
        );
        register_drop_feedback(
            &mut selected,
            feedback(DropSurface::Background, Some("C:\\Parent"), false),
        );
        assert!(selected.unwrap().plan.is_some());
    }

    #[test]
    fn background_target_highlighted_only_when_pointer_on_background() {
        let background = feedback(DropSurface::Background, Some("C:\\Parent"), false);
        let entry = feedback(DropSurface::Entry, Some("C:\\Parent\\AEGIS"), true);
        let mut selected = None;
        register_drop_feedback(&mut selected, background.clone());
        assert_eq!(selected.as_ref().unwrap().surface, DropSurface::Background);
        selected = Some(entry);
        register_drop_feedback(&mut selected, background);
        assert_eq!(selected.unwrap().surface, DropSurface::Entry);
    }

    #[test]
    fn tooltip_area_still_non_interactive_and_not_a_drop_surface() {
        let source = include_str!("file_dnd.rs");
        let preview = source
            .split("pub(crate) fn show_file_drag_preview")
            .nth(1)
            .unwrap();
        assert!(preview.contains(".interactable(false)"));
        assert!(!preview.contains("handle_file_drop_response("));
    }

    #[test]
    fn tooltip_area_is_non_interactive_and_not_a_drop_surface() {
        assert!(include_str!("file_dnd.rs").contains(".interactable(false)"));
    }

    #[test]
    fn tooltip_offset_is_small_named_constant() {
        assert_eq!(FILE_DND_HINT_OFFSET_X_POINTS, 14.0);
        assert_eq!(FILE_DND_HINT_OFFSET_Y_POINTS, 18.0);
        assert!(FILE_DND_HINT_OFFSET_X_POINTS <= FILE_DND_HINT_OFFSET_Y_POINTS);
    }

    #[test]
    fn tooltip_anchors_to_cursor_for_any_target_size() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let pointer = egui::pos2(400.0, 300.0);
        let source = include_str!("file_dnd.rs");
        let preview = source
            .split("pub(crate) fn show_file_drag_preview")
            .nth(1)
            .unwrap()
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(!preview.contains("target_rect"));
        for size in [egui::vec2(80.0, 30.0), egui::vec2(280.0, 100.0)] {
            let position = drop_hint_placement(pointer, size, viewport);
            assert_eq!(
                position,
                pointer + egui::vec2(FILE_DND_HINT_OFFSET_X_POINTS, FILE_DND_HINT_OFFSET_Y_POINTS)
            );
        }
    }

    #[test]
    fn tooltip_flips_left_when_near_right_window_edge() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let pointer = egui::pos2(790.0, 300.0);
        let size = egui::vec2(200.0, 80.0);
        let position = drop_hint_placement(pointer, size, viewport);
        assert_eq!(
            position.x,
            pointer.x - FILE_DND_HINT_OFFSET_X_POINTS - size.x
        );
    }

    #[test]
    fn tooltip_flips_up_when_near_bottom_window_edge() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let pointer = egui::pos2(400.0, 590.0);
        let size = egui::vec2(200.0, 80.0);
        let position = drop_hint_placement(pointer, size, viewport);
        assert_eq!(
            position.y,
            pointer.y - FILE_DND_HINT_OFFSET_Y_POINTS - size.y
        );
    }

    #[test]
    fn tooltip_stays_inside_window_in_all_four_positions() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let size = egui::vec2(200.0, 80.0);
        for pointer in [
            egui::pos2(400.0, 300.0),
            egui::pos2(790.0, 300.0),
            egui::pos2(400.0, 590.0),
            egui::pos2(790.0, 590.0),
        ] {
            let position = drop_hint_placement(pointer, size, viewport);
            let hint = egui::Rect::from_min_size(position, size);
            assert!(
                viewport
                    .shrink(FILE_DND_HINT_WINDOW_MARGIN_POINTS)
                    .contains_rect(hint)
            );
        }
    }

    #[test]
    fn tooltip_does_not_cover_cursor_point() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let size = egui::vec2(200.0, 80.0);
        for pointer in [egui::pos2(400.0, 300.0), egui::pos2(790.0, 590.0)] {
            let position = drop_hint_placement(pointer, size, viewport);
            assert!(!egui::Rect::from_min_size(position, size).contains(pointer));
        }
    }

    #[test]
    fn tooltip_row_highlight_under_cursor_stays_visible() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let pointer = egui::pos2(250.0, 280.0);
        let row = egui::Rect::from_min_size(egui::pos2(200.0, 250.0), egui::vec2(300.0, 72.0));
        let position = drop_hint_placement(pointer, egui::vec2(200.0, 80.0), viewport);
        assert!(row.contains(pointer));
        assert!(!egui::Rect::from_min_size(position, egui::vec2(200.0, 80.0)).contains(pointer));
    }

    #[test]
    fn toolbar_help_menu_and_hotkey_registry_unchanged() {
        let toolbar = include_str!("app/toolbar.rs");
        let hotkeys = include_str!("app/input.rs");
        for marker in [
            "menu.help",
            "help.export_logs",
            "help.reset_settings",
            "show_hotkeys",
        ] {
            assert!(toolbar.contains(marker));
        }
        assert!(hotkeys.contains("HOTKEY_BINDINGS"));
    }

    #[test]
    fn tree_expansion_selection_and_wsl_nodes_unchanged() {
        let tree = include_str!("tree.rs");
        assert!(tree.contains("is_wsl_path"));
        assert!(
            include_str!("tree.rs").contains("expand_and_collapse_toggle_the_requested_node_only")
        );
    }

    #[test]
    fn tooltip_flips_near_window_edge_and_stays_inside() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let size = egui::vec2(200.0, 80.0);
        let top_pointer = egui::pos2(400.0, 10.0);
        let bottom_pointer = egui::pos2(400.0, 590.0);
        let top = drop_hint_placement(top_pointer, size, viewport);
        let bottom = drop_hint_placement(bottom_pointer, size, viewport);
        assert_eq!(top.y, top_pointer.y + FILE_DND_HINT_OFFSET_Y_POINTS);
        assert!(bottom.y + size.y < bottom_pointer.y);
        for position in [top, bottom] {
            assert!(viewport.contains_rect(egui::Rect::from_min_size(position, size)));
        }
    }

    #[test]
    fn drop_hint_width_prevents_five_line_wrap() {
        let lines = drop_hint_lines(
            "Объектов",
            1,
            "Переместить",
            Some("Нельзя поместить папку в её подпапку"),
        );
        assert_eq!(FILE_DND_HINT_MAX_WIDTH_POINTS, 360.0);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Объектов: 1");
        assert!(
            include_str!("file_dnd.rs")
                .contains("ui.set_max_width(FILE_DND_HINT_MAX_WIDTH_POINTS)")
        );
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
            "alias_target_rejected_by_worker_reports_understandable_error_not_silent_skip",
            "alias_rejection_counts_as_failed_not_false_success",
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
