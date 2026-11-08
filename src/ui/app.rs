use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;

use anyhow::Context;

use super::style;
use crate::core::fs::{self, FileEntry, ReadDirError, SortKey};
use crate::core::i18n::{I18n, Language};
use crate::core::ops::{self, OpEvent, OpJob, OpReply};
use crate::core::paths::AppDirs;
use crate::core::preview::TextPreview;
use crate::core::search::{self, SearchValidationError};
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
    /// A newer release exists on GitHub (version, exe URL).
    UpdateFound { manifest: UpdateManifest },
    /// The verified update package is staged and ready for the separate updater.
    UpdateReady(PathBuf),
    /// The app is on the latest version (manual check).
    UpdateLatest,
    /// Update check or download failed.
    UpdateFailed(String),
}

struct PreviewState {
    target: Option<PathBuf>,
    text: Option<TextPreview>,
    image: Option<egui::TextureHandle>,
    image_dimensions: Option<(u32, u32)>,
    error: Option<String>,
    loading: bool,
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
    /// Update status/error text for the update window.
    update_err: Option<String>,
    /// Index of the tab currently being drag-reordered.
    tab_drag: Option<usize>,
    /// Tab press anchor for manual drag detection (index, press position).
    tab_press: Option<(usize, egui::Pos2)>,
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
        ops::spawn(jobs_rx, reply_rx, events_tx);
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
            update_err: None,
            tab_drag: None,
            tab_press: None,
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

    /// Open `path` in a new tab (pipe-forwarded from another instance).
    pub(crate) fn open_path_in_new_tab(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.active = super::tabs::append_tab(&mut self.tabs, path);
        self.navigate_fresh(self.active, ctx);
    }

    pub(crate) fn queue_open_path_in_new_tab(&mut self, path: PathBuf) {
        self.pending_open_tabs.push(path);
    }

