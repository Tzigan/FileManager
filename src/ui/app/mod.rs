mod archive;
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

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
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
use crate::ui::tree::TreeState;

const DISK_USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// Канал эскизов остаётся отдельным от пула тяжёлых независимых операций.
pub(crate) const THUMBNAIL_QUEUE_CAPACITY: usize = 24;

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
    UpdateFound { manifest: UpdateManifest },
    /// Получено обновление прогресса скачивания.
    UpdateProgress { downloaded: u64, total: u64 },
    /// Проверенный пакет подготовлен для установки отдельным updater.
    UpdateReady(PathBuf),
    /// При проверке вручную установлена последняя версия.
    UpdateLatest,
    /// Проверка или загрузка обновления завершилась ошибкой.
    UpdateFailed(String),
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

pub struct FileManagerApp {
    pub(crate) store: SettingsStore,
    pub(crate) i18n: I18n,
    dirs: AppDirs,
    show_about: bool,
    about_viewport_placed: bool,
    show_settings: bool,
    settings_viewport_placed: bool,
    /// Активная категория окна настроек. На сохранение значений не влияет.
    settings_category: dialogs::SettingsCategory,
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
    pub(crate) system_bookmarks: Vec<SystemBookmark>,
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
                        let _ = msg_tx.send(WorkerMsg::UpdateFound { manifest: r });
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
            show_settings: false,
            settings_viewport_placed: false,
            settings_category: dialogs::SettingsCategory::default(),
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
            system_bookmarks: crate::core::known_folders::system_bookmarks(),
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
        crate::core::logging::set_panic_language(lang);
        self.persist();
    }

    pub(crate) fn persist(&mut self) {
        if let Err(e) = self.store.save() {
            let path = self.dirs.config_dir.join("settings.json");
            let error_kind = e
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind);
            tracing::info!(
                path = %path.display(),
                error_kind = ?error_kind,
                error = %format!("{e:#}"),
                "settings save failed"
            );
            self.ops_ui.journal.push(self.t("settings.save_failed").to_owned());
            self.ops_ui.show_journal = true;
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
            let _ = self.bg_ops_pool.cancel(id);
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
            return;
        }
        self.file_list(ctx, ui);
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
        self.drain_bg_op_events();
        self.drain_messages(&ctx);
        self.sync_entry_details(&ctx);
        self.sync_dir_watch(&ctx);
        self.schedule_disk_usage_refresh(&ctx);
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
        self.finish_thumbnail_frame();
        self.about_window(&ctx);
        self.settings_window(&ctx);
        self.secure_trash_window(&ctx);
        self.trash_confirm_windows(&ctx);
        self.props_window(&ctx);
        self.preview_window(&ctx);
        self.zip_status_window(&ctx);
        self.ops_dialogs(&ctx);
        self.poll_update_restart(&ctx);

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
        self.bg_ops_pool.request_shutdown();
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
        if let Some(reply_tx) = &self.op_reply {
            if let Some((id, _, _)) = self.ops_ui.conflict.as_ref() {
                let _ = reply_tx.send(OpReply::Conflict {
                    id: *id,
                    action: crate::core::ops::ConflictAction::Cancel,
                });
            }
            if let Some((id, _, _, _)) = self.ops_ui.error_q.as_ref() {
                let _ = reply_tx.send(OpReply::Error {
                    id: *id,
                    action: crate::core::ops::ErrorAction::Cancel,
                });
            }
        }
        self.op_reply.take();
        self.ops_tx.take();
        if let Some(worker) = self.ops_worker.take() {
            let _ = worker.join();
        }
        self.bg_ops_pool.join_workers();
    }
}

#[cfg(test)]
mod tests;
