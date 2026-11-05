use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use crate::core::fs::{self, FileEntry, ReadDirError};
use crate::core::i18n::{I18n, Language};
use crate::core::ops::{self, OpEvent, OpJob, OpReply};
use crate::core::paths::AppDirs;
use crate::core::settings::{SettingsStore, Theme};
use crate::platform::{drives, install_context_menu, pipe_listen, uninstall_context_menu};
use crate::ui::opsui::OpsUi;
use crate::ui::tabs::{ListStatus, Tab};
use crate::ui::tree::TreeState;

/// Results produced by background worker threads.
pub(crate) enum WorkerMsg {
    /// A directory listing finished for a tab.
    Listing {
        tab: usize,
        generation: u64,
        result: Result<Vec<FileEntry>, ReadDirError>,
    },
    /// A folder tree node's children finished loading.
    TreeChildren {
        path: PathBuf,
        result: std::io::Result<Vec<PathBuf>>,
    },
    /// A path forwarded by another instance / CLI — open in a new tab.
    OpenPath(PathBuf),
}

pub struct FileManagerApp {
    pub(crate) store: SettingsStore,
    pub(crate) i18n: I18n,
    dirs: AppDirs,
    show_about: bool,

    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) tree: TreeState,
    /// Scroll the focused row into view next repaint (set by keyboard nav).
    pub(crate) scroll_to_cursor: bool,
    /// `--select` target: select this entry when the listing arrives.
    pending_select: Option<String>,

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

        let mut app = Self {
            store,
            i18n,
            dirs,
            show_about: false,
            tabs: paths.into_iter().map(Tab::new).collect(),
            active,
            tree: TreeState::new(drives()),
            scroll_to_cursor: false,
            pending_select: initial_select,
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
        for i in 0..app.tabs.len() {
            app.navigate_fresh(i, &cc.egui_ctx);
        }
        app.tree.reveal(&app.tabs[app.active].path.clone());
        app
    }

    /// Open `path` in a new tab (pipe-forwarded from another instance).
    fn open_path_tab(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.tabs.push(Tab::new(path));
        self.active = self.tabs.len() - 1;
        self.navigate_fresh(self.active, ctx);
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
        self.store.settings.tabs = self.tabs.iter().map(|t| t.path.clone()).collect();
        self.store.settings.active_tab = self.active;
        self.persist();
    }

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    /// User-driven navigation to a new path (pushes history).
    pub(crate) fn navigate(&mut self, tab: usize, path: PathBuf, ctx: &egui::Context) {
        let current = self.tabs[tab].path.clone();
        if current != path {
            self.tabs[tab].history.push(current);
            self.tabs[tab].path = path;
        }
        self.navigate_fresh(tab, ctx);
    }

    /// Reload `tab`'s current path in the background without touching history.
    pub(crate) fn navigate_fresh(&mut self, tab: usize, ctx: &egui::Context) {
        let t = &mut self.tabs[tab];
        t.generation += 1;
        let generation = t.generation;
        t.cancel.store(true, Ordering::Relaxed);
        t.cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel = t.cancel.clone();

        t.status = ListStatus::Loading;
        t.error = None;
        t.clear_selection();
        t.cursor = 0;
        let path = t.path.clone();
        t.address_input = path.display().to_string();

        if tab == self.active {
            self.tree.reveal(&path);
            self.persist_tabs();
        }

        let tx = self.msg_tx.clone();
        let ctx2 = ctx.clone();
        thread::spawn(move || {
            let result = fs::read_dir(&path, &cancel);
            let _ = tx.send(WorkerMsg::Listing { tab, generation, result });
            ctx2.request_repaint();
        });
    }

    fn go_back(&mut self, ctx: &egui::Context) {
        let cur = self.tabs[self.active].path.clone();
        if let Some(path) = self.tabs[self.active].history.back(cur) {
            self.tabs[self.active].path = path;
            self.navigate_fresh(self.active, ctx);
        }
    }

    fn go_forward(&mut self, ctx: &egui::Context) {
        let cur = self.tabs[self.active].path.clone();
        if let Some(path) = self.tabs[self.active].history.forward(cur) {
            self.tabs[self.active].path = path;
            self.navigate_fresh(self.active, ctx);
        }
    }

    fn go_up(&mut self, ctx: &egui::Context) {
        if let Some(parent) = self.tabs[self.active].path.parent().map(|p| p.to_path_buf()) {
            self.navigate(self.active, parent, ctx);
        }
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.navigate_fresh(self.active, ctx);
    }

    fn cancel_loading(&mut self) {
        let t = &mut self.tabs[self.active];
        t.cancel.store(true, Ordering::Relaxed);
        t.status = ListStatus::Ready;
    }

    // ------------------------------------------------------------------
    // Tabs
    // ------------------------------------------------------------------

    fn new_tab(&mut self, ctx: &egui::Context) {
        let path = self.tabs[self.active].path.clone();
        self.tabs.push(Tab::new(path));
        self.active = self.tabs.len() - 1;
        self.navigate_fresh(self.active, ctx);
    }

    fn close_tab(&mut self, idx: usize, ctx: &egui::Context) {
        if self.tabs.len() <= 1 {
            return;
        }
        self.tabs[idx].cancel.store(true, Ordering::Relaxed);
        self.tabs.remove(idx);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        self.persist_tabs();
        ctx.request_repaint();
    }

    fn switch_tab(&mut self, idx: usize) {
        if idx != self.active && idx < self.tabs.len() {
            self.active = idx;
            self.persist_tabs();
        }
    }

    // ------------------------------------------------------------------
    // Messages and input
    // ------------------------------------------------------------------

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                WorkerMsg::Listing { tab, generation, result } => {
                    if self.tabs[tab].generation != generation {
                        continue;
                    }
                    match result {
                        Ok(entries) => {
                            let t = &mut self.tabs[tab];
                            t.entries = entries;
                            t.status = ListStatus::Ready;
                            if let Some(name) = self.pending_select.take() {
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
                            let text = self.error_text(&e);
                            let t = &mut self.tabs[tab];
                            t.entries.clear();
                            t.status = ListStatus::Ready;
                            t.error = Some(text);
                        }
                    }
                }
                WorkerMsg::TreeChildren { path, result } => {
                    self.tree.pending.remove(&path);
                    if let Ok(kids) = result {
                        self.tree.children.insert(path, kids);
                    } else {
                        self.tree.children.insert(path, Vec::new());
                    }
                }
                WorkerMsg::OpenPath(path) => self.open_path_tab(path, ctx),
            }
        }
    }

    fn error_text(&self, e: &std::io::Error) -> String {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => self.t("error.access_denied").to_owned(),
            std::io::ErrorKind::NotFound => self.t("error.not_found").to_owned(),
            _ => format!("{}: {}", self.t("error.open_dir"), e),
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let input = |i: &egui::InputState| i.clone();
        let mods = ctx.input(|i| i.modifiers);

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

        // Clipboard / file operations.
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::C)) {
            self.act_copy();
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::X)) {
            self.act_cut();
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::V)) {
            self.act_paste();
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.act_delete(mods.shift);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.act_rename();
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
                        .filter(|e| e.is_dir)
                        .map(|e| e.path.clone())
                } else {
                    None
                }
            };
            if let Some(dir) = target {
                self.navigate(self.active, dir, ctx);
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
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect()
        });
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
        if len == 0 {
            return;
        }
        let target = target.min(len - 1);
        if extend {
            let anchor = *tab.anchor.get_or_insert(tab.cursor);
            let (lo, hi) = if anchor <= target {
                (anchor, target)
            } else {
                (target, anchor)
            };
            tab.selection.extend(lo..=hi);
        } else {
            tab.selection.clear();
            tab.selection.insert(target);
            tab.anchor = Some(target);
        }
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
                        let selected = self.store.settings.theme == theme;
                        if ui.selectable_label(selected, self.t(key).to_owned()).clicked() {
                            self.set_theme(theme);
                            ui.close();
                        }
                    }
                });
                ui.menu_button(self.t("view.language").to_owned(), |ui| {
                    for lang in [Language::Ru, Language::En] {
                        let selected = self.i18n.language() == lang;
                        if ui.selectable_label(selected, lang.label()).clicked() {
                            self.set_language(lang);
                            ui.close();
                        }
                    }
                });
            });

            ui.menu_button(self.t("menu.help").to_owned(), |ui| {
                if ui.button(self.t("help.about").to_owned()).clicked() {
                    self.show_about = true;
                    ui.close();
                }
            });
        });
    }

    fn tab_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let mut close = None;
            let mut switch = None;
            for (i, tab) in self.tabs.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let resp = ui.selectable_label(i == self.active, tab.title());
                    if resp.clicked() {
                        switch = Some(i);
                    }
                    if resp.middle_clicked() {
                        close = Some(i);
                    }
                    if ui.small_button("×").clicked() {
                        close = Some(i);
                    }
                });
            }
            if ui.small_button("+").clicked() {
                self.new_tab(ctx);
            }
            if let Some(i) = switch {
                self.switch_tab(i);
            }
            if let Some(i) = close {
                self.close_tab(i, ctx);
            }
        });
    }

    fn toolbar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (can_back, can_fwd, has_parent, loading) = {
                let t = &self.tabs[self.active];
                (
                    t.history.can_back(),
                    t.history.can_forward(),
                    t.path.parent().is_some(),
                    t.status == ListStatus::Loading,
                )
            };
            if ui
                .add_enabled(can_back, egui::Button::new("←"))
                .on_hover_text(self.t("toolbar.back").to_owned())
                .clicked()
            {
                self.go_back(ctx);
            }
            if ui
                .add_enabled(can_fwd, egui::Button::new("→"))
                .on_hover_text(self.t("toolbar.forward").to_owned())
                .clicked()
            {
                self.go_forward(ctx);
            }
            if ui
                .add_enabled(has_parent, egui::Button::new("↑"))
                .on_hover_text(self.t("toolbar.up").to_owned())
                .clicked()
            {
                self.go_up(ctx);
            }
            if ui
                .button("⟳")
                .on_hover_text(self.t("toolbar.refresh").to_owned())
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
            let marked = self
                .store
                .settings
                .bookmarks
                .contains(&self.tabs[self.active].path);
            if ui
                .button(if marked { "★" } else { "☆" })
                .on_hover_text(self.t("toolbar.bookmark").to_owned())
                .clicked()
            {
                self.act_bookmark();
            }

            // "New / Paste" menu.
            ui.menu_button("＋", |ui| {
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
            });

            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.tabs[self.active].address_input)
                    .desired_width(f32::INFINITY),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let path = PathBuf::from(self.tabs[self.active].address_input.trim());
                self.navigate(self.active, path, ctx);
            }
        });

        // Breadcrumbs row.
        let path = self.tabs[self.active].path.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (label, seg_path) in breadcrumb_segments(&path) {
                if ui.selectable_label(false, label).clicked() {
                    self.navigate(self.active, seg_path, ctx);
                }
                ui.label("›");
            }
        });
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let title = self.t("about.title").to_owned();
        let config_label = self.t("label.config_dir").to_owned();
        let log_label = self.t("label.log_dir").to_owned();
        let mut open = self.show_about;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.heading(format!("FileManager v{}", env!("CARGO_PKG_VERSION")));
                ui.label("Rust + egui/eframe");
                ui.add_space(8.0);
                ui.label(format!("{}: {}", config_label, self.dirs.config_dir.display()));
                ui.label(format!("{}: {}", log_label, self.dirs.log_dir.display()));
            });
        self.show_about = open;
    }
}

