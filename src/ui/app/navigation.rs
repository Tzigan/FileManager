//! Навигация по папкам, вкладкам и ZIP-путям.

use super::*;

/// Неактивная вкладка корзины перечитывает карточки, когда на неё переключаются.
pub(crate) fn should_reload_trash_on_switch(switching_away: bool, target_is_trash: bool) -> bool {
    switching_away && target_is_trash
}

impl FileManagerApp {
    /// Open `path` in a new tab (pipe-forwarded from another instance).
    pub(crate) fn open_path_in_new_tab(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.active = crate::ui::tabs::append_tab(&mut self.tabs, path);
        self.navigate_fresh(self.active, ctx);
        self.request_tab_strip_scroll_to_active(ctx);
    }

    pub(crate) fn queue_open_path_in_new_tab(&mut self, path: PathBuf) {
        self.pending_open_tabs.push(path);
    }

    pub(super) fn flush_pending_open_tabs(&mut self, ctx: &egui::Context) {
        for path in std::mem::take(&mut self.pending_open_tabs) {
            self.open_path_in_new_tab(path, ctx);
        }
    }

    pub(super) fn request_tab_strip_scroll_to_active(&mut self, ctx: &egui::Context) {
        // Прокрутка выполнится после расчёта фактического прямоугольника вкладки.
        self.tab_strip_scroll_target = self.tabs.get(self.active).map(|tab| tab.id);
        if self.tab_strip_scroll_target.is_some() {
            ctx.request_repaint();
        }
    }

    pub(super) fn persist_tabs(&mut self) {
        self.store.settings.tabs = self
            .tabs
            .iter()
            .filter(|tab| !tab.secure_trash)
            .map(persisted_tab_path)
            .collect();
        self.store.settings.pinned_tabs = self
            .tabs
            .iter()
            .filter(|tab| tab.pinned && !tab.secure_trash)
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
        self.tabs[tab].secure_trash = false;
        self.tabs[tab].trash_notes.clear();
        self.tabs[tab].archive_prefix = None;
        self.tabs[tab].archive_back.clear();
        self.tabs[tab].archive_forward.clear();
        self.tabs[tab].search.reset();
        self.navigate_fresh(tab, ctx);
    }

    fn open_archive(&mut self, tab: usize, archive: PathBuf, ctx: &egui::Context) {
        self.open_archive_at(tab, archive, String::new(), ctx);
    }

    pub(super) fn open_archive_at(
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
        self.reload_listing(tab, ctx, true);
    }

    /// `use_cache` выключен у кнопки «Обновить»: список читается с диска и заменяет запись.
    fn reload_listing(&mut self, tab: usize, ctx: &egui::Context, use_cache: bool) {
        if self.tabs[tab].secure_trash {
            // Карточки, не readdir папки хранения.
            self.load_secure_trash_tab(tab);
            return;
        }
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
        let show_hidden = self.store.settings.show_hidden_files;
        let sort_key = t.sort_key;
        let sort_desc = t.sort_desc;
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

        let cacheable = crate::core::dir_cache::uses_dir_cache(
            false,
            archive_prefix.is_some(),
            crate::platform::is_wsl_path(&path),
        );
        self.dir_cache
            .set_limit(self.store.settings.dir_cache_entries);
        if use_cache && cacheable {
            // Нет канонического пути или тома — это промах, а не старая запись.
            if let Some(entries) = self.dir_cache.lookup(&path) {
                self.finish_listing(tab, entries, show_hidden, sort_key, sort_desc);
                return;
            }
        }
        let cache_key = cacheable
            .then(|| crate::core::dir_cache::DirCache::cache_key(&path))
            .flatten();
        let cache_generation = cache_key
            .as_ref()
            .map(|key| self.dir_cache.begin(key))
            .unwrap_or(0);

        let tx = self.msg_tx.clone();
        let ctx2 = ctx.clone();
        tracing::info!(tab_id, generation, "folder listing queued");
        thread::spawn(move || {
            tracing::info!(tab_id, generation, "folder listing started");
            let result = match archive_prefix {
                Some(prefix) => crate::platform::read_zip_dir_cancellable(&path, &prefix, &cancel),
                None if crate::platform::is_wsl_path(&path) => {
                    crate::platform::read_wsl_dir(&path, &cancel)
                }
                None => fs::read_dir(&path, &cancel),
            };
            match &result {
                Ok(entries) => tracing::info!(
                    tab_id,
                    generation,
                    count = entries.len(),
                    "folder listing completed"
                ),
                Err(ReadDirError::Cancelled) => {
                    tracing::info!(tab_id, generation, "folder listing cancelled")
                }
                Err(ReadDirError::Io(error)) => {
                    tracing::warn!(tab_id, generation, error = %error, "folder listing failed")
                }
            }
            let _ = tx.send(WorkerMsg::Listing {
                tab_id,
                generation,
                result,
                cache_key,
                cache_generation,
            });
            ctx2.request_repaint();
        });
    }