    fn flush_pending_open_tabs(&mut self, ctx: &egui::Context) {
        for path in std::mem::take(&mut self.pending_open_tabs) {
            self.open_path_in_new_tab(path, ctx);
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
        self.persist();
    }

    pub(crate) fn persist(&self) {
        if let Err(e) = self.store.save() {
            tracing::error!("failed to save settings: {e}");
        }
    }

    fn persist_tabs(&mut self) {
        self.store.settings.tabs = self.tabs.iter().map(persisted_tab_path).collect();
        self.store.settings.pinned_tabs = self
            .tabs
            .iter()
            .filter(|tab| tab.pinned)
            .map(persisted_tab_path)
            .collect();
        self.store.settings.active_tab = self.active;
        self.persist();
    }

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    /// User-driven navigation to a new path (pushes history).
    pub(crate) fn navigate(&mut self, tab: usize, path: PathBuf, ctx: &egui::Context) {
        if is_zip_file(&path) {
            self.open_archive(tab, path, ctx);
            return;
        }
        let current = self.tabs[tab].path.clone();
        if current != path {
            self.tabs[tab].history.push(current);
            self.tabs[tab].path = path;
        }
        self.tabs[tab].archive_prefix = None;
        self.tabs[tab].archive_back.clear();
        self.tabs[tab].archive_forward.clear();
        self.tabs[tab].search.reset();
        self.navigate_fresh(tab, ctx);
    }

    fn open_archive(&mut self, tab: usize, archive: PathBuf, ctx: &egui::Context) {
        self.open_archive_at(tab, archive, String::new(), ctx);
    }

    fn open_archive_at(
        &mut self,
        tab: usize,
        archive: PathBuf,
        prefix: String,
        ctx: &egui::Context,
    ) {
        let state = &mut self.tabs[tab];
        if state.archive_prefix.is_none() && state.path != archive {
            state.history.push(state.path.clone());
        }
        state.path = archive;
        state.archive_prefix = Some(prefix);
        state.archive_back.clear();
        state.archive_forward.clear();
        state.search.reset();
        self.navigate_fresh(tab, ctx);
    }

    pub(crate) fn navigate_zip(&mut self, tab: usize, prefix: String, ctx: &egui::Context) {
        let state = &mut self.tabs[tab];
        let current = state.archive_prefix.clone().unwrap_or_default();
        if current != prefix {
            state.archive_back.push(current);
            state.archive_forward.clear();
            state.archive_prefix = Some(prefix);
        }
        state.search.reset();
        self.navigate_fresh(tab, ctx);
    }

    /// Reload `tab`'s current path in the background without touching history.
    pub(crate) fn navigate_fresh(&mut self, tab: usize, ctx: &egui::Context) {
        self.cancel_all_thumbnails();
        if self.tabs[tab].archive_prefix.is_none() && is_zip_file(&self.tabs[tab].path) {
            self.tabs[tab].archive_prefix = Some(String::new());
        }
        if self.tabs[tab].search.has_criteria() {
            if self.tabs[tab].search.debounce_at.is_none() {
                self.tabs[tab].search.schedule();
            }
            self.tabs[tab].search.debounce_at = None;
            self.start_search(tab, ctx);
            return;
        }
        let t = &mut self.tabs[tab];
        t.search.cancel.store(true, Ordering::Relaxed);
        t.search.running = false;
        t.search.results_active = false;
        t.search.debounce_at = None;
        t.generation += 1;
        let generation = t.generation;
        let tab_id = t.id;
        t.cancel.store(true, Ordering::Relaxed);
        t.cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = t.cancel.clone();

        t.status = ListStatus::Loading;
        t.error = None;
        t.clear_selection();
        t.cursor = 0;
        let path = t.path.clone();
        let archive_prefix = t.archive_prefix.clone();
        t.address_input = match &archive_prefix {
            Some(prefix) => format!("{}!/{prefix}", path.display()),
            None => path.display().to_string(),
        };

        if tab == self.active {
            let reveal_path = if archive_prefix.is_some() {
                path.parent().unwrap_or(&path)
            } else {
                &path
            };
            self.tree.reveal(reveal_path);
            self.persist_tabs();
        }

        let tx = self.msg_tx.clone();
        let ctx2 = ctx.clone();
        tracing::info!(tab_id, generation, "folder listing queued");
        thread::spawn(move || {
            tracing::info!(tab_id, generation, "folder listing started");
            let result = match archive_prefix {
                Some(prefix) => crate::platform::read_zip_dir_cancellable(
                    &path, &prefix, &cancel,
                ),
                None => fs::read_dir(&path, &cancel),
            };
            match &result {
                Ok(entries) => tracing::info!(tab_id, generation, count = entries.len(), "folder listing completed"),
                Err(ReadDirError::Cancelled) => tracing::info!(tab_id, generation, "folder listing cancelled"),
                Err(ReadDirError::Io(error)) => tracing::warn!(tab_id, generation, error = %error, "folder listing failed"),
            }
            let _ = tx.send(WorkerMsg::Listing { tab_id, generation, result });
            ctx2.request_repaint();
        });
    }

    fn go_back(&mut self, ctx: &egui::Context) {
        if let Some(current) = self.tabs[self.active].archive_prefix.clone() {
            if let Some(previous) = self.tabs[self.active].archive_back.pop() {
                self.tabs[self.active].archive_forward.push(current);
                self.tabs[self.active].archive_prefix = Some(previous);
                self.tabs[self.active].search.reset();
                self.navigate_fresh(self.active, ctx);
                return;
            }
            if !self.tabs[self.active].history.can_back() {
                return;
            }
            self.tabs[self.active].archive_prefix = None;
        }
        let cur = self.tabs[self.active].path.clone();
        if let Some(path) = self.tabs[self.active].history.back(cur) {
            self.tabs[self.active].path = path;
            self.tabs[self.active].archive_back.clear();
            self.tabs[self.active].archive_forward.clear();
            self.tabs[self.active].search.reset();
            self.navigate_fresh(self.active, ctx);
        }
    }

    fn go_forward(&mut self, ctx: &egui::Context) {
        if self.tabs[self.active].archive_prefix.is_some()
            && !self.tabs[self.active].archive_forward.is_empty()
        {
            let current = self.tabs[self.active]
                .archive_prefix
                .clone()
                .unwrap_or_default();
            let next = self.tabs[self.active].archive_forward.pop().unwrap();
            self.tabs[self.active].archive_back.push(current);
            self.tabs[self.active].archive_prefix = Some(next);
            self.tabs[self.active].search.reset();
            self.navigate_fresh(self.active, ctx);
            return;
        }
        let cur = self.tabs[self.active].path.clone();
        if let Some(path) = self.tabs[self.active].history.forward(cur) {
            self.tabs[self.active].path = path;
            self.tabs[self.active].archive_prefix = None;
            self.tabs[self.active].archive_back.clear();
            self.tabs[self.active].archive_forward.clear();
            self.tabs[self.active].search.reset();
            self.navigate_fresh(self.active, ctx);
        }
    }

    fn go_up(&mut self, ctx: &egui::Context) {
        if let Some(prefix) = self.tabs[self.active].archive_prefix.clone() {
            if !prefix.is_empty() {
                let parent = prefix.rsplit_once('/').map_or("", |(parent, _)| parent);
                self.navigate_zip(self.active, parent.to_owned(), ctx);
            } else if let Some(parent) = self.tabs[self.active]
                .path
                .parent()
                .map(Path::to_path_buf)
            {
                self.tabs[self.active].archive_prefix = None;
                self.tabs[self.active].archive_back.clear();
                self.tabs[self.active].archive_forward.clear();
                self.navigate(self.active, parent, ctx);
            }
            return;
        }
        if let Some(parent) = self.tabs[self.active].path.parent().map(|p| p.to_path_buf()) {
            self.navigate(self.active, parent, ctx);
        }
    }

    pub(crate) fn refresh(&mut self, ctx: &egui::Context) {
        self.navigate_fresh(self.active, ctx);
    }

    fn cancel_loading(&mut self) {
        let t = &mut self.tabs[self.active];
        t.cancel.store(true, Ordering::Relaxed);
        t.search.cancel.store(true, Ordering::Relaxed);
        t.search.running = false;
        t.search.debounce_at = None;
        t.status = ListStatus::Ready;
    }

    fn schedule_search(&mut self) {
        self.tabs[self.active].search.schedule();
    }

    fn start_search(&mut self, tab: usize, ctx: &egui::Context) {
        if !self.tabs[tab].search.has_criteria() {
            self.tabs[tab].search.running = false;
            self.tabs[tab].search.results_active = false;
            self.navigate_fresh(tab, ctx);
            return;
        }

        let options = {
            let search = &self.tabs[tab].search;
            search::parse_options(
                &search.query,
                search.recursive,
                search.fuzzy,
                &search.extensions,
                &search.min_size,
                &search.max_size,
                &search.modified_from,
                &search.modified_to,
            )
        };
        let options = match options {
            Ok(options) => options,
            Err(error) => {
                let key = match error {
                    SearchValidationError::InvalidSize => "search.error_size",
                    SearchValidationError::SizeRange => "search.error_size_range",
                    SearchValidationError::InvalidDate => "search.error_date",
                    SearchValidationError::DateRange => "search.error_date_range",
                };
                let message = self.t(key).to_owned();
                let state = &mut self.tabs[tab];
                state.search.running = false;
                state.search.results_active = false;
                state.search.validation_error = Some(message);
                state.status = ListStatus::Ready;
                return;
            }
        };

        let state = &mut self.tabs[tab];
        state.generation = state.generation.wrapping_add(1);
        state.cancel.store(true, Ordering::Relaxed);
        state.cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.status = ListStatus::Loading;
        state.error = None;
        state.search.running = true;
        state.search.results_active = true;
        state.search.validation_error = None;
        state.clear_selection();
        state.cursor = 0;
        let tab_id = state.id;
        let generation = state.search.generation;
        let root = state.path.clone();
        let archive_prefix = state.archive_prefix.clone();
        let cancel = state.search.cancel.clone();
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = match archive_prefix {
                Some(prefix) => {
                    crate::platform::search_zip(&root, &prefix, &options, &cancel)
                }
                None => search::search(&root, &options, &cancel),
            };
            let _ = tx.send(WorkerMsg::SearchResults {
                tab_id,
                generation,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn dispatch_due_searches(&mut self, ctx: &egui::Context) {
        let Some(due) = self.tabs[self.active].search.debounce_at else {
            return;
        };
        let now = std::time::Instant::now();
        if due > now {
            ctx.request_repaint_after(due.duration_since(now));
            return;
        }
        self.tabs[self.active].search.debounce_at = None;
        if self.tabs[self.active].search.has_criteria() {
            self.start_search(self.active, ctx);
        } else if self.tabs[self.active].search.results_active {
            self.tabs[self.active].search.results_active = false;
            self.navigate_fresh(self.active, ctx);
        }
    }

    // ------------------------------------------------------------------
    // Tabs
    // ------------------------------------------------------------------

    fn new_tab(&mut self, ctx: &egui::Context) {
        let source = &self.tabs[self.active];
        let path = source.path.clone();
        let archive_prefix = source.archive_prefix.clone();
        self.active = super::tabs::append_tab(&mut self.tabs, path);
        self.tabs[self.active].archive_prefix = archive_prefix;
        self.navigate_fresh(self.active, ctx);
    }

    fn close_tab(&mut self, idx: usize, ctx: &egui::Context) {
        let Some(active) = super::tabs::close_tab_state(&mut self.tabs, self.active, idx) else {
            return;
        };
        self.active = active;
        self.persist_tabs();
        ctx.request_repaint();
    }

    fn switch_tab(&mut self, idx: usize) {
        if idx != self.active && idx < self.tabs.len() {
            self.active = idx;
            self.persist_tabs();
        }
    }

    fn duplicate_tab(&mut self, idx: usize, ctx: &egui::Context) {
        let path = self.tabs[idx].path.clone();
        let archive_prefix = self.tabs[idx].archive_prefix.clone();
        let mut tab = Tab::new(path);
        tab.archive_prefix = archive_prefix;
        self.tabs.insert(idx + 1, tab);
        self.active = idx + 1;
        self.navigate_fresh(self.active, ctx);
    }

    fn close_others(&mut self, idx: usize, ctx: &egui::Context) {
        if self.tabs.len() <= 1 {
            return;
        }
        let Some(active) = super::tabs::close_other_tabs_state(&mut self.tabs, idx) else {
            return;
        };
        self.active = active;
        self.persist_tabs();
        ctx.request_repaint();
    }

    fn toggle_pin(&mut self, idx: usize) {
        self.tabs[idx].pinned = !self.tabs[idx].pinned;
        self.persist_tabs();
    }

    fn move_tab(&mut self, src: usize, dst: usize) {
        let Some(dst) = super::tabs::move_to_drop_slot(&mut self.tabs, src, dst) else {
            return;
        };
        self.active = dst;
        self.persist_tabs();
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
                    self.pending_update = Some(manifest);
                    self.update_err = None;
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateReady(stage_dir) => {
                    self.update_busy = false;
                    let launch_result = std::env::current_exe()
                        .context("locate current FileManager executable")
                        .and_then(|target| {
                            crate::platform::launch_updater(
                                &stage_dir,
                                &target,
                                std::process::id(),
                            )
                        });
                    match launch_result {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            let _ = std::fs::remove_dir_all(&stage_dir);
                            self.update_err = Some(e.to_string());
                            ctx.request_repaint();
                        }
                    }
                }
                WorkerMsg::UpdateLatest => {
                    self.update_err = Some(self.t("update.latest").to_owned());
                    ctx.request_repaint();
                }
                WorkerMsg::UpdateFailed(e) => {
                    self.update_busy = false;
                    self.update_err = Some(e);
                    ctx.request_repaint();
                }
            }
        }
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

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let text_edit_focused = ctx.text_edit_focused();
        let mods = ctx.input(|i| i.modifiers);
        let input = |i: &egui::InputState| i.clone();

        // Prefer file operations when a file selection/internal file clipboard
        // exists. This also recovers if the address TextEdit retained focus after
        // the user clicked a file row; without a file selection normal text-edit
        // shortcuts (copy/paste in the address bar) remain untouched.
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::C)) {
            match super::opsui::clipboard_target(
                super::opsui::ClipboardCommand::Copy,
                text_edit_focused,
                !self.selected_paths().is_empty(),
                false,
            ) {
                super::opsui::ClipboardTarget::Files => {
                    tracing::info!("Ctrl+C shortcut received");
                    self.act_copy();
                }
                super::opsui::ClipboardTarget::Text => {
                    ctx.input_mut(|i| i.events.push(egui::Event::Copy));
                }
            }
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::X)) {
            match super::opsui::clipboard_target(
                super::opsui::ClipboardCommand::Cut,
                text_edit_focused,
                !self.selected_paths().is_empty(),
                false,
            ) {
                super::opsui::ClipboardTarget::Files => self.act_cut(),
                super::opsui::ClipboardTarget::Text => {
                    ctx.input_mut(|i| i.events.push(egui::Event::Cut));
                }
            }
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::V)) {
            let has_file_clipboard = self.ops_ui.clipboard.is_some()
                || !crate::platform::get_clipboard_files().is_empty();
            match super::opsui::clipboard_target(
                super::opsui::ClipboardCommand::Paste,
                text_edit_focused,
                false,
                has_file_clipboard,
            ) {
                super::opsui::ClipboardTarget::Files => {
                    tracing::info!("Ctrl+V shortcut received");
                    self.act_paste();
                }
                super::opsui::ClipboardTarget::Text => {
                    let text = crate::platform::get_clipboard_text();
                    if !text.is_empty() {
                        ctx.input_mut(|i| i.events.push(egui::Event::Paste(text)));
                    }
                }
            }
            return;
        }
        if text_edit_focused {
            return;
        }

        // Tab management.
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::T)) {
            self.new_tab(ctx);
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::W)) {
            self.close_tab(self.active, ctx);
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let next = (self.active + 1) % self.tabs.len();
            self.switch_tab(next);
            return;
        }
        if mods.ctrl && mods.shift && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let prev = (self.active + self.tabs.len() - 1) % self.tabs.len();
            self.switch_tab(prev);
            return;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.act_delete(mods.shift);
            return;
        }
        if mods.alt && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.act_props();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.act_rename();
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.act_preview(ctx);
            return;
        }

        // Selection and navigation.
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::A)) {
            let len = self.tabs[self.active].entries.len();
            self.tabs[self.active].selection.extend(0..len);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Backspace)) {
            self.go_up(ctx);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            let target = {
                let tab = &self.tabs[self.active];
                if tab.selection.len() == 1 {
                    tab.selection
                        .iter()
                        .next()
                        .and_then(|&i| tab.entries.get(i))
                        .filter(|entry| {
                            entry.is_dir
                                || (entry.archive_member.is_none()
                                    && entry.extension.as_deref() == Some("zip"))
                        })
                        .map(|entry| {
                            (
                                entry.path.clone(),
                                entry.archive_member.clone(),
                                entry.is_dir,
                                entry.extension.as_deref() == Some("zip"),
                            )
                        })
                } else {
                    None
                }
            };
            if let Some((_path, Some(member), true, _)) = target {
                self.navigate_zip(self.active, member, ctx);
            } else if let Some((path, None, is_dir, is_zip)) = target {
                if is_dir || is_zip {
                    self.navigate(self.active, path, ctx);
                }
            }
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) && mods.alt {
            self.go_back(ctx);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) && mods.alt {
            self.go_forward(ctx);
            return;
        }

        let len = self.tabs[self.active].entries.len();
        if len == 0 {
            return;
        }
        let cur = self.tabs[self.active].cursor.min(len - 1);
        let extend = mods.shift;
        let i = input(&ctx.input(|i| i.clone()));
        if i.key_pressed(egui::Key::ArrowDown) {
            self.move_cursor(cur.saturating_add(1), extend);
        } else if i.key_pressed(egui::Key::ArrowUp) {
            self.move_cursor(cur.saturating_sub(1), extend);
        } else if i.key_pressed(egui::Key::PageDown) {
            self.move_cursor(cur.saturating_add(20), extend);
        } else if i.key_pressed(egui::Key::PageUp) {
            self.move_cursor(cur.saturating_sub(20), extend);
        } else if i.key_pressed(egui::Key::Home) {
            self.move_cursor(0, extend);
        } else if i.key_pressed(egui::Key::End) {
            self.move_cursor(len - 1, extend);
        }
    }

    /// Files dropped onto the window from Windows — copy into current folder.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| dropped_paths(&i.raw.dropped_files));
        if dropped.is_empty() {
            return;
        }
        let dest = self.tabs[self.active].path.clone();
        if dropped.iter().any(|p| p == &dest || dest.starts_with(p)) {
            return; // dropped onto its own parent — no-op like Explorer
        }
        self.enqueue("ops.copying", crate::core::ops::OpKind::Copy { sources: dropped, dest });
    }

    fn move_cursor(&mut self, target: usize, extend: bool) {
        let tab = &mut self.tabs[self.active];
        let len = tab.entries.len();
        let Some(target) = super::tabs::apply_cursor_move(
            &mut tab.selection,
            &mut tab.anchor,
            tab.cursor,
            len,
            target,
            extend,
        ) else {
            return;
        };
        tab.cursor = target;
        self.scroll_to_cursor = true;
    }

    // ------------------------------------------------------------------
    // Chrome: menus, tab bar, toolbar, breadcrumbs
    // ------------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(self.t("menu.file").to_owned(), |ui| {
                if ui.button(self.t("integrate.add").to_owned()).clicked() {
                    if let Ok(exe) = std::env::current_exe() {
                        let title = self.t("integrate.title").to_owned();
                        if let Err(e) = install_context_menu(&exe, &title) {
                            self.ops_ui.journal.push(format!("context menu: {e}"));
                        }
                    }
                    ui.close();
                }
                if ui.button(self.t("integrate.remove").to_owned()).clicked() {
                    if let Err(e) = uninstall_context_menu() {
                        self.ops_ui.journal.push(format!("context menu: {e}"));
                    }
                    ui.close();
                }
                if ui.button(self.t("integrate.open_recycle").to_owned()).clicked() {
                    let _ = crate::platform::open_recycle_bin();
                    ui.close();
                }
                ui.separator();
                if ui.button(self.t("file.quit").to_owned()).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button(self.t("menu.view").to_owned(), |ui| {
                ui.menu_button(self.t("view.theme").to_owned(), |ui| {
                    for (theme, key) in [
                        (Theme::System, "theme.system"),
                        (Theme::Light, "theme.light"),
                        (Theme::Dark, "theme.dark"),
                    ] {
                        if ui
                            .selectable_label(self.store.settings.theme == theme, self.t(key).to_owned())
                            .clicked()
                        {
                            self.set_theme(theme);
                            ui.close();
                        }
                    }
                });
                ui.menu_button(self.t("view.language").to_owned(), |ui| {
                    for language in [Language::Ru, Language::En] {
                        if ui
                            .selectable_label(self.i18n.language() == language, language.label())
                            .clicked()
                        {
                            self.set_language(language);
                            ui.close();
                        }
                    }
                });
            });

            if ui.button(self.t("menu.settings").to_owned()).clicked() {
                self.show_settings = true;
            }

            ui.menu_button(self.t("menu.help").to_owned(), |ui| {
                if ui.button(self.t("help.about").to_owned()).clicked() {
                    self.show_about = true;
                    ui.close();
                }
            });
        });
    }

    fn tab_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        configure_tab_scroll_style(ui.style_mut());
        ui.horizontal(|ui| {
            let mut act: Option<(&str, usize)> = None;
            let mut drop_dst = None;
            let mut drag_src = None;
            let mut dragging_tab =
                is_tab_dragging(self.tab_drag, self.tab_press, ui.ctx().pointer_latest_pos());
            let metas: Vec<(bool, String)> = self
                .tabs
                .iter()
                .map(|t| (t.pinned, t.title()))
                .collect();

            let scroll_width = (ui.available_width() - 30.0).max(1.0);
            egui::ScrollArea::horizontal()
                .id_salt("tab_strip_scroll")
                .max_width(scroll_width)
                .auto_shrink([false, true])
                .content_margin(tab_strip_content_margin())
                .scroll_source(tab_scroll_source())
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
            for (i, (pinned, title)) in metas.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = style::SPACE_XXS;

                    // Hand-rolled tab widget: click-and-drag sense so a plain click
                    // stays a click (arrow cursor), while press+drag reorders.
                    let galley = ui.painter().layout_no_wrap(
                        title.clone(),
                        egui::TextStyle::Button.resolve(ui.style()),
                        ui.visuals().text_color(),
                    );
                    // The × lives inside the tab: right-side strip of the same rect.
                    let pin_w = if *pinned { 16.0 } else { 0.0 };
                    let close_w = if *pinned { 0.0 } else { 18.0 };
                    let size = galley.size() + egui::vec2(14.0 + pin_w + close_w, 6.0);
                    // click (not click_and_drag): a focusable widget would keep keyboard
                    // focus and kill every global shortcut after a tab click.
                    let (rect, resp) =
                        ui.allocate_exact_size(size, egui::Sense::click());
                    let x_rect =
                        egui::Rect::from_min_max(egui::pos2(rect.max.x - close_w, rect.min.y), rect.max);
                    let over_x = tab_close_button_hovered(
                        dragging_tab,
                        *pinned,
                        x_rect,
                        ui.ctx().pointer_latest_pos(),
                    );
                    let visuals = ui.visuals();
                    let painter = ui.painter();
                    if i == self.active {
                        painter.rect_filled(rect, style::RADIUS_ROW, visuals.selection.bg_fill);
                    } else if resp.hovered() {
                        painter.rect_filled(rect, style::RADIUS_ROW, visuals.widgets.hovered.bg_fill);
                    }
                    let color = if i == self.active {
                        visuals.strong_text_color()
                    } else {
                        visuals.text_color()
                    };
                    let text_left = rect.left() + 7.0 + pin_w;
                    let text_right = rect.right() - 7.0 - close_w;
                    painter.galley(
                        egui::pos2(
                            (text_left + text_right - galley.size().x) * 0.5,
                            rect.center().y - galley.size().y * 0.5,
                        ),
                        galley,
                        color,
                    );
                    if *pinned {
                        let pin_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + 4.0, rect.center().y - 6.0),
                            egui::vec2(12.0, 12.0),
                        );
                        icon_pin(painter, pin_rect, color);
                    }
                    if !*pinned {
                        if over_x {
                            painter.rect_filled(
                                x_rect.shrink(2.0),
                                style::RADIUS_ROW,
                                visuals.error_fg_color.gamma_multiply(0.25),
                            );
                        }
                        let x_color = if over_x {
                            visuals.error_fg_color
                        } else {
                            visuals.weak_text_color()
                        };
                        let cross = 4.0;
                        painter.line_segment(
                            [
                                x_rect.center() + egui::vec2(-cross, -cross),
                                x_rect.center() + egui::vec2(cross, cross),
                            ],
                            egui::Stroke::new(1.4, x_color),
                        );
                        painter.line_segment(
                            [
                                x_rect.center() + egui::vec2(-cross, cross),
                                x_rect.center() + egui::vec2(cross, -cross),
                            ],
                            egui::Stroke::new(1.4, x_color),
                        );
                    }

                    resp.context_menu(|ui| {
                        style::compact_context_menu(ui);
                        let close_txt = self.t("tab.close").to_owned();
                        let others_txt = self.t("tab.close_others").to_owned();
                        let dup_txt = self.t("tab.duplicate").to_owned();
                        let pin_txt = self.t(if *pinned { "tab.unpin" } else { "tab.pin" }).to_owned();
                        if !*pinned && ui.button(close_txt).clicked() {
                            act = Some(("close", i));
                            ui.close();
                        }
                        if ui.button(others_txt).clicked() {
                            act = Some(("close_others", i));
                            ui.close();
                        }
                        if ui.button(dup_txt).clicked() {
                            act = Some(("dup", i));
                            ui.close();
                        }
                        if ui.button(pin_txt).clicked() {
                            act = Some(("pin", i));
                            ui.close();
                        }
                    });
                    if !dragging_tab && resp.clicked() {
                        let in_x = resp
                            .interact_pointer_pos()
                            .map(|p| x_rect.contains(p))
                            .unwrap_or(false);
                        if in_x {
                            act = Some(("close", i));
                        } else {
                            act = Some(("switch", i));
                        }
                    }
                    if !dragging_tab && resp.middle_clicked() && !*pinned {
                        act = Some(("close", i));
                    }
                    // Manual drag: pressed on this tab and moved past the threshold.
                    if resp.is_pointer_button_down_on() {
                        if let Some(p) = ui.ctx().pointer_latest_pos() {
                            if self.tab_press.is_none() {
                                self.tab_press = Some((i, p));
                            }
                            if let Some((si, start)) = self.tab_press {
                                if si == i && p.distance(start) > TAB_DRAG_THRESHOLD {
                                    drag_src = Some(i);
                                    dragging_tab = true;
                                }
                            }
                        }
                    }
                    if drag_src.is_some() || self.tab_drag.is_some() {
                        if let Some(pos) = ui.ctx().pointer_latest_pos() {
                            if rect.contains(pos) {
                                let before = pos.x < rect.center().x;
                                drop_dst = Some(if before { i } else { i + 1 });
                                // Vertical insertion line at the hovered edge.
                                ui.painter().vline(
                                    if before { rect.left() } else { rect.right() },
                                    rect.y_range(),
                                    egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                                );
                            }
                        }
                    }
                    let _ = resp;
                });
            }
                    });
                });
            if ui.small_button("+").clicked() {
                self.new_tab(ctx);
            }
            if let Some(src) = drag_src.or(self.tab_drag) {
                // Floating chip under the pointer while a tab is dragged.
                if let Some(pos) = ui.ctx().pointer_latest_pos() {
                    if let Some((_, title)) = metas.get(src) {
                        egui::Area::new(ui.scope_id().with("tab_drag_chip"))
                            .order(egui::Order::Foreground)
                            .current_pos(pos + egui::vec2(10.0, -16.0))
                            .show(ui.ctx(), |ui| {
                                egui::Frame::new()
                                    .fill(ui.visuals().extreme_bg_color)
                                    .corner_radius(style::RADIUS_ROW)
                                    .inner_margin(egui::Margin::symmetric(
                                        style::FRAME_PADDING_X,
                                        style::FRAME_PADDING_Y,
                                    ))
                                    .stroke(egui::Stroke::new(1.0, ui.visuals().selection.stroke.color))
                                    .show(ui, |ui| {
                                        ui.label(title.clone());
                                    });
                            });
                    }
                }
            }
            if let Some(src) = drag_src {
                self.tab_drag = Some(src);
            }
            if ui.ctx().input(|i| i.pointer.any_released()) {
                if self.tab_drag.is_some() {
                    if let (Some(src), Some(dst)) = (self.tab_drag, drop_dst) {
                        self.move_tab(src, dst);
                    }
                    self.tab_drag = None;
                }
                self.tab_press = None;
            }
            if let Some((what, i)) = act {
                match what {
                    "switch" => self.switch_tab(i),
                    "close" => self.close_tab(i, ctx),
                    "close_others" => self.close_others(i, ctx),
                    "dup" => self.duplicate_tab(i, ctx),
                    "pin" => self.toggle_pin(i),
                    _ => {}
                }
            }
        });
    }

    fn toolbar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (can_back, can_fwd, has_parent, loading) = {
                let t = &self.tabs[self.active];
                (
                    t.history.can_back() || !t.archive_back.is_empty(),
                    t.history.can_forward() || !t.archive_forward.is_empty(),
                    t.archive_prefix
                        .as_ref()
                        .is_some_and(|prefix| !prefix.is_empty())
                        || t.path.parent().is_some(),
                    t.status == ListStatus::Loading,
                )
            };
            let in_archive = self.tabs[self.active].archive_prefix.is_some();
            if icon_button(ui, can_back, self.t("toolbar.back").to_owned(), |p, r, c| {
                icon_arrow(p, r, egui::vec2(-1.0, 0.0), c);
            })
            .clicked()
            {
                self.go_back(ctx);
            }
            if icon_button(ui, can_fwd, self.t("toolbar.forward").to_owned(), |p, r, c| {
                icon_arrow(p, r, egui::vec2(1.0, 0.0), c);
            })
            .clicked()
            {
                self.go_forward(ctx);
            }
            if icon_button(ui, has_parent, self.t("toolbar.up").to_owned(), |p, r, c| {
                icon_arrow(p, r, egui::vec2(0.0, -1.0), c);
            })
            .clicked()
            {
                self.go_up(ctx);
            }
            if icon_button(ui, true, self.t("toolbar.refresh").to_owned(), |p, r, c| {
                icon_refresh(p, r, c);
            })
            .clicked()
            {
                self.refresh(ctx);
            }
            if loading {
                ui.spinner();
                if ui.button(self.t("toolbar.cancel").to_owned()).clicked() {
                    self.cancel_loading();
                }
            }

            // Bookmark toggle for the current folder.
            let marked = !in_archive && self
                .store
                .settings
                .bookmarks
                .contains(&self.tabs[self.active].path);
            let star_color = ui.visuals().selection.stroke.color;
            if icon_button(ui, !in_archive, self.t("toolbar.bookmark").to_owned(), |p, r, c| {
                icon_star(p, r, if marked { star_color } else { c }, marked);
            })
            .clicked()
            {
                self.act_bookmark();
            }

            // "New / Paste" menu.
            ui.menu_button("+", |ui| {
                if in_archive {
                    if ui.button(self.t("zip.extract_all").to_owned()).clicked() {
                        self.extract_zip_all(ctx);
                        ui.close();
                    }
                } else {
                    if ui.button(self.t("ctx.new_folder").to_owned()).clicked() {
                        self.ops_ui.create = Some((true, String::new()));
                        ui.close();
                    }
                    if ui.button(self.t("ctx.new_file").to_owned()).clicked() {
                        self.ops_ui.create = Some((false, String::new()));
                        ui.close();
                    }
                    if ui
                        .add_enabled(self.ops_ui.clipboard.is_some(), egui::Button::new(self.t("ctx.paste").to_owned()))
                        .clicked()
                    {
                        self.act_paste();
                        ui.close();
                    }
                }
            });

            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.tabs[self.active].address_input)
                    .desired_width(f32::INFINITY),
            );
            // A click anywhere outside the address bar releases its keyboard focus
            // (TextEdit focus is otherwise sticky and blocks every file shortcut).
            if resp.clicked_elsewhere() {
                resp.surrender_focus();
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let address = self.tabs[self.active].address_input.trim().to_owned();
                if let Some((archive, prefix)) = parse_archive_address(&address) {
                    self.open_archive_at(self.active, archive, prefix, ctx);
                } else {
                    self.navigate(self.active, PathBuf::from(address), ctx);
                }
            }
        });

        // Breadcrumbs row.
        let path = self.tabs[self.active].path.clone();
        let archive_prefix = self.tabs[self.active].archive_prefix.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = style::SPACE_XXS;
            for (label, seg_path) in breadcrumb_segments(&path) {
                if ui.selectable_label(false, label).clicked() {
                    self.navigate(self.active, seg_path, ctx);
                }
                ui.label("›");
            }
            if let Some(prefix) = archive_prefix.filter(|prefix| !prefix.is_empty()) {
                let mut current = String::new();
                for component in prefix.split('/') {
                    if !current.is_empty() {
                        current.push('/');
                    }
                    current.push_str(component);
                    if ui.selectable_label(false, component).clicked() {
                        self.navigate_zip(self.active, current.clone(), ctx);
                    }
                    ui.label("›");
                }
            }
        });
    }

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        let hint = self.t("search.hint").to_owned();
        let recursive_label = self.t("search.subfolders").to_owned();
        let fuzzy_label = self.t("search.fuzzy").to_owned();
        let filters_label = self.t("search.filters").to_owned();
        let clear_label = self.t("search.clear").to_owned();
        let extension_label = self.t("search.extension").to_owned();
        let min_size_label = self.t("search.min_size").to_owned();
        let max_size_label = self.t("search.max_size").to_owned();
        let from_label = self.t("search.modified_from").to_owned();
        let to_label = self.t("search.modified_to").to_owned();
        let date_hint = self.t("search.date_hint").to_owned();
        let size_hint = self.t("search.size_hint").to_owned();
        let mut changed = false;
        let mut submit_now = false;

        ui.horizontal_wrapped(|ui| {
            let search = &mut self.tabs[self.active].search;
            let response = ui.add(
                egui::TextEdit::singleline(&mut search.query)
                    .hint_text(hint)
                    .desired_width(250.0),
            );
            changed |= response.changed();
            submit_now |=
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            changed |= ui.checkbox(&mut search.recursive, recursive_label).changed();
            changed |= ui.checkbox(&mut search.fuzzy, fuzzy_label).changed();
            if ui.button(filters_label).clicked() {
                search.filters_expanded = !search.filters_expanded;
            }
            if search.has_criteria() && ui.button(clear_label).clicked() {
                search.query.clear();
                search.recursive = false;
                search.fuzzy = false;
                search.extensions.clear();
                search.min_size.clear();
                search.max_size.clear();
                search.modified_from.clear();
                search.modified_to.clear();
                changed = true;
            }
        });

        if self.tabs[self.active].search.filters_expanded {
            ui.horizontal_wrapped(|ui| {
                let search = &mut self.tabs[self.active].search;
                ui.label(extension_label);
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut search.extensions).desired_width(130.0))
                    .changed();
                ui.label(min_size_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.min_size)
                            .hint_text(size_hint.clone())
                            .desired_width(90.0),
                    )
                    .changed();
                ui.label(max_size_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.max_size)
                            .hint_text(size_hint)
                            .desired_width(90.0),
                    )
                    .changed();
                ui.label(from_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.modified_from)
                            .hint_text(date_hint.clone())
                            .desired_width(105.0),
                    )
                    .changed();
                ui.label(to_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.modified_to)
                            .hint_text(date_hint)
                            .desired_width(105.0),
                    )
                    .changed();
            });
        }

        if let Some(error) = &self.tabs[self.active].search.validation_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if changed || submit_now {
            self.schedule_search();
            if submit_now {
                self.tabs[self.active].search.debounce_at = Some(std::time::Instant::now());
            }
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let title = self.t("about.title").to_owned();
        let config_label = self.t("label.config_dir").to_owned();
        let log_label = self.t("label.log_dir").to_owned();
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-about");
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(500.0, 240.0),
            self.about_viewport_placed,
        );
        let mut close_requested = false;
        ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, _class| {
            let viewport_ctx = viewport_ui.ctx().clone();
            if viewport_ctx.input(|i| i.viewport().close_requested()) {
                viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                close_requested = true;
                return;
            }
            egui::CentralPanel::default().show(viewport_ui, |ui| {
                ui.heading(format!("FileManager v{}", env!("CARGO_PKG_VERSION")));
                ui.label("Rust + egui/eframe");
                ui.add_space(style::SPACE_SM);
                ui.label(format!("{}: {}", config_label, self.dirs.config_dir.display()));
                ui.label(format!("{}: {}", log_label, self.dirs.log_dir.display()));
                ui.add_space(style::SPACE_SM);
                if ui.button(self.t("update.check").to_owned()).clicked() {
                    self.spawn_update_check(&viewport_ctx);
                }
            });
        });
        self.about_viewport_placed = !close_requested;
        if close_requested {
            self.show_about = false;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let title = self.t("menu.settings").to_owned();
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-settings");
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(560.0, 205.0),
            self.settings_viewport_placed,
        );
        let mut close_requested = false;
        ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, _class| {
            let viewport_ctx = viewport_ui.ctx().clone();
            if viewport_ctx.input(|i| i.viewport().close_requested()) {
                viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                close_requested = true;
                return;
            }
            egui::CentralPanel::default().show(viewport_ui, |ui| {
                let content_height = ui.available_height();
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(150.0, content_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.add_space(style::SPACE_SM);
                            let _ = ui.selectable_label(true, self.t("settings.category.general"));
                        },
                    );
                    let (divider, _) = ui.allocate_exact_size(
                        egui::vec2(1.0, content_height),
                        egui::Sense::hover(),
                    );
                    ui.painter().vline(
                        divider.center().x,
                        divider.y_range(),
                        egui::Stroke::new(
                            1.0,
                            ui.visuals().widgets.noninteractive.bg_stroke.color,
                        ),
                    );
                    ui.add_space(style::SPACE_SM);
                    ui.vertical(|ui| {
                        ui.add_space(style::SPACE_MD);
                        let single_instance_label =
                            self.t("settings.single_instance").to_owned();
                        if ui
                            .checkbox(
                                &mut self.store.settings.single_instance,
                                single_instance_label,
                            )
                            .changed()
                        {
                            self.persist();
                        }
                        ui.add_space(style::SPACE_SM);
                        ui.label(
                            egui::RichText::new(self.t("settings.single_instance_hint"))
                                .weak(),
                        );
                    });
                });
            });
        });
        self.settings_viewport_placed = !close_requested;
        if close_requested {
            self.show_settings = false;
        }
    }

    fn spawn_update_check(&self, ctx: &egui::Context) {
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let msg = match crate::core::update::check_latest() {
                Ok(manifest)
                    if crate::core::update::version_newer(
                        env!("CARGO_PKG_VERSION"),
                        &manifest.version,
                    ) =>
                {
                    WorkerMsg::UpdateFound { manifest }
                }
                Ok(_) => WorkerMsg::UpdateLatest,
                Err(e) => WorkerMsg::UpdateFailed(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn start_update_download(&mut self, ctx: &egui::Context) {
        let Some(manifest) = self.pending_update.clone() else {
            return;
        };
        self.update_busy = true;
        self.update_err = None;
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let msg = match crate::core::update::download(&manifest) {
                Ok(archive) => {
                    let stage = crate::platform::prepare_update_archive(
                        &archive,
                        manifest.size,
                        &manifest.sha256,
                    );
                    if let Err(e) = std::fs::remove_file(&archive) {
                        tracing::warn!(path = %archive.display(), "failed to remove downloaded update archive: {e}");
                    }
                    match stage {
                        Ok(stage_dir) => WorkerMsg::UpdateReady(stage_dir),
                        Err(e) => WorkerMsg::UpdateFailed(e.to_string()),
                    }
                }
                Err(e) => WorkerMsg::UpdateFailed(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub(crate) fn open_preview(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !crate::core::preview::supports_preview(&path) {
            return;
        }
        self.preview.cancel.store(true, Ordering::Relaxed);
        self.preview.generation = self.preview.generation.wrapping_add(1);
        self.preview.cancel = Arc::new(AtomicBool::new(false));
        self.preview.target = Some(path.clone());
        self.preview.text = None;
        self.preview.image = None;
        self.preview.image_dimensions = None;
        self.preview.error = None;
        self.preview.loading = true;

        let generation = self.preview.generation;
        let cancel = self.preview.cancel.clone();
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        if crate::core::preview::supports_text_preview(&path) {
            thread::spawn(move || {
                let result = if cancel.load(Ordering::Relaxed) {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "preview cancelled",
                    ))
                } else {
                    crate::core::preview::read_text_preview(&path)
                };
                let _ = tx.send(WorkerMsg::TextPreview {
                    generation,
                    path,
                    result,
                });
                ctx.request_repaint();
            });
        } else {
            thread::spawn(move || {
                let result = crate::platform::decode_image(&path, 1600);
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(WorkerMsg::ImagePreview {
                    generation,
                    path,
                    result,
                });
                ctx.request_repaint();
            });
        }
    }

    fn act_preview(&mut self, ctx: &egui::Context) {
        let tab = &self.tabs[self.active];
        let index = tab
            .selection
            .iter()
            .next()
            .copied()
            .or(if tab.entries.is_empty() {
                None
            } else {
                Some(tab.cursor)
            });
        let path = index
            .and_then(|index| tab.entries.get(index))
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.path.clone());
        if let Some(path) = path {
            self.open_preview(path, ctx);
        }
    }

    fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.preview.target.clone() else {
            return;
        };
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.display().to_string());
        let title = format!("{} — {name}", self.t("preview.title"));
        let loading_label = self.t("preview.loading").to_owned();
        let truncated_label = self.t("preview.truncated").to_owned();
        let mut open = true;
        egui::Window::new(title)
            .resizable(true)
            .default_size([760.0, 540.0])
            .open(&mut open)
            .show(ctx, |ui| {
                if self.preview.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&loading_label);
                    });
                }
                if let Some(error) = &self.preview.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(preview) = &mut self.preview.text {
                    if preview.truncated {
                        ui.label(&truncated_label);
                    }
                    ui.add(
                        egui::TextEdit::multiline(&mut preview.text)
                            .font(egui::TextStyle::Monospace)
                            .code_editor()
                            .interactive(false)
                            .desired_width(f32::INFINITY)
                            .desired_rows(28),
                    );
                }
                if let Some(texture) = &self.preview.image {
                    if let Some((width, height)) = self.preview.image_dimensions {
                        ui.label(format!("{width} × {height}"));
                    }
                    let image_size = texture.size_vec2();
                    let available = ui.available_size();
                    let scale = (available.x.max(1.0) / image_size.x)
                        .min(available.y.max(1.0) / image_size.y)
                        .min(1.0);
                    let display_size = image_size * scale;
                    ui.centered_and_justified(|ui| {
                        ui.add(egui::Image::new((texture.id(), display_size)));
                    });
                }
            });
        if !open {
            self.preview.clear();
        }
    }

    pub(crate) fn paint_thumbnail(
        &mut self,
        ui: &mut egui::Ui,
        entry: &FileEntry,
        rect: egui::Rect,
    ) -> bool {
        if !crate::core::preview::supports_image_preview(&entry.path)
            || !ui.is_rect_visible(rect)
        {
            return false;
        }
        let key = Self::thumbnail_key(entry);
        self.thumbnail_visible.insert(key.clone());

        if !self.thumbnail_cache.contains_key(&key)
            && !self.thumbnail_jobs.contains_key(&key)
            && !self.thumbnail_failed.contains(&key)
        {
            let cancel = Arc::new(AtomicBool::new(false));
            match self.thumbnail_tx.try_send(ThumbnailRequest {
                key: key.clone(),
                cancel: cancel.clone(),
            }) {
                Ok(()) => {
                    self.thumbnail_jobs.insert(key.clone(), cancel);
                }
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
            }
        }

        let Some(texture) = self.thumbnail_cache.get(&key) else {
            return false;
        };
        let source_size = texture.size_vec2();
        let scale = (rect.width() / source_size.x).min(rect.height() / source_size.y);
        let target = egui::Rect::from_center_size(rect.center(), source_size * scale);
        ui.painter().image(
            texture.id(),
            target,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        true
    }

    fn thumbnail_key(entry: &FileEntry) -> ThumbnailKey {
        let modified_nanos = entry
            .modified
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        ThumbnailKey {
            path: entry.path.clone(),
            size: entry.size.unwrap_or_default(),
            modified_nanos,
        }
    }

    fn cache_thumbnail(
        &mut self,
        ctx: &egui::Context,
        key: ThumbnailKey,
        decoded: crate::platform::DecodedImage,
    ) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [decoded.width as usize, decoded.height as usize],
            &decoded.rgba,
        );
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let texture = ctx.load_texture(
            format!("thumbnail-{:x}", hasher.finish()),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.thumbnail_cache.insert(key.clone(), texture);
        self.thumbnail_order.push_back(key);
        while self.thumbnail_cache.len() > 256 {
            if let Some(oldest) = self.thumbnail_order.pop_front() {
                self.thumbnail_cache.remove(&oldest);
            }
        }
    }

    fn cancel_invisible_thumbnails(&mut self) {
        let invisible: Vec<_> = self
            .thumbnail_jobs
            .keys()
            .filter(|key| !self.thumbnail_visible.contains(*key))
            .cloned()
            .collect();
        for key in invisible {
            if let Some(cancel) = self.thumbnail_jobs.remove(&key) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.thumbnail_failed
            .retain(|key| self.thumbnail_visible.contains(key));
    }

    pub(crate) fn begin_thumbnail_frame(&mut self) {
        self.thumbnail_visible.clear();
    }

    pub(crate) fn finish_thumbnail_frame(&mut self) {
        self.cancel_invisible_thumbnails();
    }

    pub(crate) fn cancel_all_thumbnails(&mut self) {
        for cancel in self.thumbnail_jobs.drain().map(|(_, cancel)| cancel) {
            cancel.store(true, Ordering::Relaxed);
        }
        self.thumbnail_visible.clear();
    }

    pub(crate) fn extract_zip_file(&mut self, member: String, ctx: &egui::Context) {
        let archive = self.tabs[self.active].path.clone();
        let destination = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        self.spawn_zip_extraction(
            move || crate::platform::extract_zip_file(&archive, &member, &destination),
            ctx,
        );
    }

    pub(crate) fn extract_zip_folder(&mut self, folder: String, ctx: &egui::Context) {
        let archive = self.tabs[self.active].path.clone();
        let destination = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        self.spawn_zip_extraction(
            move || crate::platform::extract_zip_folder(&archive, &folder, &destination),
            ctx,
        );
    }

    pub(crate) fn extract_zip_all(&mut self, ctx: &egui::Context) {
        let archive = self.tabs[self.active].path.clone();
        self.extract_zip_all_from(archive, ctx);
    }

    pub(crate) fn extract_zip_all_from(&mut self, archive: PathBuf, ctx: &egui::Context) {
        let destination = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        self.spawn_zip_extraction(
            move || crate::platform::extract_zip_all(&archive, &destination),
            ctx,
        );
    }

    fn spawn_zip_extraction(
        &mut self,
        extraction: impl FnOnce() -> std::io::Result<PathBuf> + Send + 'static,
        ctx: &egui::Context,
    ) {
        if self.zip_busy {
            return;
        }
        self.zip_generation = self.zip_generation.wrapping_add(1);
        self.zip_busy = true;
        self.zip_message = None;
        let generation = self.zip_generation;
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = extraction();
            let _ = tx.send(WorkerMsg::ZipExtracted {
                generation,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn zip_error_text(&self, error: &std::io::Error) -> String {
        if error.raw_os_error().is_some_and(|code| code == 32 || code == 33) {
            return self.t("zip.error_busy").to_owned();
        }
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => self.t("error.access_denied").to_owned(),
            std::io::ErrorKind::NotFound => self.t("zip.error_invalid").to_owned(),
            std::io::ErrorKind::InvalidData => {
                if error.to_string().contains("unsafe path") {
                    self.t("zip.error_path").to_owned()
                } else {
                    self.t("zip.error_invalid").to_owned()
                }
            }
            std::io::ErrorKind::InvalidInput => {
                if error.to_string().contains("filename encoding") {
                    self.t("zip.error_names").to_owned()
                } else {
                    self.t("zip.error_path").to_owned()
                }
            }
            std::io::ErrorKind::AlreadyExists => self.t("zip.error_exists").to_owned(),
            std::io::ErrorKind::StorageFull => self.t("zip.error_space").to_owned(),
            std::io::ErrorKind::WouldBlock => self.t("zip.error_busy").to_owned(),
            _ => self.t("zip.error").to_owned(),
        }
    }

    fn zip_status_window(&mut self, ctx: &egui::Context) {
        if !self.zip_busy && self.zip_message.is_none() {
            return;
        }
        let title = self.t("zip.window").to_owned();
        let extracting = self.t("zip.extracting").to_owned();
        let close_label = self.t("common.close").to_owned();
        let mut open = true;
        let mut close_requested = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                if self.zip_busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&extracting);
                    });
                } else if let Some(message) = &self.zip_message {
                    ui.label(message);
                    if ui.button(&close_label).clicked() {
                        close_requested = true;
                    }
                }
            });
        if !open || close_requested {
            self.zip_message = None;
        }
    }

    fn act_props(&mut self) {
        let tab = &self.tabs[self.active];
        if tab.archive_prefix.is_some() {
            return;
        }
        let idx = tab
            .selection
            .iter()
            .next()
            .copied()
            .or(if tab.entries.is_empty() { None } else { Some(tab.cursor) });
        if let Some(e) = idx.and_then(|i| tab.entries.get(i)) {
            self.props_target = Some(e.path.clone());
        }
    }

    fn props_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.props_target.clone() else {
            return;
        };
        let title = self.t("props.title").to_owned();
        let mut dirty = false;
        let mut apply = None;
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-properties");
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(560.0, 360.0),
            self.props_viewport_placed,
        );
        let mut close_requested = false;
        ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, _class| {
            let viewport_ctx = viewport_ui.ctx().clone();
            if viewport_ctx.input(|i| i.viewport().close_requested()) {
                viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                close_requested = true;
                return;
            }
            egui::CentralPanel::default().show(viewport_ui, |ui| {
                let entry = self
                    .tabs[self.active]
                    .entries
                    .iter()
                    .find(|e| e.path == target)
                    .cloned();
                let name = entry
                    .as_ref()
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| {
                        target
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| target.display().to_string())
                    });
                ui.heading(&name);
                ui.label(format!("{}: {}", self.t("props.path"), target.display()));
                if let Some(e) = &entry {
                    let ty = if e.is_dir {
                        self.t("type.folder").to_owned()
                    } else {
                        match &e.extension {
                            Some(x) => format!("{} {}", x.to_uppercase(), self.t("type.file")),
                            None => self.t("type.file").to_owned(),
                        }
                    };
                    ui.label(format!("{}: {ty}", self.t("col.type")));
                    if let Some(s) = e.size {
                        ui.label(format!("{}: {}", self.t("col.size"), crate::core::format::format_size(s)));
                    }
                    if let Some(m) = e.modified {
                        ui.label(format!(
                            "{}: {}",
                            self.t("col.modified"),
                            crate::platform::format_local_time(m, self.i18n.language())
                        ));
                    }
                    if let Some(c) = e.created {
                        ui.label(format!(
                            "{}: {}",
                            self.t("props.created"),
                            crate::platform::format_local_time(c, self.i18n.language())
                        ));
                    }
                    ui.separator();
                    let mut ro = e.readonly;
                    let mut hi = e.hidden;
                    if ui.checkbox(&mut ro, self.t("props.readonly").to_owned()).changed() {
                        dirty = true;
                    }
                    if ui.checkbox(&mut hi, self.t("props.hidden").to_owned()).changed() {
                        dirty = true;
                    }
                    if dirty {
                        apply = Some((ro, hi));
                    }
                }
            });
        });
        self.props_viewport_placed = !close_requested;
        if let Some((ro, hi)) = apply {
            if crate::platform::set_attributes(&target, ro, hi).is_ok() {
                self.refresh(ctx);
            }
        }
        if close_requested {
            self.props_target = None;
        }
    }

    fn update_window(&mut self, ctx: &egui::Context) {
        if self.pending_update.is_none() && self.update_err.is_none() && !self.update_busy {
            return;
        }
        let title = self.t("update.title").to_owned();
        let found = self.pending_update.clone();
        let mut open = true;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                if self.update_busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(self.t("update.downloading").to_owned());
                    });
                }
                if let Some(manifest) = &found {
                    ui.label(self.t("update.found").replace("{v}", &manifest.version));
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.update_busy, egui::Button::new(self.t("update.now").to_owned())).clicked() {
                            self.start_update_download(ctx);
                        }
                        if ui.button(self.t("update.later").to_owned()).clicked() {
                            self.pending_update = None;
                        }
                    });
                }
                if let Some(e) = &self.update_err {
                    ui.label(e.as_str());
                }
            });
        if !open {
            self.pending_update = None;
            self.update_err = None;
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

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("tab_bar")
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(
                    style::FRAME_PADDING_X,
                    0,
                )),
            )
            .show(ui, |ui| self.tab_bar(&ctx, ui));
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

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let tab = &self.tabs[self.active];
                if tab.status == ListStatus::Loading {
                    let status = if tab.search.running || tab.search.debounce_at.is_some() {
                        "search.searching"
                    } else {
                        "status.loading"
                    };
                    ui.label(self.t(status).to_owned());
                } else {
                    ui.label(format!(
                        "{}: {}",
                        self.t("status.items"),
                        tab.entries.len()
                    ));
                    if !tab.selection.is_empty() {
                        ui.separator();
                        ui.label(format!(
                            "{}: {}",
                            self.t("status.selected"),
                            tab.selection.len()
                        ));
                    }
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
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
            if let Some(err) = error {
                self.cancel_all_thumbnails();
                ui.vertical_centered(|ui| {
                    ui.add_space(style::SPACE_EMPTY_STATE);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                    ui.add_space(style::SPACE_MD);
                    if ui.button(self.t("toolbar.up").to_owned()).clicked() {
                        self.go_up(&ctx);
                    }
                });
                return;
            }
            if empty {
                self.cancel_all_thumbnails();
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
            self.file_list(&ctx, ui);
        });

        self.flush_pending_open_tabs(&ctx);
        self.about_window(&ctx);
        self.settings_window(&ctx);
        self.update_window(&ctx);
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

