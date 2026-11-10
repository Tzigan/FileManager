mod archive;
mod dialogs;
mod input;
mod navigation;
mod preview;
mod search;
mod tab_drag_preview;
mod toolbar;
mod window_frame;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;

use super::style;
use crate::core::fs::{self, FileEntry, ReadDirError, SortKey};
use crate::core::i18n::{I18n, Language};
use crate::core::ops::{self, OpEvent, OpJob, OpReply};
use crate::core::paths::AppDirs;
use crate::core::preview::TextPreview;
use crate::core::search::SearchValidationError;
use crate::core::settings::{SettingsStore, Theme, ViewMode};
use crate::core::update::UpdateManifest;
use crate::platform::{drives, install_context_menu, pipe_listen, uninstall_context_menu};
use crate::ui::opsui::OpsUi;
use crate::ui::tabs::{ListStatus, Tab};
use crate::ui::tree::TreeState;

/// Results produced by background worker threads.
pub(crate) enum WorkerMsg {
    /// A directory listing finished for a tab.
    Listing {
        tab_id: u64,
        generation: u64,
        result: Result<Vec<FileEntry>, ReadDirError>,
    },
    /// Завершился отложенный поиск по имени для вкладки.
    SearchResults {
        tab_id: u64,
        generation: u64,
        result: Result<Vec<FileEntry>, ReadDirError>,
    },
    /// Завершилось чтение ограниченного текстового предпросмотра.
    TextPreview {
        generation: u64,
        path: PathBuf,
        result: std::io::Result<TextPreview>,
    },
    /// Завершилось декодирование изображения для предпросмотра.
    ImagePreview {
        generation: u64,
        path: PathBuf,
        result: std::io::Result<crate::platform::DecodedImage>,
    },
    /// Завершилась фоновая генерация эскиза.
    ThumbnailReady {
        key: ThumbnailKey,
        result: std::io::Result<crate::platform::DecodedImage>,
    },
    /// Завершилось извлечение элемента из ZIP.
    ZipExtracted {
        generation: u64,
        result: std::io::Result<PathBuf>,
    },
    /// A folder tree node's children finished loading.
    TreeChildren {
        path: PathBuf,
        result: std::io::Result<Vec<PathBuf>>,
    },
    /// A path forwarded by another instance / CLI — open in a new tab.
    OpenPath(PathBuf),
    /// Найдена более новая версия приложения.
    UpdateFound { manifest: UpdateManifest },
    /// Получено обновление прогресса скачивания.
    UpdateProgress { downloaded: u64, total: u64 },
    /// Проверенный пакет подготовлен для установки отдельным updater.
    UpdateReady(PathBuf),
    /// При проверке вручную установлена последняя версия.
    UpdateLatest,
    /// Проверка или загрузка обновления завершилась ошибкой.
    UpdateFailed(String),
}

