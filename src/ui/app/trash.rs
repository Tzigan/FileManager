//! Список защищённой корзины и маршрутизация обычного удаления.
//! Системная Корзина Windows отсюда не подменяется.

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use crate::core::secure_trash::{self, RestoreConflict, TrashOptions, TrashRecord, TrashStatus};
use crate::core::settings::route_delete;

use super::*;

impl FileManagerApp {
    pub(crate) fn dispatch_recycle(&mut self, paths: Vec<PathBuf>) {
        let route = route_delete(
            self.store.settings.delete_mode,
            false,
            &paths,
            &self.store.settings.secure_trash_folders,
        );
        if !route.secure.is_empty() {
            self.start_secure_trash(route.secure);
        }
        if !route.windows.is_empty() {
            self.enqueue(
                "ops.deleting",
                crate::core::ops::OpKind::Recycle {
                    sources: route.windows,
                },
            );
        }
        if !route.ask.is_empty() {
            self.ops_ui.delete_ask = Some(route.ask);
        }
    }

    pub(crate) fn delete_ask_dialog(&mut self, ctx: &egui::Context) {
        let Some(paths) = self.ops_ui.delete_ask.clone() else {
            return;
        };
        let title = self.t("trash.ask_title").to_owned();
        let message = format!("{}: {}", self.t("trash.ask_msg"), paths.len());
        let windows = self.t("settings.delete_mode.windows").to_owned();
        let secure = self.t("settings.delete_mode.secure").to_owned();
        let cancel = self.t("dlg.cancel").to_owned();
        let mut choice = 0u8;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(message);
                ui.horizontal(|ui| {
                    if ui.button(windows).clicked() {
                        choice = 1;
                    }
                    if ui.button(secure).clicked() {
                        choice = 2;
                    }
                    if ui.button(cancel).clicked() {
                        choice = 3;
                    }
                });
            });
        if choice == 0 {
            return;
        }
        self.ops_ui.delete_ask = None;
        if choice == 1 {
            self.enqueue(
                "ops.deleting",
                crate::core::ops::OpKind::Recycle { sources: paths },
            );
        } else if choice == 2 {
            self.start_secure_trash(paths);
        }
    }

    pub(crate) fn open_secure_trash_tab(&mut self, ctx: &egui::Context) {
        let path = secure_trash::program_trash_root()
            .unwrap_or_else(|_| std::path::PathBuf::from(secure_trash::TRASH_DIR_NAME));
        let index = crate::ui::tabs::append_tab(&mut self.tabs, path);
        self.tabs[index].secure_trash = true;
        self.tabs[index].address_input = self.t("bookmark.secure_trash").to_owned();
        self.active = index;
        self.load_secure_trash_tab(index);
        self.request_tab_strip_scroll_to_active(ctx);
        self.persist_tabs();
    }

    pub(crate) fn load_secure_trash_tab(&mut self, tab: usize) {
        let records = secure_trash::list_trash(&self.trash_volumes());
        let mut notes = std::collections::HashMap::new();
        let entries = records
            .iter()
            .map(|record| {
                let row = secure_trash::list_row(record);
                let status = self.trash_status_label(record);
                let origin = if row.origin.is_empty() {
                    self.t("trash.unknown_origin").to_owned()
                } else {
                    row.origin.clone()
                };
                // Суффикс только для отрисовки. entry.name и extension остаются данными карточки.
                let note = format!("{}: {origin} · {status}", self.t("trash.col.origin"));
                notes.insert(row.id.clone(), note);
                let modified = (row.deleted_at > 0).then(|| {
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(row.deleted_at)
                });
                crate::core::fs::FileEntry {
                    name: row.name,
                    path: if row.stored_path.is_empty() {
                        std::path::PathBuf::from(format!("secure-trash:{}", row.id))
                    } else {
                        std::path::PathBuf::from(&row.stored_path)
                    },
                    is_dir: row.is_dir,
                    is_symlink: false,
                    hidden: false,
                    system: false,
                    readonly: false,
                    size: Some(row.size),
                    modified,
                    created: None,
                    extension: row.extension,
                    archive_member: Some(row.id),
                }
            })
            .collect::<Vec<_>>();
        {
            let tab_state = &mut self.tabs[tab];
            crate::ui::tabs::retain_existing_selection(&mut tab_state.selection, &entries);
            if tab_state
                .anchor
                .as_ref()
                .is_some_and(|key| !tab_state.selection.contains(key))
            {
                tab_state.anchor = None;
            }
            tab_state.entries = entries;
            tab_state.trash_notes = notes;
            tab_state.status = crate::ui::tabs::ListStatus::Ready;
            tab_state.error = None;
        }
        let title = self.t("bookmark.secure_trash").to_owned();
        self.tabs[tab].address_input = title;
    }

    pub(crate) fn selected_trash_ids(&self) -> Vec<String> {
        let tab = &self.tabs[self.active];
        tab.entries
            .iter()
            .filter(|entry| tab.is_selected(entry))
            .filter_map(|entry| entry.archive_member.clone())
            .collect()
    }

    fn reload_open_trash_tabs(&mut self) {
        let indexes: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.secure_trash)
            .map(|(index, _)| index)
            .collect();
        for index in indexes {
            self.load_secure_trash_tab(index);
        }
    }

    fn start_secure_trash(&mut self, paths: Vec<PathBuf>) {
        let hash_max = self.store.settings.secure_trash_hash_max_bytes;
        let allow_fallback = self.store.settings.secure_trash_allow_fallback;
        let fallback = allow_fallback.then(|| self.dirs.config_dir.clone());
        let tx = self.msg_tx.clone();
        let finished_paths = paths.clone();
        thread::spawn(move || {
            let mut errors = Vec::new();
            for path in paths {
                let options = TrashOptions {
                    hash_max_bytes: hash_max,
                    volume_id: crate::platform::volume_id(&path),
                    fallback_root: fallback.clone(),
                    allow_fallback,
                    // None: корень на диске программы. Запасная папка настроек — только если её включили.
                    storage_root: None,
                };
                if let Err(error) = secure_trash::move_to_trash(&path, &options) {
                    errors.push(format!("{}: {}", path.display(), error));
                }
            }
            let _ = tx.send(WorkerMsg::SecureTrashFinished {
                errors,
                paths: finished_paths,
            });
        });
    }

    fn trash_volumes(&self) -> Vec<PathBuf> {
        let mut volumes = crate::platform::drives();
        volumes.push(self.dirs.config_dir.clone());
        volumes
    }

    pub(super) fn secure_trash_window(&mut self, ctx: &egui::Context) {
        if !self.show_secure_trash {
            return;
        }
        let title = self.t("trash.title").to_owned();
        let mut open = true;
        let volumes = self.trash_volumes();
        let items = secure_trash::list_trash(&volumes);
        let mut restore_ids = None;
        let mut purge_ids = None;
        let mut empty = false;
        egui::Window::new(title)
            .open(&mut open)
            .default_size([760.0, 420.0])
            .show(ctx, |ui| {
                if let Some(notice) = &self.trash_notice {
                    ui.label(notice);
                }
                ui.horizontal(|ui| {
                    if ui.button(self.t("trash.restore")).clicked() {
                        let ids: Vec<_> = self.trash_selected.iter().cloned().collect();
                        if !ids.is_empty() {
                            restore_ids = Some(ids);
                        }
                    }
                    if ui.button(self.t("trash.purge")).clicked() {
                        purge_ids = Some(self.trash_selected.iter().cloned().collect());
                    }
                    if ui.button(self.t("trash.empty")).clicked() {
                        empty = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for item in &items {
                        let status = self.trash_status_label(item);
                        let checked = self.trash_selected.contains(&item.id);
                        ui.horizontal(|ui| {
                            let mut selected = checked;
                            if ui.checkbox(&mut selected, "").changed() {
                                if selected {
                                    self.trash_selected.insert(item.id.clone());
                                } else {
                                    self.trash_selected.remove(&item.id);
                                }
                            }
                            if ui.button(self.t("trash.restore")).clicked() {
                                restore_ids = Some(vec![item.id.clone()]);
                            }
                            if ui.button(self.t("trash.purge")).clicked() {
                                purge_ids = Some(vec![item.id.clone()]);
                            }
                            ui.label(if item.original_name.is_empty() {
                                self.t("trash.damaged_name").to_owned()
                            } else {
                                item.original_name.clone()
                            });
                            ui.label(self.t(if item.is_dir {
                                "trash.folder"
                            } else {
                                "trash.file"
                            }));
                            ui.label(crate::core::format::format_size(item.size));
                            ui.label(self.trash_time(item.deleted_at));
                            ui.label(if item.original_path.is_empty() {
                                self.t("trash.unknown_origin").to_owned()
                            } else {
                                item.original_path.clone()
                            });
                            ui.label(status);
                        });
                    }
                    if items.is_empty() {
                        ui.label(self.t("trash.empty_list"));
                    }
                });
            });
        self.show_secure_trash = open;
        if let Some(ids) = restore_ids {
            self.restore_trash_ids(ids, None, ctx);
        }
        if let Some(ids) = purge_ids.filter(|ids| !ids.is_empty()) {
            self.trash_purge_ids = Some(ids);
        }
        if empty {
            self.trash_empty_confirm = true;
        }
    }

    pub(super) fn trash_confirm_windows(&mut self, ctx: &egui::Context) {
        let items = secure_trash::list_trash(&self.trash_volumes());
        if let Some(ids) = self.trash_purge_ids.clone() {
            let chosen: Vec<_> = items
                .iter()
                .filter(|item| ids.iter().any(|id| id == &item.id))
                .cloned()
                .collect();
            let summary = secure_trash::removal_facts(&chosen)
                .into_iter()
                .map(|(name, is_dir, size, origin)| {
                    format!(
                        "{name} ({}, {}) — {origin}",
                        self.t(if is_dir { "trash.folder" } else { "trash.file" }),
                        crate::core::format::format_size(size)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut decision = 0u8;
            egui::Window::new(self.t("trash.purge_title").to_owned())
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(self.t("trash.purge_msg"));
                    ui.label(summary);
                    ui.horizontal(|ui| {
                        if ui.button(self.t("trash.purge")).clicked() {
                            decision = 1;
                        }
                        if ui.button(self.t("dlg.cancel")).clicked() {
                            decision = 2;
                        }
                    });
                });
            if decision != 0 {
                self.trash_purge_ids = None;
            }
            if decision == 1 {
                let mut errors = Vec::new();
                for item in chosen {
                    if let Err(error) = secure_trash::purge(&item) {
                        errors.push(format!("{}: {error}", item.original_name));
                    }
                }
                self.trash_notice = Some(if errors.is_empty() {
                    self.t("trash.purged").to_owned()
                } else {
                    errors.join("\n")
                });
                self.reload_open_trash_tabs();
            }
        }
        if self.trash_empty_confirm {
            let (count, bytes) = secure_trash::removal_totals(&items);
            let mut decision = 0u8;
            egui::Window::new(self.t("trash.empty_title").to_owned())
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "{}: {count}, {}",
                        self.t("trash.empty_msg"),
                        crate::core::format::format_size(bytes)
                    ));
                    ui.horizontal(|ui| {
                        if ui.button(self.t("trash.empty")).clicked() {
                            decision = 1;
                        }
                        if ui.button(self.t("dlg.cancel")).clicked() {
                            decision = 2;
                        }
                    });
                });
            if decision != 0 {
                self.trash_empty_confirm = false;
            }
            if decision == 1 {
                let errors = secure_trash::empty_trash(&self.trash_volumes());
                self.trash_notice = Some(if errors.is_empty() {
                    self.t("trash.emptied").to_owned()
                } else {
                    errors.join("\n")
                });
                self.reload_open_trash_tabs();
            }
        }
        if let Some(id) = self.trash_conflict.clone() {
            let mut decision = None;
            egui::Window::new(self.t("trash.conflict_title").to_owned())
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(self.t("trash.conflict_msg"));
                    ui.horizontal(|ui| {
                        if ui.button(self.t("trash.conflict_rename")).clicked() {
                            decision = Some(RestoreConflict::NewName);
                        }
                        if ui.button(self.t("trash.conflict_overwrite")).clicked() {
                            decision = Some(RestoreConflict::Overwrite);
                        }
                        if ui.button(self.t("trash.conflict_skip")).clicked() {
                            decision = Some(RestoreConflict::Skip);
                        }
                        if ui.button(self.t("dlg.cancel")).clicked() {
                            decision = Some(RestoreConflict::Cancel);
                        }
                    });
                });
            if let Some(choice) = decision {
                self.trash_conflict = None;
                if !matches!(choice, RestoreConflict::Skip | RestoreConflict::Cancel) {
                    self.restore_trash_ids(vec![id], Some(choice), ctx);
                }
            }
        }
    }

    pub(crate) fn restore_trash_ids(
        &mut self,
        ids: Vec<String>,
        conflict: Option<RestoreConflict>,
        ctx: &egui::Context,
    ) {
        let items = secure_trash::list_trash(&self.trash_volumes());
        for id in ids {
            let Some(record) = items.iter().find(|item| item.id == id).cloned() else {
                continue;
            };
            if record.status == TrashStatus::Damaged || record.original_path.is_empty() {
                self.trash_notice = Some(self.t("trash.err.damaged").to_owned());
                continue;
            }
            let current = crate::platform::volume_id(Path::new(&record.original_path));
            let roots = secure_trash::allowed_restore_roots(Path::new(&record.original_path));
            match secure_trash::restore(&record, current.as_deref(), &roots, conflict) {
                Ok(path) => {
                    // Восстановленный файл лежит в обычной папке. Корзина по-прежнему читается отдельно.
                    self.dir_cache.invalidate_dir(&path);
                    let mut notice = format!("{}: {}", self.t("trash.restored"), path.display());
                    if let Some(key) = secure_trash::integrity_status_key(&record) {
                        notice = format!("{notice}. {}", self.t(key));
                    }
                    self.trash_notice = Some(notice);
                    self.reload_open_trash_tabs();
                    let active = self.active;
                    if !self.tabs[active].secure_trash {
                        self.navigate_fresh(active, ctx);
                    }
                }
                Err(secure_trash::TrashError::Conflict) => {
                    self.trash_conflict = Some(id);
                    break;
                }
                Err(error) => {
                    self.trash_notice =
                        Some(format!("{} ({})", self.t(error.message_key()), error));
                }
            }
        }
    }

    fn trash_status_label(&self, record: &TrashRecord) -> String {
        if record.status == TrashStatus::Damaged || record.original_path.is_empty() {
            return self.t("trash.status.damaged").to_owned();
        }
        let current = crate::platform::volume_id(Path::new(&record.original_path));
        if matches!((&record.volume_id, &current), (Some(stored), Some(now)) if stored != now)
            || (record.volume_id.is_some() && current.is_none())
        {
            return self.t("trash.status.volume").to_owned();
        }
        let base = if record.volume_id.is_none() {
            self.t("trash.status.unidentified")
        } else {
            self.t("trash.status.ok")
        };
        match secure_trash::integrity_status_key(record) {
            Some(key) => format!("{base}. {}", self.t(key)),
            None => base.to_owned(),
        }
    }

    fn trash_time(&self, secs: u64) -> String {
        if secs == 0 {
            return "—".to_owned();
        }
        crate::platform::format_local_time(
            UNIX_EPOCH + Duration::from_secs(secs),
            self.i18n.language(),
        )
    }
}

// Режим по умолчанию не должен уводить удаление в защищённую корзину.
#[cfg(test)]
mod tests {
    #[test]
    fn default_delete_mode_stays_on_windows_recycle() {
        assert_eq!(
            crate::core::settings::DeleteMode::default(),
            crate::core::settings::DeleteMode::WindowsRecycle
        );
    }
}