    pub(super) fn go_back(&mut self, ctx: &egui::Context) {
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

    pub(super) fn go_forward(&mut self, ctx: &egui::Context) {
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

    pub(super) fn go_up(&mut self, ctx: &egui::Context) {
        if let Some(prefix) = self.tabs[self.active].archive_prefix.clone() {
            if !prefix.is_empty() {
                let parent = prefix.rsplit_once('/').map_or("", |(parent, _)| parent);
                self.navigate_zip(self.active, parent.to_owned(), ctx);
            } else if let Some(parent) = self.tabs[self.active].path.parent().map(Path::to_path_buf)
            {
                self.tabs[self.active].archive_prefix = None;
                self.tabs[self.active].archive_back.clear();
                self.tabs[self.active].archive_forward.clear();
                self.navigate(self.active, parent, ctx);
            }
            return;
        }
        if let Some(parent) = self.tabs[self.active]
            .path
            .parent()
            .map(|p| p.to_path_buf())
        {
            self.navigate(self.active, parent, ctx);
        }
    }

    pub(crate) fn refresh(&mut self, ctx: &egui::Context) {
        self.refresh_wsl_distributions();
        if let Some(path) = self.tabs.get(self.active).map(|tab| tab.path.clone()) {
            self.dir_cache.invalidate_dir(&path);
        }
        self.reload_listing(self.active, ctx, false);
    }

    fn finish_listing(
        &mut self,
        tab: usize,
        entries: Vec<FileEntry>,
        show_hidden: bool,
        sort_key: crate::core::fs::SortKey,
        sort_desc: bool,
    ) {
        let mut entries = fs::filter_hidden(entries, show_hidden);
        fs::sort_entries_by(&mut entries, sort_key, sort_desc);
        let state = &mut self.tabs[tab];
        state.entries = entries;
        state.status = ListStatus::Ready;
    }

    pub(super) fn cancel_loading(&mut self) {
        let t = &mut self.tabs[self.active];
        t.cancel.store(true, Ordering::Relaxed);
        t.search.cancel.store(true, Ordering::Relaxed);
        t.search.running = false;
        t.search.debounce_at = None;
        t.status = ListStatus::Ready;
    }

    // ------------------------------------------------------------------
    // Tabs
    // ------------------------------------------------------------------

    pub(super) fn new_tab(&mut self, ctx: &egui::Context) {
        let source = &self.tabs[self.active];
        let path = source.path.clone();
        let archive_prefix = source.archive_prefix.clone();
        self.active = crate::ui::tabs::append_tab(&mut self.tabs, path);
        self.tabs[self.active].archive_prefix = archive_prefix;
        self.navigate_fresh(self.active, ctx);
        self.request_tab_strip_scroll_to_active(ctx);
    }

    pub(super) fn close_tab(&mut self, idx: usize, ctx: &egui::Context) {
        if self.visible_tab_indices().len() <= 1
            || self
                .tabs
                .get(idx)
                .is_none_or(|tab| self.is_tab_detached(tab.id))
        {
            return;
        }
        let Some(active) = crate::ui::tabs::close_tab_state(&mut self.tabs, self.active, idx)
        else {
            return;
        };
        self.active = active;
        self.persist_tabs();
        ctx.request_repaint();
    }

    pub(super) fn close_detached_tab(&mut self, tab_id: u64, ctx: &egui::Context) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        if self.tabs[index].pinned {
            return;
        }
        self.detached_tabs.remove(&tab_id);
        self.active = index;
        self.close_tab(index, ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    pub(super) fn switch_tab(&mut self, idx: usize) {
        if idx != self.active && idx < self.tabs.len() && !self.is_tab_detached(self.tabs[idx].id) {
            self.active = idx;
            self.persist_tabs();
            // Неактивная вкладка корзины могла устареть, пока действие шло в другой.
            if should_reload_trash_on_switch(true, self.tabs[idx].secure_trash) {
                self.load_secure_trash_tab(idx);
            }
        }
    }

    pub(super) fn duplicate_tab(&mut self, idx: usize, ctx: &egui::Context) {
        let path = self.tabs[idx].path.clone();
        let archive_prefix = self.tabs[idx].archive_prefix.clone();
        let mut tab = Tab::new(path);
        tab.archive_prefix = archive_prefix;
        self.tabs.insert(idx + 1, tab);
        self.active = idx + 1;
        self.navigate_fresh(self.active, ctx);
        self.request_tab_strip_scroll_to_active(ctx);
    }

    pub(super) fn close_others(&mut self, idx: usize, ctx: &egui::Context) {
        let Some(keep_id) = self.tabs.get(idx).map(|tab| tab.id) else {
            return;
        };
        if self.visible_tab_indices().len() <= 1 || self.is_tab_detached(keep_id) {
            return;
        }
        if self.detached_tabs.is_empty() {
            let Some(active) = crate::ui::tabs::close_other_tabs_state(&mut self.tabs, idx) else {
                return;
            };
            self.active = active;
            self.persist_tabs();
            ctx.request_repaint();
            return;
        }
        let mut index = self.tabs.len();
        while index > 0 {
            index -= 1;
            let tab = &self.tabs[index];
            if tab.id != keep_id && !tab.pinned && !self.is_tab_detached(tab.id) {
                tab.cancel.store(true, Ordering::Relaxed);
                tab.search.cancel.store(true, Ordering::Relaxed);
                self.tabs.remove(index);
            }
        }
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == keep_id)
            .unwrap_or_default();
        self.persist_tabs();
        ctx.request_repaint();
    }

    pub(super) fn toggle_pin(&mut self, idx: usize) {
        self.tabs[idx].pinned = !self.tabs[idx].pinned;
        self.persist_tabs();
    }

    pub(super) fn move_tab(&mut self, src: usize, dst: usize) {
        let Some(dst) = crate::ui::tabs::move_to_drop_slot(&mut self.tabs, src, dst) else {
            return;
        };
        self.active = dst;
        self.persist_tabs();
    }

    pub(super) fn move_visible_tab(&mut self, source: usize, drop_slot: usize) {
        if self.detached_tabs.is_empty() {
            self.move_tab(source, drop_slot);
            return;
        }
        let visible = self.visible_tab_indices();
        let Some(source_id) = self.tabs.get(source).map(|tab| tab.id) else {
            return;
        };
        let Some(source_slot) = visible.iter().position(|&index| index == source) else {
            return;
        };
        if crate::ui::tabs::move_to_visible_drop_slot(
            &mut self.tabs,
            &visible,
            source_slot,
            drop_slot,
        )
        .is_none()
        {
            return;
        }
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == source_id)
            .unwrap_or_default();
        self.persist_tabs();
    }
}

#[cfg(test)]
mod tests {
    use super::should_reload_trash_on_switch;

    #[test]
    fn inactive_trash_tab_is_reread_when_it_becomes_active() {
        assert!(should_reload_trash_on_switch(true, true));
        assert!(!should_reload_trash_on_switch(false, true));
        assert!(!should_reload_trash_on_switch(true, false));
    }
}