struct PreviewState {
    target: Option<PathBuf>,
    text: Option<TextPreview>,
    image: Option<egui::TextureHandle>,
    image_dimensions: Option<(u32, u32)>,
    error: Option<String>,
    loading: bool,
    viewport_placed: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            target: None,
            text: None,
            image: None,
            image_dimensions: None,
            error: None,
            loading: false,
            viewport_placed: false,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl PreviewState {
    fn clear(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.wrapping_add(1);
        self.target = None;
        self.text = None;
        self.image = None;
        self.image_dimensions = None;
        self.error = None;
        self.loading = false;
        self.viewport_placed = false;
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ThumbnailKey {
    path: PathBuf,
    size: u64,
    modified_nanos: u128,
}

struct ThumbnailRequest {
    key: ThumbnailKey,
    cancel: Arc<AtomicBool>,
}

pub struct FileManagerApp {
    pub(crate) store: SettingsStore,
    pub(crate) i18n: I18n,
    dirs: AppDirs,
    show_about: bool,
    about_viewport_placed: bool,
    show_settings: bool,
    settings_viewport_placed: bool,
    style_configured: bool,
    update_startup_signaled: bool,
    /// Update offer: (version, exe URL).
    pending_update: Option<UpdateManifest>,
    update_busy: bool,
    update_downloaded: u64,
    update_total: u64,
    update_ready: Option<PathBuf>,
    update_status: Option<String>,
    /// Ошибка обновления, показываемая в окне «О программе».
    update_err: Option<String>,
    /// Index of the tab currently being drag-reordered.
    tab_drag: Option<usize>,
    /// Tab press anchor for manual drag detection (index, press position).
    tab_press: Option<(usize, egui::Pos2)>,
    tab_drag_preview: tab_drag_preview::TabDragPreviewState,
    /// Path shown in the properties dialog.
    pub(crate) props_target: Option<PathBuf>,
    props_viewport_placed: bool,
    preview: PreviewState,
    /// Tabs requested by widgets while the current frame is rendering.
    pending_open_tabs: Vec<PathBuf>,
    /// Drag state for reordering grouped table sections.
    pub(crate) group_drag: Option<(String, String)>,
    pub(crate) group_press: Option<(String, String, egui::Pos2)>,
    thumbnail_tx: SyncSender<ThumbnailRequest>,
    thumbnail_jobs: HashMap<ThumbnailKey, Arc<AtomicBool>>,
    thumbnail_visible: HashSet<ThumbnailKey>,
    thumbnail_cache: HashMap<ThumbnailKey, egui::TextureHandle>,
    thumbnail_order: VecDeque<ThumbnailKey>,
    thumbnail_failed: HashSet<ThumbnailKey>,
    zip_generation: u64,
    zip_busy: bool,
    zip_message: Option<String>,
    detached_tabs: HashMap<u64, window_frame::DetachedTabWindow>,
    tab_detach_error: Option<String>,

    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) tree: TreeState,
    /// Scroll the focused row into view next repaint (set by keyboard nav).
    pub(crate) scroll_to_cursor: bool,
    /// `--select` target: select this entry when the listing arrives.
    pending_select: Option<(u64, String)>,

    msg_tx: Sender<WorkerMsg>,
    msg_rx: Receiver<WorkerMsg>,
    /// Requests to the dedicated folder-tree worker.
    pub(crate) tree_req: Sender<PathBuf>,

    // File-operation queue (v0.4).
    pub(crate) ops_ui: OpsUi,
    pub(crate) ops_tx: Sender<OpJob>,
    pub(crate) ops_evt: Receiver<OpEvent>,
    pub(crate) op_reply: Sender<OpReply>,
}

impl FileManagerApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        store: SettingsStore,
        i18n: I18n,
        dirs: AppDirs,
        initial_path: Option<PathBuf>,
        initial_select: Option<String>,
    ) -> Self {
        let (msg_tx, msg_rx) = channel();
        let (tree_req, tree_req_rx) = channel::<PathBuf>();
        let (ops_tx, jobs_rx) = channel::<OpJob>();
        let (op_reply, reply_rx) = channel::<OpReply>();
        let (events_tx, ops_evt) = channel::<OpEvent>();
        ops::spawn(
            jobs_rx,
            reply_rx,
            events_tx,
            crate::platform::ops_platform(),
        );
        let (thumbnail_tx, thumbnail_rx) = sync_channel::<ThumbnailRequest>(24);
        {
            let msg_tx = msg_tx.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || {
                while let Ok(request) = thumbnail_rx.recv() {
                    if request.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    let result = crate::platform::decode_image(&request.key.path, 128);
                    if request.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    if msg_tx
                        .send(WorkerMsg::ThumbnailReady {
                            key: request.key,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }

        crate::core::update::cleanup_leftovers();
        {
            let msg_tx = msg_tx.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || {
                if let Ok(r) = crate::core::update::check_latest() {
                    if crate::core::update::version_newer(env!("CARGO_PKG_VERSION"), &r.version) {
                        let _ = msg_tx.send(WorkerMsg::UpdateFound {
                            manifest: r,
                        });
                        ctx.request_repaint();
                    }
                }
            });
        }

        // Secondary instances forward their CLI path here via named pipe.
        {
            let msg_tx = msg_tx.clone();
            let ctx = cc.egui_ctx.clone();
            pipe_listen(move |path| {
                let _ = msg_tx.send(WorkerMsg::OpenPath(path));
                ctx.request_repaint();
            });
        }

        // Folder-tree worker: serial, low priority — one directory at a time.
        {
            let msg_tx = msg_tx.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || {
                while let Ok(path) = tree_req_rx.recv() {
                    let result = fs::read_subdirs(&path);
                    if msg_tx.send(WorkerMsg::TreeChildren { path, result }).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }

        let mut paths: Vec<PathBuf> = store
            .settings
            .tabs
            .iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect();
        if paths.is_empty() {
            paths.push(
                directories::UserDirs::new()
                    .map(|u| u.home_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from(".")),
            );
        }
        let active = store.settings.active_tab.min(paths.len() - 1);

        let pins: BTreeSet<PathBuf> = store.settings.pinned_tabs.iter().cloned().collect();
        let mut app = Self {
            store,
            i18n,
            dirs,
            show_about: false,
            about_viewport_placed: false,
            show_settings: false,
            settings_viewport_placed: false,
            style_configured: false,
            update_startup_signaled: false,
            pending_update: None,
            update_busy: false,
            update_downloaded: 0,
            update_total: 0,
            update_ready: None,
            update_status: None,
            update_err: None,
            tab_drag: None,
            tab_press: None,
            tab_drag_preview: tab_drag_preview::TabDragPreviewState::default(),
            props_target: None,
            props_viewport_placed: false,
            preview: PreviewState::default(),
            pending_open_tabs: Vec::new(),
            group_drag: None,
            group_press: None,
            thumbnail_tx,
            thumbnail_jobs: HashMap::new(),
            thumbnail_visible: HashSet::new(),
            thumbnail_cache: HashMap::new(),
            thumbnail_order: VecDeque::new(),
            thumbnail_failed: HashSet::new(),
            zip_generation: 0,
            zip_busy: false,
            zip_message: None,
            detached_tabs: HashMap::new(),
            tab_detach_error: None,
            tabs: paths.into_iter().map(|p| { let mut t = Tab::new(p); t.pinned = pins.contains(&t.path); t }).collect(),
            active,
            tree: TreeState::new(drives()),
            scroll_to_cursor: false,
            pending_select: None,
            msg_tx,
            msg_rx,
            tree_req,
            ops_ui: OpsUi::default(),
            ops_tx,
            ops_evt,
            op_reply,
        };
        // CLI path (Explorer "Open in FileManager") wins over restored tabs.
        if let Some(p) = initial_path {
            app.tabs.push(Tab::new(p));
            app.active = app.tabs.len() - 1;
        }
        if let Some(name) = initial_select {
            app.pending_select = Some((app.tabs[app.active].id, name));
        }
        for i in 0..app.tabs.len() {
            app.navigate_fresh(i, &cc.egui_ctx);
        }
        let tab = &app.tabs[app.active];
        let reveal_path = if tab.archive_prefix.is_some() {
            tab.path.parent().unwrap_or(&tab.path)
        } else {
            &tab.path
        };
        app.tree.reveal(reveal_path);
        app
    }

    pub(crate) fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.i18n.t(key)
    }

    fn set_theme(&mut self, theme: Theme) {
        self.store.settings.theme = theme;
        self.persist();
    }

    fn set_language(&mut self, lang: Language) {
        self.store.settings.language = lang;
        self.i18n.set_language(lang);
        self.persist();
    }

    pub(crate) fn persist(&self) {
        if let Err(e) = self.store.save() {
            tracing::error!("failed to save settings: {e}");
        }
    }

    pub(crate) fn sort_col(&mut self, key: SortKey) {
        let tab = &mut self.tabs[self.active];
        (tab.sort_key, tab.sort_desc) =
            super::list::next_sort_order(tab.sort_key, tab.sort_desc, key);
        let (k, d) = (tab.sort_key, tab.sort_desc);
        fs::sort_entries_by(&mut tab.entries, k, d);
        tab.clear_selection();
    }

    pub(crate) fn set_view(&mut self, mode: ViewMode) {
        self.store.settings.view_mode = mode;
        self.persist();
    }

    // ------------------------------------------------------------------
    // Messages and input
    // ------------------------------------------------------------------

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                WorkerMsg::Listing { tab_id, generation, result } => {
                    let Some(tab) = super::tabs::listing_tab_index(&self.tabs, tab_id, generation) else {
                        continue;
                    };
                    match result {
                        Ok(entries) => {
                            let pending_name =
                                super::tabs::take_pending_selection(&mut self.pending_select, tab_id);
                            let t = &mut self.tabs[tab];
                            t.entries = entries;
                            t.status = ListStatus::Ready;
                            if let Some(name) = pending_name {
                                if let Some(i) = t.entries.iter().position(|e| e.name == name) {
                                    t.selection.insert(i);
                                    t.cursor = i;
                                    t.anchor = Some(i);
                                    self.scroll_to_cursor = true;
                                }
                            }
                        }
                        Err(ReadDirError::Cancelled) => {}
                        Err(ReadDirError::Io(e)) => {
                            tracing::warn!(path = %self.tabs[tab].path.display(), "read_dir failed: {e}");
                            let text = if self.tabs[tab].archive_prefix.is_some() {
                                self.zip_error_text(&e)
                            } else {
                                self.error_text(&e)
                            };
                            let t = &mut self.tabs[tab];
                            t.entries.clear();
                            t.status = ListStatus::Ready;
                            t.error = Some(text);
                        }
                    }
                }
                WorkerMsg::SearchResults {
                    tab_id,
                    generation,
                    result,
                } => {
                    let Some(tab) = self
                        .tabs
                        .iter()
                        .position(|tab| tab.id == tab_id && tab.search.generation == generation)
                    else {
                        continue;
                    };
                    match result {
                        Ok(entries) => {
                            let state = &mut self.tabs[tab];
                            state.entries = entries;
                            state.status = ListStatus::Ready;
                            state.search.running = false;
                            state.search.results_active = true;
                        }
                        Err(ReadDirError::Cancelled) => {
                            self.tabs[tab].search.running = false;
                        }
                        Err(ReadDirError::Io(error)) => {
                            let message = if self.tabs[tab].archive_prefix.is_some()
                                && error.kind() == std::io::ErrorKind::Unsupported
                            {
                                self.t("search.archive_filters").to_owned()
                            } else if self.tabs[tab].archive_prefix.is_some() {
                                self.zip_error_text(&error)
                            } else {
                                self.error_text(&error)
                            };
                            let state = &mut self.tabs[tab];
                            state.status = ListStatus::Ready;
                            state.search.running = false;
                            state.search.validation_error = Some(message);
                        }
                    }
                }
                WorkerMsg::TextPreview {
                    generation,
                    path,
                    result,
                } => {
                    if self.preview.generation != generation
                        || self.preview.target.as_ref() != Some(&path)
                    {
                        continue;
                    }
                    self.preview.loading = false;
                    match result {
                        Ok(text) => {
                            self.preview.text = Some(text);
                            self.preview.image = None;
                            self.preview.image_dimensions = None;
                            self.preview.error = None;
                        }
                        Err(error) => {
                            let key = match error.kind() {
                                std::io::ErrorKind::PermissionDenied => "preview.access_denied",
                                std::io::ErrorKind::NotFound => "preview.not_found",
                                _ => "preview.error",
                            };
                            self.preview.error = Some(self.t(key).to_owned());
                        }
                    }
                }
                WorkerMsg::ImagePreview {
                    generation,
                    path,
                    result,
                } => {
                    if self.preview.generation != generation
                        || self.preview.target.as_ref() != Some(&path)
                    {
                        continue;
                    }
                    self.preview.loading = false;
                    match result {
                        Ok(image) => {
                            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                                [image.width as usize, image.height as usize],
                                &image.rgba,
                            );
                            self.preview.image = Some(ctx.load_texture(
                                format!("preview-{generation}"),
                                color_image,
                                egui::TextureOptions::LINEAR,
                            ));
                            self.preview.image_dimensions = Some((image.width, image.height));
                            self.preview.text = None;
                            self.preview.error = None;
                        }
                        Err(error) => {
                            let key = match error.kind() {
                                std::io::ErrorKind::PermissionDenied => "preview.access_denied",
                                std::io::ErrorKind::NotFound => "preview.not_found",
                                _ => "preview.error",
                            };
                            self.preview.error = Some(self.t(key).to_owned());
                        }
                    }
                }
                WorkerMsg::ThumbnailReady { key, result } => {
                    let Some(cancel) = self.thumbnail_jobs.remove(&key) else {
                        continue;
                    };
                    if cancel.load(Ordering::Relaxed)
                        || !self.thumbnail_visible.contains(&key)
                    {
                        continue;
                    }
                    match result {
                        Ok(decoded) => self.cache_thumbnail(ctx, key, decoded),
                        Err(_) => {
                            self.thumbnail_failed.insert(key);
                        }
                    }
                }
                WorkerMsg::ZipExtracted {
                    generation,
                    result,
                } => {
                    if generation != self.zip_generation {
                        continue;
                    }
                    self.zip_busy = false;
                    self.zip_message = Some(match result {
                        Ok(path) => {
                            format!("{}: {}", self.t("zip.extracted"), path.display())
                        }
                        Err(error) => self.zip_error_text(&error),
                    });
                }
                WorkerMsg::TreeChildren { path, result } => {
                    self.tree.complete_children_load(path, result);
                }
                WorkerMsg::OpenPath(path) => self.open_path_in_new_tab(path, ctx),
                WorkerMsg::UpdateFound { manifest } => {
                    self.update_busy = false;
                    self.update_total = manifest.size;
                    self.update_downloaded = 0;
                    self.update_ready = None;
                    self.pending_update = Some(manifest);
                    self.update_status = None;
                    self.update_err = None;
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateProgress { downloaded, total } => {
                    self.update_downloaded = downloaded;
                    self.update_total = total;
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateReady(stage_dir) => {
                    self.update_busy = false;
                    self.update_downloaded = self.update_total;
                    self.update_ready = Some(stage_dir);
                    self.update_status = None;
                    self.update_err = None;
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateLatest => {
                    self.update_busy = false;
                    self.pending_update = None;
                    self.update_ready = None;
                    self.update_downloaded = 0;
                    self.update_total = 0;
                    self.update_err = None;
                    self.update_status = Some(self.t("update.latest").to_owned());
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateFailed(e) => {
                    self.update_busy = false;
                    tracing::warn!(error = %e, "update operation failed");
                    self.update_ready = None;
                    self.update_status = None;
                    self.update_err = Some(self.t("update.failed").to_owned());
                    ctx.request_repaint();
                }
            }
        }
    }

    pub(super) fn show_active_tab_status(&self, ui: &mut egui::Ui) {
        let tab = &self.tabs[self.active];
        ui.horizontal(|ui| {
            if tab.status == ListStatus::Loading {
                let status = if tab.search.running || tab.search.debounce_at.is_some() {
                    "search.searching"
                } else {
                    "status.loading"
                };
                ui.label(self.t(status).to_owned());
            } else {
                ui.label(format!("{}: {}", self.t("status.items"), tab.entries.len()));
                if !tab.selection.is_empty() {
                    ui.separator();
                    ui.label(format!(
                        "{}: {}",
                        self.t("status.selected"),
                        tab.selection.len()
                    ));
                }
            }
            if let Some(error) = &self.tab_detach_error {
                ui.separator();
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }

    pub(super) fn show_active_file_content(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let (error, empty, loading, searching, no_results) = {
            let tab = &self.tabs[self.active];
            (
                tab.error.clone(),
                tab.entries.is_empty(),
                tab.status == ListStatus::Loading,
                tab.search.running || tab.search.debounce_at.is_some(),
                tab.search.results_active,
            )
        };
        if let Some(error) = error {
            ui.vertical_centered(|ui| {
                ui.add_space(style::SPACE_EMPTY_STATE);
                ui.colored_label(ui.visuals().error_fg_color, error);
                ui.add_space(style::SPACE_MD);
                if ui.button(self.t("toolbar.up").to_owned()).clicked() {
                    self.go_up(ctx);
                }
            });
            return;
        }
        if empty {
            ui.vertical_centered(|ui| {
                ui.add_space(style::SPACE_EMPTY_STATE);
                if loading {
                    ui.spinner();
                    let status = if searching {
                        "search.searching"
                    } else {
                        "status.loading"
                    };
                    ui.label(self.t(status).to_owned());
                } else if no_results {
                    ui.label(self.t("search.no_results").to_owned());
                } else {
                    ui.label(self.t("folder.empty").to_owned());
                }
            });
            return;
        }
        self.file_list(ctx, ui);
    }

    fn error_text(&self, e: &std::io::Error) -> String {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => self.t("error.access_denied").to_owned(),
            std::io::ErrorKind::NotFound => self.t("error.not_found").to_owned(),
            std::io::ErrorKind::InvalidData => self.t("zip.error_invalid").to_owned(),
            std::io::ErrorKind::InvalidInput => self.t("zip.error_path").to_owned(),
            std::io::ErrorKind::AlreadyExists => self.t("zip.error_exists").to_owned(),
            std::io::ErrorKind::StorageFull => self.t("zip.error_space").to_owned(),
            std::io::ErrorKind::WouldBlock => self.t("zip.error_busy").to_owned(),
            _ => format!("{}: {}", self.t("error.open_dir"), e),
        }
    }
}

fn floating_dialog_viewport(
    ctx: &egui::Context,
    title: String,
    inner_size: egui::Vec2,
    already_placed: bool,
) -> egui::ViewportBuilder {
    let mut builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(inner_size)
        .with_resizable(false)
        .with_maximize_button(false)
        .with_always_on_top();
    if !already_placed {
        let parent_rect = ctx
            .input(|i| i.viewport().outer_rect)
            .unwrap_or_else(|| ctx.content_rect());
        let estimated_outer_size = inner_size + egui::vec2(16.0, 40.0);
        builder = builder.with_position(parent_rect.center() - estimated_outer_size / 2.0);
    }
    builder
}

/// Splits a path into clickable breadcrumb segments:
/// `C:\a\b` → `[("C:\", C:\), ("a", C:\a), ("b", C:\a\b)]`.
/// Collects real filesystem paths from egui's dropped-file handles.
pub(crate) fn dropped_paths(files: &[egui::DroppedFileHandle]) -> Vec<PathBuf> {
    files.iter().map(|f| f.path().to_path_buf()).collect()
}

fn is_zip_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("zip"))
}

fn persisted_tab_path(tab: &Tab) -> PathBuf {
    if tab.archive_prefix.is_some() {
        tab.path
            .parent()
            .unwrap_or(&tab.path)
            .to_path_buf()
    } else {
        tab.path.clone()
    }
}

fn parse_archive_address(value: &str) -> Option<(PathBuf, String)> {
    let (archive, prefix) = value.split_once("!/")?;
    let archive = PathBuf::from(archive);
    if !archive
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("zip"))
    {
        return None;
    }
    Some((archive, prefix.trim_matches('/').to_owned()))
}

fn breadcrumb_segments(path: &Path) -> Vec<(String, PathBuf)> {
    let mut segs = Vec::new();
    let mut acc = PathBuf::new();
    let mut components = path.components().peekable();
    while let Some(comp) = components.next() {
        match comp {
            Component::Prefix(prefix) => {
                acc.push(prefix.as_os_str());
                let mut label = prefix.as_os_str().to_string_lossy().into_owned();
                // Treat `C:\` as one breadcrumb, and make its target absolute.
                if matches!(components.peek(), Some(Component::RootDir)) {
                    if let Some(root) = components.next() {
                        acc.push(root.as_os_str());
                    }
                    label = acc.display().to_string();
                }
                segs.push((label, acc.clone()));
            }
            Component::RootDir => {
                acc.push(comp.as_os_str());
                segs.push((acc.display().to_string(), acc.clone()));
            }
            _ => {
                acc.push(comp.as_os_str());
                segs.push((comp.as_os_str().to_string_lossy().into_owned(), acc.clone()));
            }
        }
    }
    segs
}

impl eframe::App for FileManagerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.set_theme(match self.store.settings.theme {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        });
        if !self.style_configured {
            super::style::configure(&ctx);
            self.style_configured = true;
        }
        window_frame::handle_resize_edges(&ctx);
        if !self.update_startup_signaled {
            self.update_startup_signaled = true;
            if let Err(e) = crate::platform::signal_update_started() {
                tracing::warn!("failed to signal successful update startup: {e}");
            }
        }
        self.drain_messages(&ctx);
        self.drain_ops_events(&ctx);
        self.handle_keys(&ctx);
        self.handle_dropped_files(&ctx);

        self.begin_thumbnail_frame();
        egui::Panel::top("window_frame")
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin::symmetric(style::FRAME_PADDING_X, 0)),
            )
            .show(ui, |ui| self.tab_bar(&ctx, ui));
        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(&ctx, ui));
        egui::Panel::top("search_bar").show(ui, |ui| self.search_bar(ui));
        self.dispatch_due_searches(&ctx);

        egui::Panel::left("tree_panel")
            .resizable(true)
            .default_size(230.0)
            .size_range(150.0..=400.0)
            .show(ui, |ui| self.tree_panel(&ctx, ui));

        egui::Panel::right("props_panel")
            .resizable(true)
            .default_size(380.0)
            .size_range(300.0..=500.0)
            .show(ui, |ui| self.props_panel(ui));

        egui::Panel::bottom("ops_panel").show(ui, |ui| self.ops_panel(ui));

        egui::Panel::bottom("status_bar").show(ui, |ui| self.show_active_tab_status(ui));
        egui::CentralPanel::default().show(ui, |ui| self.show_active_file_content(&ctx, ui));

        self.show_tab_drag_preview(&ctx);
        self.flush_pending_open_tabs(&ctx);
        self.show_detached_tab_windows(&ctx);
        self.finish_thumbnail_frame();
        self.about_window(&ctx);
        self.settings_window(&ctx);
        self.props_window(&ctx);
        self.preview_window(&ctx);
        self.zip_status_window(&ctx);
        self.ops_dialogs(&ctx);

        // Keep repainting while operations run so progress stays live.
        if !self.ops_ui.jobs.is_empty()
            || self.ops_ui.conflict.is_some()
            || self.ops_ui.error_q.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