const TAB_DRAG_THRESHOLD: f32 = 5.0;

fn is_tab_dragging(
    active_drag: Option<usize>,
    press: Option<(usize, egui::Pos2)>,
    pointer: Option<egui::Pos2>,
) -> bool {
    active_drag.is_some()
        || matches!((press, pointer), (Some((_, start)), Some(pos)) if pos.distance(start) > TAB_DRAG_THRESHOLD)
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
    // egui normally requires Shift for vertical wheel input in horizontal-only
    // areas. The tab strip is the one place where plain wheel maps horizontally.
    style.always_scroll_the_only_direction = true;
    // The panel should hug the tab row without resizing the tabs or the plus button.
    style.spacing.item_spacing.y = super::style::TAB_BAR_VERTICAL_SPACING;
}

#[cfg(test)]
mod tests {
    use super::{
        breadcrumb_segments, configure_tab_scroll_style, dropped_paths, floating_dialog_viewport,
        is_tab_dragging, parse_archive_address, persisted_tab_path, tab_close_button_hovered,
        tab_scroll_source, tab_strip_content_margin,
    };
    use std::path::{Path, PathBuf};

    #[derive(Debug)]
    struct MockDrop(PathBuf);

    impl egui::DroppedFile for MockDrop {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Ok(vec![])
        }
    }

    #[test]
    fn dropped_paths_collects() {
        let files: Vec<egui::DroppedFileHandle> = vec![
            std::sync::Arc::new(MockDrop(PathBuf::from("C:\\a.txt"))),
            std::sync::Arc::new(MockDrop(PathBuf::from("D:\\b.txt"))),
        ];
        let got = dropped_paths(&files);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], PathBuf::from("C:\\a.txt"));
        assert_eq!(got[1], PathBuf::from("D:\\b.txt"));
    }

    #[test]
    fn breadcrumbs_split_path() {
        #[cfg(windows)]
        {
            let segs = breadcrumb_segments(Path::new("C:\\a\\b"));
            assert_eq!(
                segs.iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>(),
                vec!["C:\\", "a", "b"]
            );
            assert_eq!(segs[0].0, "C:\\");
            assert_eq!(segs[0].1, PathBuf::from("C:\\"));
            let last = segs.last().unwrap();
            assert_eq!(last.0, "b");
            assert!(last.1.ends_with("b"));
        }
    }

    #[test]
    fn archive_address_round_trips_the_internal_folder() {
        let (archive, prefix) =
            parse_archive_address(r"C:\Downloads\files.zip!/docs/notes").unwrap();
        assert_eq!(archive, PathBuf::from(r"C:\Downloads\files.zip"));
        assert_eq!(prefix, "docs/notes");
    }

    #[test]
    fn archive_tabs_restore_the_containing_filesystem_folder() {
        let mut tab = super::Tab::new(PathBuf::from(r"C:\Downloads\files.zip"));
        tab.archive_prefix = Some("docs".to_owned());
        assert_eq!(
            persisted_tab_path(&tab),
            PathBuf::from(r"C:\Downloads")
        );
    }

    #[test]
    fn tab_strip_uses_mouse_wheel_without_showing_a_scrollbar() {
        let source = tab_scroll_source();
        assert!(source.mouse_wheel);
        assert!(!source.scroll_bar);
        assert_eq!(source.drag, egui::scroll_area::DragScroll::Never);

        let mut style = egui::Style::default();
        configure_tab_scroll_style(&mut style);
        assert!(style.always_scroll_the_only_direction);
        assert_eq!(
            style.spacing.item_spacing.y,
            super::style::TAB_BAR_VERTICAL_SPACING
        );
        let margin = tab_strip_content_margin();
        assert_eq!(margin.left, super::style::FRAME_PADDING_X);
        assert_eq!(margin.right, super::style::FRAME_PADDING_X);
        assert_eq!(margin.top, 0);
        assert_eq!(margin.bottom, 0);
    }

    #[test]
    fn dragging_over_another_tabs_close_control_does_not_activate_it() {
        let close_rect =
            egui::Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(18.0, 24.0));
        let pointer = Some(close_rect.center());
        assert!(tab_close_button_hovered(false, false, close_rect, pointer));
        assert!(!tab_close_button_hovered(true, false, close_rect, pointer));
        assert!(!tab_close_button_hovered(false, true, close_rect, pointer));

        let press = Some((0, egui::pos2(10.0, 10.0)));
        assert!(!is_tab_dragging(None, press, Some(egui::pos2(15.0, 10.0))));
        assert!(is_tab_dragging(None, press, Some(egui::pos2(16.0, 10.0))));
        assert!(is_tab_dragging(Some(0), None, None));
    }

    #[test]
    fn detached_dialog_viewports_are_topmost_and_keep_user_moved_positions() {
        let ctx = egui::Context::default();
        let size = egui::vec2(480.0, 280.0);
        let initial = floating_dialog_viewport(&ctx, "About".into(), size, false);
        assert_eq!(initial.window_level, Some(egui::viewport::WindowLevel::AlwaysOnTop));
        assert_eq!(initial.inner_size, Some(size));
        assert_eq!(initial.maximize_button, Some(false));
        assert!(initial.position.is_some());

        let moved = floating_dialog_viewport(&ctx, "About".into(), size, true);
        assert_eq!(moved.window_level, Some(egui::viewport::WindowLevel::AlwaysOnTop));
        assert_eq!(moved.maximize_button, Some(false));
        assert_eq!(moved.position, None);
    }

    #[test]
    fn drive_root_is_one_absolute_breadcrumb() {
        #[cfg(windows)]
        {
            let segs = breadcrumb_segments(Path::new("C:\\"));
            assert_eq!(segs.len(), 1);
            assert_eq!(segs[0].0, "C:\\");
            assert_eq!(segs[0].1, PathBuf::from("C:\\"));
        }
    }
}

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