/// Splits a path into clickable breadcrumb segments:
/// `C:\a\b` → `[("C:\", C:\), ("a", C:\a), ("b", C:\a\b)]`.
fn breadcrumb_segments(path: &Path) -> Vec<(String, PathBuf)> {
    let mut segs = Vec::new();
    let mut acc = PathBuf::new();
    for comp in path.components() {
        acc.push(comp.as_os_str());
        let mut label = comp.as_os_str().to_string_lossy().into_owned();
        // Show the drive root as "C:\" rather than bare "C:".
        if segs.is_empty() && label.ends_with(':') {
            label.push('\\');
        }
        segs.push((label, acc.clone()));
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

        self.drain_messages(&ctx);
        self.drain_ops_events(&ctx);
        self.handle_keys(&ctx);
        self.handle_dropped_files(&ctx);

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("tab_bar").show(ui, |ui| self.tab_bar(&ctx, ui));
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(&ctx, ui));

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
                    ui.label(self.t("status.loading").to_owned());
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
            let (error, empty, loading) = {
                let tab = &self.tabs[self.active];
                (
                    tab.error.clone(),
                    tab.entries.is_empty(),
                    tab.status == ListStatus::Loading,
                )
            };
            if let Some(err) = error {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                    ui.add_space(12.0);
                    if ui.button(self.t("toolbar.up").to_owned()).clicked() {
                        self.go_up(&ctx);
                    }
                });
                return;
            }
            if empty {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    if loading {
                        ui.spinner();
                        ui.label(self.t("status.loading").to_owned());
                    } else {
                        ui.label(self.t("folder.empty").to_owned());
                    }
                });
                return;
            }
            self.file_list(&ctx, ui);
        });

        self.about_window(&ctx);
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


#[cfg(test)]
mod tests {
    use super::breadcrumb_segments;
    use std::path::Path;

    #[test]
    fn breadcrumbs_split_path() {
        #[cfg(windows)]
        {
            let segs = breadcrumb_segments(Path::new("C:\\a\\b"));
            assert!(segs.len() >= 3);
            assert_eq!(segs[0].0, "C:\\");
            let last = segs.last().unwrap();
            assert_eq!(last.0, "b");
            assert!(last.1.ends_with("b"));
        }
    }
}