fn tab_scroll_source() -> egui::scroll_area::ScrollSource {
    egui::scroll_area::ScrollSource::MOUSE_WHEEL
}

fn is_tab_dragging(
    active_drag: Option<usize>,
    press: Option<(usize, egui::Pos2)>,
    pointer: Option<egui::Pos2>,
) -> bool {
    active_drag.is_some()
        || matches!((press, pointer), (Some((_, start)), Some(pos)) if pos.distance(start) > window_frame::TAB_DRAG_THRESHOLD)
}

fn tab_close_button_hovered(
    dragging: bool,
    pinned: bool,
    close_rect: egui::Rect,
    pointer: Option<egui::Pos2>,
) -> bool {
    !dragging && !pinned && pointer.is_some_and(|pos| close_rect.contains(pos))
}

fn tab_strip_content_margin() -> egui::Margin {
    egui::Margin::symmetric(style::FRAME_PADDING_X, 0)
}

fn configure_tab_scroll_style(style: &mut egui::Style) {
    // В обычных горизонтальных областях egui прокрутка колесом требует Shift.
    // Для полосы вкладок колесо прокручивает список напрямую.
    style.always_scroll_the_only_direction = true;
    // Высота панели подстраивается к строке, не меняя размеры вкладок и кнопки «+».
    style.spacing.item_spacing.y = super::style::TAB_BAR_VERTICAL_SPACING;
}

