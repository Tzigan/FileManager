mod archive;
mod bookmark_cache;
mod chrome;
mod dialogs;
mod input;
mod navigation;
mod preview;
use preview::PreviewState;
mod search;
mod tab_drag_preview;
mod toolbar;
mod trash;
mod window_frame;

pub(crate) use chrome::{
    configure_tab_scroll_style, icon_arrow, icon_button, icon_pin, icon_refresh, icon_star,
    tab_close_button_hovered, tab_scroll_source, tab_strip_content_margin,
};

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError, channel, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use super::style;
use crate::core::fs::{self, FileEntry, ReadDirError, SortKey};
use crate::core::i18n::{I18n, Language};
use crate::core::known_folders::SystemBookmark;
use crate::core::ops::{self, OpEvent, OpJob, OpReply};
use crate::core::paths::AppDirs;
use crate::core::preview::TextPreview;
use crate::core::search::SearchValidationError;
use crate::core::settings::{SettingsStore, Theme, ViewMode};
use crate::core::update::UpdateManifest;
use crate::platform::{
    DiskUsage, drives, install_context_menu, pipe_listen, uninstall_context_menu,
};
use crate::ui::opsui::OpsUi;
use crate::ui::tabs::{ListStatus, Tab};
use crate::ui::tree::{
    TREE_PANEL_MAX_WIDTH, TREE_PANEL_MIN_WIDTH, TreeState, clamp_tree_panel_width,
};
use bookmark_cache::{BookmarkProbeBatch, BookmarkStateCache};

const TREE_PANEL_PERSIST_DEBOUNCE: Duration = Duration::from_millis(300);
const TREE_PANEL_WIDTH_CHANGE_EPSILON: f32 = 0.5;

const DISK_USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// Канал эскизов остаётся отдельным от пула тяжёлых независимых операций.
pub(crate) const THUMBNAIL_QUEUE_CAPACITY: usize = 24;
const SHUTDOWN_NOTICE_DELAY: Duration = Duration::from_secs(10);
/// Ноль зарезервирован за автопроверкой при запуске.
const STARTUP_UPDATE_CHECK_ID: u64 = 0;
/// Завершает ожидание интерфейса, не прерывая сетевой поток.
const MANUAL_UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Отбрасывает поздний ответ, если проверка About уже закрыта или заменена новой.
#[derive(Default)]
struct UpdateCheckState {
    generation: u64,
    busy: bool,
    deadline: Option<(u64, Instant)>,
    timed_out_request_id: Option<u64>,
}

impl UpdateCheckState {
    fn begin_manual(&mut self, in_flight: &AtomicBool, now: Instant) -> Option<u64> {
        if self.busy
            || in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return None;
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        self.busy = true;
        self.deadline = Some((self.generation, now + MANUAL_UPDATE_CHECK_TIMEOUT));
        self.timed_out_request_id = None;
        Some(self.generation)
    }

    fn accept_result(&mut self, request_id: u64, about_open: bool) -> bool {
        // Автопроверка общая для приложения, ручной ответ допустим только в своём окне.
        if request_id == STARTUP_UPDATE_CHECK_ID {
            return self.generation == STARTUP_UPDATE_CHECK_ID;
        }
        if self.generation != request_id || !self.busy || !about_open {
            return false;
        }
        self.busy = false;
        self.deadline = None;
        true
    }

    fn expire(&mut self, now: Instant) -> Option<(u64, bool)> {
        let (request_id, deadline) = self.deadline?;
        if now < deadline {
            return None;
        }
        self.deadline = None;
        self.timed_out_request_id = Some(request_id);
        let was_current = self.busy && self.generation == request_id;
        if was_current {
            self.generation = self.generation.wrapping_add(1).max(1);
            self.busy = false;
        }
        Some((request_id, was_current))
    }

    fn worker_finished(&mut self, request_id: u64) {
        if self
            .deadline
            .is_some_and(|(active_id, _)| active_id == request_id)
        {
            self.deadline = None;
        }
        if self.timed_out_request_id == Some(request_id) {
            self.timed_out_request_id = None;
        }
    }

    fn close_about(&mut self) {
        // Ответ, пришедший после закрытия, не должен менять состояние следующего окна.
        // Без ручной проверки поколение 0 остаётся актуальным для автопроверки приложения.
        if self.generation != STARTUP_UPDATE_CHECK_ID {
            self.generation = self.generation.wrapping_add(1).max(1);
        }
        self.busy = false;
    }
}

// Нулевой ID зарезервирован только за автопроверкой при запуске.
fn is_startup_update_check(request_id: u64) -> bool {
    request_id == STARTUP_UPDATE_CHECK_ID
}

/// Освобождает слот после фактического завершения ручной проверки.
struct UpdateCheckInFlightGuard(Arc<AtomicBool>);