#[cfg(test)]
mod tests;

/// Square toolbar button with a painter-drawn glyph — no font glyphs to miss.
fn icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    hover: String,
    draw: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let button_size = style::TOOLBAR_BUTTON_SIZE;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(button_size, button_size), sense);
    let resp = resp.on_hover_text(hover);
    let visuals = ui.visuals();
    let painter = ui.painter();
    if enabled && resp.hovered() {
        painter.rect_filled(rect, style::RADIUS_CONTROL, visuals.widgets.hovered.bg_fill);
    }
    let color = if !enabled {
        visuals.weak_text_color().gamma_multiply(0.4)
    } else if resp.hovered() {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    let icon_inset = (button_size - style::TOOLBAR_ICON_SIZE) * 0.5;
    draw(painter, rect.shrink(icon_inset), color);
    resp
}

fn icon_pin(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let x = rect.center().x;
    let head = egui::pos2(x, rect.top() + 3.0);
    painter.circle_filled(head, 2.2, color);
    painter.line_segment(
        [egui::pos2(x, rect.top() + 5.0), egui::pos2(x, rect.bottom() - 1.0)],
        egui::Stroke::new(1.3, color),
    );
    painter.line_segment(
        [
            egui::pos2(x - 3.0, rect.top() + 7.0),
            egui::pos2(x + 3.0, rect.top() + 7.0),
        ],
        egui::Stroke::new(1.3, color),
    );
}