impl Drop for UpdateCheckInFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Results produced by background worker threads.
pub(crate) enum WorkerMsg {
    /// A directory listing finished for a tab.
    Listing {
        tab_id: u64,
        generation: u64,
        result: Result<Vec<FileEntry>, ReadDirError>,
        /// Ключ кэша и номер чтения. `None` — ZIP, WSL или путь без канонизации.
        cache_key: Option<crate::core::dir_cache::CacheKey>,
        cache_generation: u64,
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
        destination: PathBuf,
        result: std::io::Result<PathBuf>,
    },
    /// Завершилось создание ZIP-архива.
    ZipCreated {
        generation: u64,
        destination: PathBuf,
        result: std::io::Result<PathBuf>,
    },
    /// A folder tree node's children finished loading.
    TreeChildren {
        path: PathBuf,
        result: std::io::Result<Vec<PathBuf>>,
    },
    /// Обновились сведения о занятости томов.
    DiskUsage { values: HashMap<PathBuf, DiskUsage> },
    /// Обнаружены установленные дистрибутивы WSL.
    WslDistributions(Vec<String>),
    /// A path forwarded by another instance / CLI — open in a new tab.
    OpenPath(PathBuf),
    /// Найдена более новая версия приложения.
    UpdateFound {
        request_id: u64,
        manifest: UpdateManifest,
    },
    /// Получено обновление прогресса скачивания.
    UpdateProgress { downloaded: u64, total: u64 },
    /// Проверенный пакет подготовлен для установки отдельным updater.
    UpdateReady(PathBuf),
    /// При проверке вручную установлена последняя версия.
    UpdateLatest { request_id: u64 },
    /// Проверка или загрузка обновления завершилась ошибкой.
    UpdateFailed {
        /// `None` используется для ошибки загрузки после завершения проверки.
        check_id: Option<u64>,
        error: String,
    },
    /// Фоновое помещение в защищённую корзину закончилось.
    /// `paths` — исходные файлы: их папки надо сбросить в кэше, сама корзина кэшем списка не обслуживается.
    SecureTrashFinished {
        errors: Vec<(PathBuf, crate::core::secure_trash::TrashError)>,
        paths: Vec<PathBuf>,
    },
    /// Завершился отдельный локальный экспорт логов.
    LogExportFinished {
        result: Result<
            (crate::core::log_export::ExportedLog, bool),
            crate::core::log_export::LogExportError,
        >,
    },
    /// Тяжёлые свойства одного объекта. Поколение отбрасывает ответ для уже другого выбора.
    EntryDetails {
        generation: u64,
        path: PathBuf,
        details: crate::core::entry_details::EntryDetails,
    },
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

/// Отложенный reload привязан к пути, вкладке и поколению watcher-а.
struct PendingDirWatchReload {
    token: crate::core::dir_watch::WatchReloadToken,
    deadline: Instant,
}

#[derive(Clone, Copy)]
enum BgOpUiKind {
    Search { tab_id: u64, generation: u64 },
    Zip { generation: u64 },
}

struct BgOpUiTask {
    kind: BgOpUiKind,
    queued: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShutdownOperationKind {
    File(&'static str),
    Search,
    Zip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShutdownOperation {
    id: u64,
    kind: ShutdownOperationKind,
    queued: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ShutdownSnapshot {
    operations: Vec<ShutdownOperation>,
}

impl ShutdownSnapshot {
    fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    fn summary(&self, i18n: &I18n) -> String {
        let mut file_operations = BTreeMap::<&'static str, usize>::new();
        let mut searches = 0;
        let mut zip_operations = 0;
        for operation in &self.operations {
            match operation.kind {
                ShutdownOperationKind::File(verb) => {
                    *file_operations.entry(verb).or_default() += 1;
                }
                ShutdownOperationKind::Search => searches += 1,
                ShutdownOperationKind::Zip => zip_operations += 1,
            }
        }

        let mut details = Vec::new();
        for (verb, count) in file_operations {
            details.push(format!("{}: {count}", i18n.t(verb)));
        }
        if searches > 0 {
            details.push(format!("{}: {searches}", i18n.t("shutdown.searches")));
        }
        if zip_operations > 0 {
            details.push(format!(
                "{}: {zip_operations}",
                i18n.t("shutdown.zip_operations")
            ));
        }
        if details.is_empty() {
            i18n.t("shutdown.finishing").to_owned()
        } else {
            details.join(", ")
        }
    }
}

#[derive(Default)]
struct ShutdownProgress {
    started_at: Option<Instant>,
    notice_started_at: Option<Instant>,
    initial_snapshot: ShutdownSnapshot,
    warning_shown: bool,
    awaiting_user_response: bool,
    close_allowed: bool,
}

impl ShutdownProgress {
    fn begin(&mut self, now: Instant, snapshot: ShutdownSnapshot) -> bool {
        if self.started_at.is_some() || snapshot.is_empty() {
            return false;
        }
        self.started_at = Some(now);
        self.notice_started_at = Some(now);
        self.initial_snapshot = snapshot;
        true
    }

    fn is_active(&self) -> bool {
        self.started_at.is_some()
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.started_at
            .map(|started_at| now.saturating_duration_since(started_at))
            .unwrap_or_default()
    }

    fn take_warning(&mut self, now: Instant) -> bool {
        if self.is_active()
            && !self.awaiting_user_response
            && !self.warning_shown
            && self
                .notice_started_at
                .is_some_and(|started_at| now.saturating_duration_since(started_at) >= SHUTDOWN_NOTICE_DELAY)
        {
            self.warning_shown = true;
            return true;
        }
        false
    }

    // Ожидание ответа пользователя нельзя выдавать за задержку файловой системы.
    fn set_awaiting_user_response(&mut self, awaiting: bool, now: Instant) {
        if self.awaiting_user_response != awaiting {
            self.awaiting_user_response = awaiting;
            self.notice_started_at = Some(now);
            self.warning_shown = false;
        }
    }

    fn allow_close(&mut self) -> bool {
        if self.close_allowed {
            return false;
        }
        self.close_allowed = true;
        true
    }

    fn workers_finished(&self, pending: &ShutdownSnapshot, file_worker_finished: bool) -> bool {
        pending.is_empty() && file_worker_finished
    }
}

fn log_shutdown_start(snapshot: &ShutdownSnapshot) {
    tracing::info!(
        pending_operations = snapshot.operations.len(),
        "начато штатное ожидание фоновых операций"
    );
    for operation in &snapshot.operations {
        match operation.kind {
            ShutdownOperationKind::File(verb) => tracing::info!(
                operation_id = operation.id,
                operation_type = "file",
                action = verb,
                queued = ?operation.queued,
                "ожидает завершения файловая операция"
            ),
            ShutdownOperationKind::Search => tracing::info!(
                operation_id = operation.id,
                operation_type = "search",
                queued = ?operation.queued,
                "ожидает завершения поиск"
            ),
            ShutdownOperationKind::Zip => tracing::info!(
                operation_id = operation.id,
                operation_type = "zip",
                queued = ?operation.queued,
                "ожидает завершения операция ZIP"
            ),
        }
    }
}

fn log_shutdown_wait(elapsed: Duration, snapshot: &ShutdownSnapshot) {
    tracing::warn!(
        elapsed_ms = elapsed.as_millis(),
        pending_operations = snapshot.operations.len(),
        "порог ожидания shutdown превышен; приложение продолжает ждать worker-ы"
    );
    for operation in &snapshot.operations {
        match operation.kind {
            ShutdownOperationKind::File(verb) => tracing::warn!(
                operation_id = operation.id,
                operation_type = "file",
                action = verb,
                queued = ?operation.queued,
                "файловая операция всё ещё ожидает завершения"
            ),
            ShutdownOperationKind::Search => tracing::warn!(
                operation_id = operation.id,
                operation_type = "search",
                queued = ?operation.queued,
                "поиск всё ещё ожидает завершения"
            ),
            ShutdownOperationKind::Zip => tracing::warn!(
                operation_id = operation.id,
                operation_type = "zip",
                queued = ?operation.queued,
                "операция ZIP всё ещё ожидает завершения"
            ),
        }
    }
}

pub struct FileManagerApp {
    pub(crate) store: SettingsStore,
    pub(crate) i18n: I18n,
    dirs: AppDirs,
    show_about: bool,
    about_viewport_placed: bool,
    show_hotkeys: bool,
    show_settings: bool,
    settings_viewport_placed: bool,
    /// Активная категория окна настроек. На сохранение значений не влияет.
    settings_category: dialogs::SettingsCategory,
    settings_reset_confirm: bool,
    settings_reset_status: Option<String>,
    pub(crate) show_secure_trash: bool,
    trash_selected: HashSet<String>,
    trash_conflict: Option<String>,
    pub(crate) trash_purge_ids: Option<Vec<String>>,
    pub(crate) trash_empty_confirm: bool,
    trash_notice: Option<String>,
    style_configured: bool,
    update_startup_signaled: bool,
    /// Update offer: (version, exe URL).
    pending_update: Option<UpdateManifest>,
    update_busy: bool,
    /// Однократная прокрутка окна «О программе» к началу загрузки обновления.
    about_scroll_to_update: bool,
    update_check: UpdateCheckState,
    update_check_inflight: Arc<AtomicBool>,
    update_downloaded: u64,
    update_total: u64,
    update_ready: Option<PathBuf>,
    update_status: Option<String>,
    /// Ошибка обновления, показываемая в окне «О программе».
    update_err: Option<String>,
    log_export_status: Option<String>,
    log_export_busy: bool,
    /// Когда отдать установку updater и закрыть корневое окно.
    update_restart_at: Option<Instant>,
    /// Автоперезапуск выключен: ждём кнопку, процесс не завершаем.
    update_restart_offer: bool,
    tab_drag_state: tab_drag_preview::TabDragState,
    tab_drag_preview: tab_drag_preview::TabDragPreviewState,
    /// ID новой активной вкладки, которую нужно показать в строке.
    tab_strip_scroll_target: Option<u64>,
    /// Path shown in the properties dialog.
    pub(crate) props_target: Option<PathBuf>,
    /// Снимок выбранных объектов для агрегата в том же окне свойств.
    pub(crate) props_multi_target: Option<Vec<crate::core::fs::FileEntry>>,
    /// Запрос тяжёлых свойств. Список папки его не заполняет.
    pub(crate) details_watch: crate::core::entry_details::DetailsWatch,
    props_viewport_placed: bool,
    preview: PreviewState,
    /// Tabs requested by widgets while the current frame is rendering.
    pending_open_tabs: Vec<PathBuf>,
    /// Drag state for reordering grouped table sections.
    pub(crate) group_drag: Option<(String, String)>,
    pub(crate) group_press: Option<(String, String, egui::Pos2)>,
    pub(crate) file_drop_feedback: Option<crate::ui::file_dnd::DropFeedback>,
    // Отдельный получатель отпускания не зависит от приоритета подсветки.
    pub(crate) file_drop_release: Option<crate::ui::file_dnd::DropFeedback>,
    pub(crate) file_drag_candidate: Option<(egui::Id, Instant)>,
    thumbnail_tx: SyncSender<ThumbnailRequest>,
    thumbnail_jobs: HashMap<ThumbnailKey, Arc<AtomicBool>>,
    thumbnail_visible: HashSet<ThumbnailKey>,
    thumbnail_cache: HashMap<ThumbnailKey, egui::TextureHandle>,
    /// Лёгкие списки уже открытых папок. Эскизы и дерево сюда не входят.
    pub(crate) dir_cache: crate::core::dir_cache::DirCache,
    dir_watch_tx: Sender<crate::core::dir_watch::DirWatchCommand>,
    dir_watch_rx: Receiver<crate::core::dir_watch::WatchNote>,
    dir_watch_target: Option<PathBuf>,
    dir_watch_tab_id: Option<u64>,
    dir_watch_generation: u64,
    dir_watch_active_tab_id: Option<u64>,
    dir_watch_debounce: crate::core::dir_watch::WatchDebounce,
    dir_watch_deadline: Option<PendingDirWatchReload>,
    dir_watch_manual: bool,
    /// Только поиск и ZIP; чтение списков, DetailsWatch, watcher и invalidate_dir вне пула.
    bg_ops_pool: crate::core::bg_ops::BgOpPool,
    bg_ops_events_rx: Receiver<crate::core::bg_ops::BgOpEvent>,
    bg_op_ui: HashMap<u64, BgOpUiTask>,
    shutdown_bg_op_ids: HashSet<u64>,
    thumbnail_order: VecDeque<ThumbnailKey>,
    thumbnail_failed: HashSet<ThumbnailKey>,
    zip_generation: u64,
    zip_busy: bool,
    zip_creating: bool,
    zip_task_id: Option<u64>,
    zip_queued: bool,
    zip_cancel_requested: bool,
    zip_message: Option<String>,
    zip_create_dialog: Option<archive::ZipCreateDialog>,
    disk_usage_tx: Sender<Vec<PathBuf>>,
    disk_usage_loading: bool,
    disk_usage_refresh_at: Instant,
    wsl_req: Sender<()>,
    pub(crate) wsl_loading: bool,
    pub(crate) wsl_distributions: Vec<String>,
    detached_tabs: HashMap<u64, window_frame::DetachedTabWindow>,
    tab_detach_error: Option<String>,

    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) tree: TreeState,
    tree_panel_persist_deadline: Option<Instant>,
    pub(crate) system_bookmarks: Vec<SystemBookmark>,
    bookmark_state_cache: BookmarkStateCache,
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
    pub(crate) ops_tx: Option<Sender<OpJob>>,
    pub(crate) ops_evt: Receiver<OpEvent>,
    pub(crate) op_reply: Option<Sender<OpReply>>,
    ops_worker: Option<std::thread::JoinHandle<()>>,
    shutdown: ShutdownProgress,
    shutdown_prepared: bool,
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
        let settings_load_issue = store.load_issue;
        let (msg_tx, msg_rx) = channel();
        let (bg_ops_events_tx, bg_ops_events_rx) = channel();
        let repaint_ctx = cc.egui_ctx.clone();
        let bg_ops_pool = crate::core::bg_ops::BgOpPool::new(
            store.settings.max_concurrent_bg_ops,
            bg_ops_events_tx,
            move || repaint_ctx.request_repaint(),
        );
        let (dir_watch_notes_tx, dir_watch_rx) = channel();
        let dir_watch_tx = crate::platform::spawn_dir_watch(dir_watch_notes_tx);
        let (tree_req, tree_req_rx) = channel::<PathBuf>();
        let (disk_usage_tx, disk_usage_rx) = channel::<Vec<PathBuf>>();
        let (wsl_req, wsl_req_rx) = channel::<()>();
        let (ops_tx, jobs_rx) = channel::<OpJob>();
        let (op_reply, reply_rx) = channel::<OpReply>();
        let (events_tx, ops_evt) = channel::<OpEvent>();
        // Файловые операции остаются одним FIFO-worker ради зависимых перемещений.
        let ops_worker = ops::spawn(
            jobs_rx,
            reply_rx,
            events_tx,
            crate::platform::ops_platform(),
        );
        let (thumbnail_tx, thumbnail_rx) =
            sync_channel::<ThumbnailRequest>(THUMBNAIL_QUEUE_CAPACITY);
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

        {
            let msg_tx = msg_tx.clone();
            let ctx = cc.egui_ctx.clone();
            // Системные запросы к томам не выполняются во время отрисовки дерева.
            thread::spawn(move || {
                while let Ok(roots) = disk_usage_rx.recv() {
                    let values = roots
                        .into_iter()
                        .filter_map(|root| {
                            crate::platform::disk_usage(&root).map(|usage| (root, usage))
                        })
                        .collect();
                    if msg_tx.send(WorkerMsg::DiskUsage { values }).is_err() {
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
                            request_id: STARTUP_UPDATE_CHECK_ID,
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
                    let result = if crate::platform::is_wsl_path(&path) {
                        crate::platform::read_wsl_subdirs(&path)
                    } else {
                        fs::read_subdirs(&path)
                    };
                    if msg_tx
                        .send(WorkerMsg::TreeChildren { path, result })
                        .is_err()
                    {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
        {
            let msg_tx = msg_tx.clone();
            let ctx = cc.egui_ctx.clone();
            thread::spawn(move || {
                while wsl_req_rx.recv().is_ok() {
                    if msg_tx
                        .send(WorkerMsg::WslDistributions(
                            crate::platform::wsl_distributions(),
                        ))
                        .is_err()
                    {
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
        let drive_roots = drives();
        let wsl_insert_after = drive_roots
            .iter()
            .rposition(|path| crate::platform::is_local_drive(path))
            .map_or(drive_roots.len(), |index| index + 1);
        let mut tree = TreeState::new(drive_roots);
        tree.wsl_insert_after = wsl_insert_after;
        let dir_cache_entries = store.settings.dir_cache_entries;
        let mut app = Self {
            store,
            i18n,
            dirs,
            show_about: false,
            about_viewport_placed: false,
            show_hotkeys: false,
            show_settings: false,
            settings_viewport_placed: false,
            settings_category: dialogs::SettingsCategory::default(),
            settings_reset_confirm: false,
            settings_reset_status: None,
            show_secure_trash: false,
            trash_selected: HashSet::new(),
            trash_conflict: None,
            trash_purge_ids: None,
            trash_empty_confirm: false,
            trash_notice: None,
            style_configured: false,
            update_startup_signaled: false,
            pending_update: None,
            update_busy: false,
            about_scroll_to_update: false,
            update_check: UpdateCheckState::default(),
            update_check_inflight: Arc::new(AtomicBool::new(false)),
            update_downloaded: 0,
            update_total: 0,
            update_ready: None,
            update_status: None,
            update_err: None,
            log_export_status: None,
            log_export_busy: false,
            update_restart_at: None,
            update_restart_offer: false,
            tab_drag_state: tab_drag_preview::TabDragState::Idle,
            tab_drag_preview: tab_drag_preview::TabDragPreviewState::default(),
            tab_strip_scroll_target: None,
            props_target: None,
            props_multi_target: None,
            details_watch: crate::core::entry_details::DetailsWatch::default(),
            props_viewport_placed: false,
            preview: PreviewState::default(),
            pending_open_tabs: Vec::new(),
            group_drag: None,
            group_press: None,
            file_drop_feedback: None,
            file_drop_release: None,
            file_drag_candidate: None,
            thumbnail_tx,
            thumbnail_jobs: HashMap::new(),
            thumbnail_visible: HashSet::new(),
            thumbnail_cache: HashMap::new(),
            dir_cache: crate::core::dir_cache::DirCache::new(dir_cache_entries),
            dir_watch_tx,
            dir_watch_rx,
            dir_watch_target: None,
            dir_watch_tab_id: None,
            dir_watch_generation: 0,
            dir_watch_active_tab_id: None,
            dir_watch_debounce: crate::core::dir_watch::WatchDebounce::default(),
            dir_watch_deadline: None,
            dir_watch_manual: false,
            bg_ops_pool,
            bg_ops_events_rx,
            bg_op_ui: HashMap::new(),
            shutdown_bg_op_ids: HashSet::new(),
            thumbnail_order: VecDeque::new(),
            thumbnail_failed: HashSet::new(),
            zip_generation: 0,
            zip_busy: false,
            zip_creating: false,
            zip_task_id: None,
            zip_queued: false,
            zip_cancel_requested: false,
            zip_message: None,
            zip_create_dialog: None,
            disk_usage_tx,
            disk_usage_loading: false,
            disk_usage_refresh_at: Instant::now(),
            wsl_req,
            wsl_loading: false,
            wsl_distributions: Vec::new(),
            detached_tabs: HashMap::new(),
            tab_detach_error: None,
            tabs: paths
                .into_iter()
                .map(|p| {
                    let mut t = Tab::new(p);
                    t.pinned = pins.contains(&t.path);
                    t
                })
                .collect(),
            active,
            tree,
            tree_panel_persist_deadline: None,
            system_bookmarks: crate::core::known_folders::system_bookmarks(),
            bookmark_state_cache: BookmarkStateCache::new(),
            scroll_to_cursor: false,
            pending_select: None,
            msg_tx,
            msg_rx,
            tree_req,
            ops_ui: OpsUi::default(),
            ops_tx: Some(ops_tx),
            ops_evt,
            op_reply: Some(op_reply),
            ops_worker: Some(ops_worker),
            shutdown: ShutdownProgress::default(),
            shutdown_prepared: false,
        };
        if let Some(issue) = settings_load_issue {
            app.ops_ui.journal.push(app.t(issue.message_key()).to_owned());
            app.ops_ui.show_journal = true;
        }
        // CLI path (Explorer "Open in FileManager") wins over restored tabs.
        if let Some(p) = initial_path {
            app.tabs.push(Tab::new(p));
            app.active = app.tabs.len() - 1;
            app.request_tab_strip_scroll_to_active(&cc.egui_ctx);
        }
        if let Some(name) = initial_select {
            app.pending_select = Some((app.tabs[app.active].id, name));
        }
        for i in 0..app.tabs.len() {
            app.navigate_fresh(i, &cc.egui_ctx);
        }
        if app.store.settings.show_wsl_in_tree {
            app.refresh_wsl_distributions();
        }
        let tab = &app.tabs[app.active];
        let reveal_path = if tab.archive_prefix.is_some() {
            tab.path.parent().unwrap_or(&tab.path)
        } else {
            &tab.path
        };
        app.tree.reveal(reveal_path);
        app.pump_bookmark_state_cache(&cc.egui_ctx);
        app
    }

    pub(crate) fn bookmark_drop_state(
        &self,
        path: &Path,
    ) -> crate::ui::file_dnd::BookmarkPathState {
        self.bookmark_state_cache.drop_state(path)
    }

    fn pump_bookmark_state_cache(&mut self, ctx: &egui::Context) {
        self.bookmark_state_cache.drain_results();
        if self.shutdown.is_active() {
            return;
        }

        let paths = self
            .system_bookmarks
            .iter()
            .filter_map(|bookmark| match &bookmark.target {
                crate::core::known_folders::SystemBookmarkTarget::Folder(path) => {
                    Some(path.clone())
                }
                _ => None,
            })
            .chain(self.store.settings.bookmarks.iter().cloned());
        self.bookmark_state_cache.sync_paths(paths);
        let now = Instant::now();
        self.bookmark_state_cache
            .queue_expired(now, bookmark_cache::BOOKMARK_STATE_TTL);

        if let Some(paths) = self.bookmark_state_cache.take_pending() {
            let sender = self.bookmark_state_cache.sender();
            let ctx = ctx.clone();
            // Проверка только читает metadata. Поток намеренно отсоединён: поздний ответ лишь
            // отправляется в канал и не меняет состояние приложения или файлы, поэтому shutdown
            // не ждёт его завершения, в отличие от файлового worker-а.
            thread::spawn(move || {
                let results = paths
                    .into_iter()
                    .map(|path| {
                        let is_dir = std::fs::metadata(&path)
                            .ok()
                            .map(|metadata| metadata.is_dir());
                        (path, is_dir)
                    })
                    .collect();
                let batch = BookmarkProbeBatch {
                    results,
                    checked_at: Instant::now(),
                };
                tracing::debug!(
                    paths = batch.results.len(),
                    "проверка путей закладок завершена"
                );
                let _ = bookmark_cache::send_probe_result(&sender, batch);
                ctx.request_repaint();
            });
        }

        if self.bookmark_state_cache.is_in_flight() {
            ctx.request_repaint_after(bookmark_cache::BOOKMARK_PROBE_DRAIN_INTERVAL);
        } else if let Some(delay) = self
            .bookmark_state_cache
            .until_next_expiration(now, bookmark_cache::BOOKMARK_STATE_TTL)
        {
            ctx.request_repaint_after(delay);
        }
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
        crate::core::logging::set_panic_language(lang);
        self.persist();
    }

    pub(crate) fn persist(&mut self) {
        let _ = self.persist_checked();
    }

    pub(crate) fn persist_checked(&mut self) -> anyhow::Result<()> {
        match self.store.save() {
            Ok(()) => Ok(()),
            Err(error) => {
                let path = self.dirs.config_dir.join("settings.json");
                let error_kind = error
                    .downcast_ref::<std::io::Error>()
                    .map(std::io::Error::kind);
                tracing::info!(
                    path = %path.display(),
                    error_kind = ?error_kind,
                    error = %format!("{error:#}"),
                    "settings save failed"
                );
                self.ops_ui
                    .journal
                    .push(self.t("settings.save_failed").to_owned());
                self.ops_ui.show_journal = true;
                Err(error)
            }
        }
    }

    fn note_tree_panel_width(&mut self, ctx: &egui::Context, width: f32) {
        let width = clamp_tree_panel_width(width);
        if (self.store.settings.tree_panel_width - width).abs()
            < TREE_PANEL_WIDTH_CHANGE_EPSILON
        {
            return;
        }
        self.store.settings.tree_panel_width = width;
        self.tree_panel_persist_deadline = Some(Instant::now() + TREE_PANEL_PERSIST_DEBOUNCE);
        ctx.request_repaint_after(TREE_PANEL_PERSIST_DEBOUNCE);
    }

    fn persist_tree_panel_width_if_due(&mut self, ctx: &egui::Context) {
        let Some(deadline) = self.tree_panel_persist_deadline else {
            return;
        };
        let now = Instant::now();
        if deadline <= now {
            self.tree_panel_persist_deadline = None;
            let _ = self.persist_checked();
        } else {
            ctx.request_repaint_after(deadline - now);
        }
    }

    pub(crate) fn sort_col(&mut self, key: SortKey) {
        let tab = &mut self.tabs[self.active];
        let cursor_key = tab
            .entries
            .get(tab.cursor)
            .map(crate::ui::tabs::SelectionKey::from_entry);
        (tab.sort_key, tab.sort_desc) =
            super::list::next_sort_order(tab.sort_key, tab.sort_desc, key);
        let (k, d) = (tab.sort_key, tab.sort_desc);
        fs::sort_entries_by(&mut tab.entries, k, d);
        if let Some(cursor_key) = cursor_key {
            tab.cursor = tab
                .entries
                .iter()
                .position(|entry| cursor_key.matches(entry))
                .unwrap_or(0);
        }
    }

    pub(crate) fn set_view(&mut self, mode: ViewMode) {
        self.store.settings.view_mode = mode;
        self.persist();
    }

    pub(crate) fn refresh_wsl_distributions(&mut self) {
        if !self.store.settings.show_wsl_in_tree || self.wsl_loading {
            return;
        }
        self.wsl_loading = true;
        self.tree
            .children
            .retain(|path, _| !crate::platform::is_wsl_path(path));
        self.tree
            .failed_children
            .retain(|path| !crate::platform::is_wsl_path(path));
        if self.wsl_req.send(()).is_err() {
            self.wsl_loading = false;
        }
    }

    // ------------------------------------------------------------------
    // Messages and input
    // ------------------------------------------------------------------

    pub(super) fn cancel_search_bg_ops(&mut self, tab_id: u64) {
        let ids: Vec<_> = self
            .bg_op_ui
            .iter()
            .filter_map(|(&id, task)| {
                match task.kind {
                    BgOpUiKind::Search {
                        tab_id: owner,
                        generation,
                    } if owner == tab_id => Some((id, generation)),
                    _ => None,
                }
            })
            .collect();
        for (id, generation) in ids {
            if self.bg_ops_pool.cancel(id) == crate::core::bg_ops::BgOpCancelOutcome::Running {
                // Поиск уже не показывается, но shutdown должен дождаться его worker-а.
                self.shutdown_bg_op_ids.insert(id);
            }
            self.bg_op_ui.remove(&id);
            if let Some(tab) = self
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id && tab.search.generation == generation)
            {
                tab.search.running = false;
            }
        }
    }

    fn pending_shutdown_snapshot(&self) -> ShutdownSnapshot {
        let mut operations: Vec<_> = self
            .ops_ui
            .jobs
            .iter()
            .map(|job| ShutdownOperation {
                id: job.id,
                kind: ShutdownOperationKind::File(job.verb),
                queued: None,
            })
            .collect();
        let mut pool_ids = HashSet::new();
        for (&id, task) in &self.bg_op_ui {
            pool_ids.insert(id);
            operations.push(ShutdownOperation {
                id,
                kind: match task.kind {
                    BgOpUiKind::Search { .. } => ShutdownOperationKind::Search,
                    BgOpUiKind::Zip { .. } => ShutdownOperationKind::Zip,
                },
                queued: Some(task.queued),
            });
        }
        for &id in &self.shutdown_bg_op_ids {
            if pool_ids.insert(id) {
                operations.push(ShutdownOperation {
                    id,
                    kind: ShutdownOperationKind::Search,
                    queued: Some(false),
                });
            }
        }
        operations.sort_by_key(|operation| operation.id);
        ShutdownSnapshot { operations }
    }

    fn start_shutdown(&mut self) {
        let snapshot = self.pending_shutdown_snapshot();
        if !self.shutdown.begin(Instant::now(), snapshot.clone()) {
            return;
        }
        log_shutdown_start(&snapshot);
        self.prepare_shutdown();
    }

    pub(crate) fn shutdown_active(&self) -> bool {
        self.shutdown.is_active()
    }

    fn prepare_shutdown(&mut self) {
        if self.shutdown_prepared {
            return;
        }
        self.shutdown_prepared = true;
        self.bg_ops_pool.request_shutdown();

        let queued_pool_ids: Vec<_> = self
            .bg_op_ui
            .iter()
            .filter_map(|(&id, task)| task.queued.then_some(id))
            .collect();
        for id in queued_pool_ids {
            if self.bg_ops_pool.cancel(id) != crate::core::bg_ops::BgOpCancelOutcome::Running {
                self.bg_op_ui.remove(&id);
                tracing::info!(
                    operation_id = id,
                    "ожидающая фоновая операция снята штатной отменой shutdown"
                );
            }
        }

        // Навигация остаётся вне пула, но получает штатные флаги отмены при закрытии окна.
        for tab in &self.tabs {
            tab.cancel.store(true, Ordering::Relaxed);
            tab.search.cancel.store(true, Ordering::Relaxed);
        }
        let _ = self
            .dir_watch_tx
            .send(crate::core::dir_watch::DirWatchCommand::Stop);
        // Сначала отменяем текущую и ожидающие операции, не нарушая их FIFO-порядок.
        for job in &self.ops_ui.jobs {
            job.cancel.store(true, Ordering::Relaxed);
        }
        // Пока worker ждёт ответа, UI должен оставить канал открытым и показать вопрос.
        self.ops_tx.take();
    }

    fn drain_shutdown_ops(&mut self, ctx: &egui::Context) {
        self.drain_ops_events(ctx);
    }

    fn shutdown_frame(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        close_requested: bool,
    ) {
        self.drain_bg_op_events();
        self.drain_shutdown_ops(ctx);

        let now = Instant::now();
        let awaiting_user_response =
            self.ops_ui.conflict.is_some() || self.ops_ui.error_q.is_some();
        self.shutdown
            .set_awaiting_user_response(awaiting_user_response, now);
        let pending = self.pending_shutdown_snapshot();
        let elapsed = self.shutdown.elapsed(now);
        let file_worker_finished = self
            .ops_worker
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished);
        let workers_finished = self
            .shutdown
            .workers_finished(&pending, file_worker_finished);
        if !workers_finished && self.shutdown.take_warning(now) {
            log_shutdown_wait(elapsed, &pending);
        }
        let details = pending.summary(&self.i18n);
        let warning_shown = self.shutdown.warning_shown;

        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(32.0);
                ui.heading(self.t("shutdown.waiting"));
                ui.add_space(12.0);
                ui.label(format!("{}: {details}", self.t("shutdown.pending")));
                if awaiting_user_response {
                    ui.add_space(8.0);
                    ui.label(self.t("shutdown.awaiting_response"));
                }
                if warning_shown {
                    ui.add_space(16.0);
                    ui.separator();
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        self.t("shutdown.fs_unresponsive"),
                    );
                    ui.label(self.t("shutdown.continue_waiting"));
                    ui.label(self.t("shutdown.force_close_risk"));
                }
            });
        });
        self.ops_dialogs(ctx);

        if workers_finished && self.shutdown.allow_close() {
            tracing::info!(
                elapsed_ms = elapsed.as_millis(),
                initial_operations = self.shutdown.initial_snapshot.operations.len(),
                "операции завершились; штатные join продолжают безопасное закрытие"
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if close_requested && !self.shutdown.close_allowed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        ctx.request_repaint_after(Duration::from_millis(150));
    }

    fn search_bg_op_queued(&self, tab_id: u64, generation: u64) -> bool {
        self.bg_op_ui.values().any(|task| {
            task.queued
                && matches!(
                    task.kind,
                    BgOpUiKind::Search {
                        tab_id: owner,
                        generation: task_generation
                    } if owner == tab_id && task_generation == generation
                )
        })
    }

    fn drain_bg_op_events(&mut self) {
        self.bg_ops_pool.prune_cancelled();
        while let Ok(event) = self.bg_ops_events_rx.try_recv() {
            match event {
                crate::core::bg_ops::BgOpEvent::Started { id, kind } => {
                    let Some(task) = self.bg_op_ui.get_mut(&id) else {
                        continue;
                    };
                    task.queued = false;
                    match (task.kind, kind) {
                        (
                            BgOpUiKind::Search { tab_id, generation },
                            crate::core::bg_ops::BgOpKind::Search,
                        ) => {
                            if let Some(tab) = self.tabs.iter_mut().find(|tab| {
                                tab.id == tab_id && tab.search.generation == generation
                            }) {
                                tab.search.running = true;
                            } else {
                                let _ = self.bg_ops_pool.cancel(id);
                            }
                        }
                        (
                            BgOpUiKind::Zip { generation },
                            crate::core::bg_ops::BgOpKind::ZipCreate
                            | crate::core::bg_ops::BgOpKind::ZipExtract,
                        ) if self.zip_generation == generation => {
                            self.zip_queued = false;
                        }
                        _ => {}
                    }
                }
                crate::core::bg_ops::BgOpEvent::Finished {
                    id,
                    kind,
                    panicked,
                } => {
                    let shutdown_tracked = self.shutdown_bg_op_ids.remove(&id);
                    if shutdown_tracked && self.shutdown.is_active() {
                        tracing::info!(
                            operation_id = id,
                            operation_type = ?kind,
                            "отменённый поиск завершился во время ожидания shutdown"
                        );
                    }
                    if self.shutdown.is_active() && !shutdown_tracked {
                        tracing::info!(
                            operation_id = id,
                            operation_type = ?kind,
                            panicked,
                            "фоновая операция завершилась при shutdown"
                        );
                    }
                    let Some(task) = self.bg_op_ui.remove(&id) else {
                        continue;
                    };
                    match (task.kind, kind) {
                        (
                            BgOpUiKind::Search { tab_id, generation },
                            crate::core::bg_ops::BgOpKind::Search,
                        ) => {
                            let failure_message =
                                panicked.then(|| self.t("background_ops.failed").to_owned());
                            if let Some(tab) = self.tabs.iter_mut().find(|tab| {
                                tab.id == tab_id && tab.search.generation == generation
                            }) {
                                tab.search.running = false;
                                if let Some(message) = failure_message {
                                    tab.search.results_active = false;
                                    tab.search.validation_error = Some(message);
                                    tab.status = ListStatus::Ready;
                                }
                            }
                        }
                        (
                            BgOpUiKind::Zip { generation },
                            crate::core::bg_ops::BgOpKind::ZipCreate
                            | crate::core::bg_ops::BgOpKind::ZipExtract,
                        ) if self.zip_generation == generation => {
                            self.zip_task_id = None;
                            self.zip_queued = false;
                            if panicked {
                                self.zip_busy = false;
                                self.zip_creating = false;
                                self.zip_cancel_requested = false;
                                self.zip_message =
                                    Some(self.t("background_ops.failed").to_owned());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                WorkerMsg::Listing {
                    tab_id,
                    generation,
                    result,
                    cache_key,
                    cache_generation,
                } => {
                    let Some(tab) = super::tabs::listing_tab_index(&self.tabs, tab_id, generation)
                    else {
                        continue;
                    };
                    match result {
                        Ok(entries) => {
                            if let Some(key) = cache_key {
                                // Поздний ответ другого чтения в кэш не пишется.
                                let _ = self.dir_cache.store(&key, cache_generation, entries.clone());
                            }
                            let pending_name = super::tabs::take_pending_selection(
                                &mut self.pending_select,
                                tab_id,
                            );
                            let show_hidden = self.store.settings.show_hidden_files;
                            let mut entries = fs::filter_hidden(entries, show_hidden);
                            let (sort_key, sort_desc) = {
                                let tab_state = &self.tabs[tab];
                                (tab_state.sort_key, tab_state.sort_desc)
                            };
                            fs::sort_entries_by(&mut entries, sort_key, sort_desc);
                            let t = &mut self.tabs[tab];
                            t.entries = entries;
                            super::tabs::retain_existing_selection(&mut t.selection, &t.entries);
                            let anchor_is_stale = t.anchor.as_ref().is_some_and(|anchor| {
                                !t.entries.iter().any(|entry| anchor.matches(entry))
                            });
                            if anchor_is_stale {
                                t.anchor = None;
                            }
                            t.cursor = t.cursor.min(t.entries.len().saturating_sub(1));
                            t.status = ListStatus::Ready;
                            if let Some(name) = pending_name {
                                if let Some(i) = t.entries.iter().position(|e| e.name == name) {
                                    let key =
                                        crate::ui::tabs::SelectionKey::from_entry(&t.entries[i]);
                                    t.selection.insert(key.clone());
                                    t.cursor = i;
                                    t.anchor = Some(key);
                                    self.scroll_to_cursor = true;
                                }
                            }
                        }
                        Err(ReadDirError::Cancelled) => {}
                        Err(ReadDirError::Io(e)) => {
                            let text = if self.tabs[tab].archive_prefix.is_some() {
                                self.zip_error_text(&e)
                            } else if crate::platform::is_wsl_path(&self.tabs[tab].path) {
                                self.t("tree.wsl.not_running").to_owned()
                            } else {
                                self.error_text(&e)
                            };
                            let t = &mut self.tabs[tab];
                            t.entries.clear();
                            t.clear_selection();
                            t.cursor = 0;
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
                    let Some(tab) = search::search_result_tab_index(&self.tabs, tab_id, generation)
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
                    if cancel.load(Ordering::Relaxed) || !self.thumbnail_visible.contains(&key) {
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
                    destination,
                    result,
                } => {
                    if generation != self.zip_generation {
                        continue;
                    }
                    self.zip_busy = false;
                    self.zip_creating = false;
                    self.zip_task_id = None;
                    self.zip_queued = false;
                    self.zip_cancel_requested = false;
                    // Ошибка могла произойти после публикации части выбранных элементов.
                    self.dir_cache.invalidate_dir(&destination);
                    self.zip_message = Some(match result {
                        Ok(path) => {
                            format!("{}: {}", self.t("zip.extracted"), path.display())
                        }
                        Err(error) => self.zip_error_text(&error),
                    });
                }
                WorkerMsg::EntryDetails {
                    generation,
                    path,
                    details,
                } => {
                    // Чужое поколение или путь молча отбрасываются.
                    let _ = self.details_watch.receive(generation, &path, details);
                }
                WorkerMsg::SecureTrashFinished { errors, paths } => {
                    self.trash_notice = if errors.is_empty() {
                        Some(self.t("trash.moved").to_owned())
                    } else {
                        Some(
                            errors
                                .iter()
                                .map(|(path, error)| {
                                    self.trash_error_line(&path.display().to_string(), error)
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                        )
                    };
                    for path in paths {
                        self.dir_cache.invalidate_dir(&path);
                    }
                    let active = self.active;
                    self.navigate_fresh(active, ctx);
                }
                WorkerMsg::ZipCreated {
                    generation,
                    destination,
                    result,
                } => {
                    if generation != self.zip_generation {
                        continue;
                    }
                    self.zip_busy = false;
                    self.zip_task_id = None;
                    self.zip_queued = false;
                    self.zip_cancel_requested = false;
                    if result.is_ok() {
                        // Новый архив появляется в папке назначения; сброс выполняется до перечитывания.
                        self.dir_cache.invalidate_dir(&destination);
                        if let Some(index) = self
                            .tabs
                            .iter()
                            .position(|tab| tab.path == destination && tab.archive_prefix.is_none())
                        {
                            self.navigate_fresh(index, ctx);
                        }
                    }
                    self.zip_message = Some(match result {
                        Ok(path) => {
                            format!("{}: {}", self.t("zip.created"), path.display())
                        }
                        Err(error) => self.zip_create_error_text(&error),
                    });
                }
                WorkerMsg::TreeChildren { path, result } => {
                    self.tree.complete_children_load(path, result);
                }
                WorkerMsg::DiskUsage { values } => {
                    self.tree.disk_usage = values;
                    self.disk_usage_loading = false;
                    self.disk_usage_refresh_at = Instant::now() + DISK_USAGE_REFRESH_INTERVAL;
                }
                WorkerMsg::WslDistributions(distributions) => {
                    self.wsl_distributions = distributions;
                    self.wsl_loading = false;
                    self.tree
                        .children
                        .retain(|path, _| !crate::platform::is_wsl_path(path));
                    self.tree
                        .failed_children
                        .retain(|path| !crate::platform::is_wsl_path(path));
                }
                WorkerMsg::OpenPath(path) => self.open_path_in_new_tab(path, ctx),
                WorkerMsg::LogExportFinished { result } => {
                    self.log_export_busy = false;
                    match result {
                        Ok((exported, freshness_confirmed)) => {
                            let mut status = format!(
                                "{}: {}",
                                self.t("logs.export_done"),
                                exported.path.display()
                            );
                            if !freshness_confirmed {
                                status.push_str("\n");
                                status.push_str(self.t("logs.export_pending_note"));
                            }
                            if !exported.skipped_files.is_empty() {
                                status.push_str("\n");
                                status.push_str(
                                    &self
                                        .t("logs.export_skipped_note")
                                        .replace("{count}", &exported.skipped_files.len().to_string()),
                                );
                            }
                            self.log_export_status = Some(status);
                        }
                        Err(crate::core::log_export::LogExportError::NoLogFiles) => {
                            self.log_export_status =
                                Some(self.t("logs.export_no_logs").to_owned());
                        }
                        Err(error) => {
                            let mut status = format!(
                                "{}: {}",
                                self.t("logs.export_failed"),
                                self.t(dialogs::log_export_error_key(&error))
                            );
                            if let crate::core::log_export::LogExportError::Io {
                                path,
                                source,
                                incomplete_path,
                            } = error
                            {
                                self.record_io_error(Some(&path), &source);
                                if let Some(incomplete_path) = incomplete_path {
                                    status.push_str("\n");
                                    status.push_str(
                                        &self
                                            .t("logs.export_temp_left")
                                            .replace("{path}", &incomplete_path.display().to_string()),
                                    );
                                }
                            }
                            self.log_export_status = Some(status);
                        }
                    }
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateFound {
                    request_id,
                    manifest,
                } => {
                    self.update_check.worker_finished(request_id);
                    if !self
                        .update_check
                        .accept_result(request_id, self.show_about)
                    {
                        continue;
                    }
                    self.update_busy = false;
                    self.update_total = manifest.size;
                    self.update_downloaded = 0;
                    self.update_ready = None;
                    self.pending_update = Some(manifest);
                    self.update_status = None;
                    self.update_err = None;
                    if is_startup_update_check(request_id) {
                        self.show_about = true;
                    }
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
                WorkerMsg::UpdateLatest { request_id } => {
                    self.update_check.worker_finished(request_id);
                    if !self
                        .update_check
                        .accept_result(request_id, self.show_about)
                    {
                        continue;
                    }
                    self.update_busy = false;
                    self.pending_update = None;
                    self.update_ready = None;
                    self.update_downloaded = 0;
                    self.update_total = 0;
                    self.update_err = None;
                    self.update_status = Some(self.t("update.latest").to_owned());
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateFailed { check_id, error } => {
                    if let Some(request_id) = check_id {
                        self.update_check.worker_finished(request_id);
                        if !self
                            .update_check
                            .accept_result(request_id, self.show_about)
                        {
                            continue;
                        }
                    }
                    self.update_busy = false;
                    tracing::warn!(error = %error, "update operation failed");
                    self.update_ready = None;
                    self.update_status = None;
                    self.update_err = Some(self.t("update.failed").to_owned());
                    ctx.request_repaint();
                }
            }
        }
    }

    // Запрашиваем объём томов редко и только в фоне, не во время отрисовки дерева.
    fn schedule_disk_usage_refresh(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if !self.disk_usage_loading && now >= self.disk_usage_refresh_at {
            self.disk_usage_loading = true;
            self.disk_usage_refresh_at = now + DISK_USAGE_REFRESH_INTERVAL;
            if self.disk_usage_tx.send(self.tree.roots.clone()).is_err() {
                self.disk_usage_loading = false;
            }
        }
        if !self.disk_usage_loading {
            let delay = self
                .disk_usage_refresh_at
                .saturating_duration_since(Instant::now());
            if !delay.is_zero() {
                ctx.request_repaint_after(delay);
            }
        }
    }

    pub(super) fn show_active_tab_status(&self, ui: &mut egui::Ui) {
        let tab = &self.tabs[self.active];
        let search_queued = self.search_bg_op_queued(tab.id, tab.search.generation);
        ui.horizontal(|ui| {
            if tab.status == ListStatus::Loading {
                let status = if search_queued {
                    "background_ops.queued"
                } else if tab.search.running || tab.search.debounce_at.is_some() {
                    "search.searching"
                } else {
                    "status.loading"
                };
                ui.label(self.t(status).to_owned());
            } else {
                ui.label(format!("{}: {}", self.t("status.items"), tab.entries.len()));
                if self.dir_watch_manual {
                    ui.separator();
                    ui.label(self.t("watch.unavailable").to_owned());
                }
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
        let (error, empty, loading, searching, queued, no_results) = {
            let tab = &self.tabs[self.active];
            (
                tab.error.clone(),
                tab.entries.is_empty(),
                tab.status == ListStatus::Loading,
                tab.search.running || tab.search.debounce_at.is_some(),
                self.search_bg_op_queued(tab.id, tab.search.generation),
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
        let bg_rect = crate::ui::list::list_background_rect(ui);
        if empty {
            ui.vertical_centered(|ui| {
                ui.add_space(style::SPACE_EMPTY_STATE);
                if loading {
                    if !queued {
                        ui.spinner();
                    }
                    let status = if queued {
                        "background_ops.queued"
                    } else if searching {
                        "search.searching"
                    } else {
                        "status.loading"
                    };
                    ui.label(self.t(status).to_owned());
                } else if no_results {
                    ui.label(self.t("search.no_results").to_owned());
                } else if self.tabs[self.active].secure_trash {
                    ui.label(self.t("trash.empty_list").to_owned());
                } else {
                    ui.label(self.t("folder.empty").to_owned());
                }
            });
            // Фон регистрируется после заглушки: её ответ наведения иначе перехватывает правый клик.
            let bg_resp = crate::ui::list::list_background_response(
                ui,
                self.store.settings.view_mode,
                bg_rect,
            );
            let marquee_rect = if self.store.settings.view_mode == ViewMode::Table {
                self.update_selection_marquee(ctx, &bg_resp, bg_resp.rect)
            } else {
                self.tabs[self.active].selection_marquee = None;
                None
            };
            crate::ui::list::paint_selection_marquee(ui, marquee_rect);
            self.finish_selection_marquee(ctx);
            self.finish_list_background(&bg_resp, ui, ctx);
            return;
        }
        let bg_resp =
            crate::ui::list::list_background_response(ui, self.store.settings.view_mode, bg_rect);
        self.file_list(ctx, ui, bg_resp);
    }

    fn error_text(&self, e: &std::io::Error) -> String {
        self.operation_error_message(e.kind())
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
        tab.path.parent().unwrap_or(&tab.path).to_path_buf()
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
        self.pump_bookmark_state_cache(&ctx);
        crate::ui::file_dnd::clear_frame_feedback(&mut self.file_drop_feedback);
        crate::ui::file_dnd::clear_frame_feedback(&mut self.file_drop_release);
        if !ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary)) {
            self.file_drag_candidate = None;
        }
        let escape_pressed = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let _ = crate::ui::file_dnd::cancel_on_escape(&ctx, escape_pressed);
        let close_requested = ctx.input(|input| input.viewport().close_requested());
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
        if self.shutdown.is_active() {
            self.shutdown_frame(ui, &ctx, close_requested);
            return;
        }
        self.drain_bg_op_events();
        self.drain_messages(&ctx);
        if let Some((request_id, was_current)) = self.update_check.expire(Instant::now()) {
            tracing::warn!(request_id, "manual update check timed out");
            if was_current && self.show_about {
                self.update_status = None;
                self.update_err = Some(self.t("update.check_timeout").to_owned());
            }
        }
        if let Some((_, deadline)) = self.update_check.deadline {
            ctx.request_repaint_after(deadline.saturating_duration_since(Instant::now()));
        }
        self.sync_entry_details(&ctx);
        self.sync_dir_watch(&ctx);
        self.schedule_disk_usage_refresh(&ctx);
        self.drain_ops_events(&ctx);
        if close_requested {
            self.start_shutdown();
            if self.shutdown.is_active() {
                self.shutdown_frame(ui, &ctx, close_requested);
                return;
            }
        }
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

        let tree_panel_width = egui::Panel::left("tree_panel")
            .resizable(true)
            .default_size(clamp_tree_panel_width(
                self.store.settings.tree_panel_width,
            ))
            .size_range(TREE_PANEL_MIN_WIDTH..=TREE_PANEL_MAX_WIDTH)
            .show(ui, |ui| self.tree_panel(&ctx, ui))
            .response
            .rect
            .width();
        self.note_tree_panel_width(&ctx, tree_panel_width);

        egui::Panel::right("props_panel")
            .resizable(false)
            .default_size(crate::ui::PROPERTY_PANEL_FIXED_WIDTH)
            .size_range(crate::ui::property_panel_fixed_range())
            .show(ui, |ui| self.props_panel(ui));

        egui::Panel::bottom("ops_panel").show(ui, |ui| self.ops_panel(ui));

        egui::Panel::bottom("status_bar").show(ui, |ui| self.show_active_tab_status(ui));
        egui::CentralPanel::default().show(ui, |ui| self.show_active_file_content(&ctx, ui));

        self.show_tab_drag_preview(&ctx);
        self.flush_pending_open_tabs(&ctx);
        self.show_detached_tab_windows(&ctx);
        // Не теряем release из detached-viewport: все цели участвуют в общем выборе адресата до enqueue.
        self.finish_file_drop_frame(ui, &ctx);
        self.show_file_drag_preview(&ctx);
        self.finish_thumbnail_frame();
        self.about_window(&ctx);
        self.hotkeys_window(&ctx);
        self.settings_window(&ctx);
        self.secure_trash_window(&ctx);
        self.trash_confirm_windows(&ctx);
        self.props_window(&ctx);
        self.preview_window(&ctx);
        self.zip_status_window(&ctx);
        self.ops_dialogs(&ctx);
        self.poll_update_restart(&ctx);
        self.persist_tree_panel_width_if_due(&ctx);

        // Keep repainting while operations run so progress stays live.
        if !self.ops_ui.jobs.is_empty()
            || self.ops_ui.conflict.is_some()
            || self.ops_ui.error_q.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }
}

impl Drop for FileManagerApp {
    fn drop(&mut self) {
        self.prepare_shutdown();
        if let Some(worker) = self.ops_worker.take() {
            let _ = worker.join();
        }
        self.bg_ops_pool.join_workers();
        if self.shutdown.is_active() {
            tracing::info!(
                elapsed_ms = self.shutdown.elapsed(Instant::now()).as_millis(),
                "join файлового worker-а и пула завершён; приложение штатно выходит"
            );
        }
    }
}

#[cfg(test)]
mod tests;