/// Arrow glyph: `dir` is the tip direction (unit axis vector).
fn icon_arrow(p: &egui::Painter, r: egui::Rect, dir: egui::Vec2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.6, color);
    let c = r.center();
    let tip = c + dir * (r.width() * 0.5);
    let tail = c - dir * (r.width() * 0.42);
    p.line_segment([tail, tip], stroke);
    let perp = egui::vec2(-dir.y, dir.x);
    let w = r.width() * 0.32;
    p.line_segment([tip, tip - dir * w + perp * w], stroke);
    p.line_segment([tip, tip - dir * w - perp * w], stroke);
}

/// Circular arrow (refresh): ~280° arc with a filled triangular tip at top-right.
fn icon_refresh(p: &egui::Painter, r: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.7, color);
    let c = r.center();
    let rad = r.width() * 0.44;
    let pts: Vec<egui::Pos2> = (0..=20)
        .map(|i| {
            let a = 0.9 + std::f32::consts::TAU * 0.78 * (i as f32 / 20.0);
            c + egui::vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    p.add(egui::Shape::line(pts.clone(), stroke));
    let end = *pts.last().unwrap();
    let tan = (end - pts[pts.len() - 2]).normalized();
    let perp = egui::vec2(-tan.y, tan.x);
    p.add(egui::Shape::convex_polygon(
        vec![end, end - tan * 5.0 + perp * 2.6, end - tan * 5.0 - perp * 2.6],
        color,
        egui::Stroke::NONE,
    ));
}

/// Five-point star: filled (marked) or outline.
fn icon_star(p: &egui::Painter, r: egui::Rect, color: egui::Color32, filled: bool) {
    let c = r.center();
    let ro = r.width() * 0.5;
    let ri = ro * 0.42;
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let rad = if i % 2 == 0 { ro } else { ri };
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            c + egui::vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    if filled {
        p.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    } else {
        let mut closed = pts;
        closed.push(closed[0]);
        p.add(egui::Shape::line(closed, egui::Stroke::new(1.4, color)));
    }
}
