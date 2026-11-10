//! Выбор элементов и контекстные действия.

use super::*;

impl FileManagerApp {
    /// Empty space below the rows: left-click clears the selection, right-click
    /// shows the folder menu (Explorer semantics).
    pub(super) fn empty_area_menu(&mut self, resp: &egui::Response, ctx: &egui::Context) {
        if resp.clicked() {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            let tab = &mut self.tabs[self.active];
            tab.selection.clear();
            tab.anchor = None;
        }
        let in_archive = self.tabs[self.active].archive_prefix.is_some();
        resp.context_menu(|ui| {
            style::compact_context_menu(ui);
            if in_archive {
                if ui.button(self.t("zip.extract_all").to_owned()).clicked() {
                    self.extract_zip_all(ctx);
                    ui.close();
                }
                if ui.button(self.t("toolbar.refresh").to_owned()).clicked() {
                    self.refresh(ctx);
                    ui.close();
                }
                return;
            }
            if ui
                .add_enabled(
                    self.ops_ui.clipboard.is_some(),
                    egui::Button::new(self.t("ctx.paste").to_owned()),
                )
                .clicked()
            {
                self.act_paste();
                ui.close();
            }
            ui.separator();
            if ui.button(self.t("ctx.new_folder").to_owned()).clicked() {
                self.ops_ui.create = Some((true, String::new()));
                ui.close();
            }
            if ui.button(self.t("ctx.new_file").to_owned()).clicked() {
                self.ops_ui.create = Some((false, String::new()));
                ui.close();
            }
            if ui.button(self.t("ctx.open_terminal").to_owned()).clicked() {
                let path = self.tabs[self.active].path.clone();
                if let Err(e) = crate::platform::open_terminal(&path) {
                    tracing::error!(path = %path.display(), "failed to launch terminal: {e}");
                    self.ops_ui.journal.push(format!("open terminal: {e}"));
                }
                ui.close();
            }
            ui.separator();
            if ui.button(self.t("toolbar.refresh").to_owned()).clicked() {
                self.refresh(ctx);
                ui.close();
            }
        });
    }

    /// Shared row/tile interactions: selection, right-click select, open, context menu.
    pub(super) fn entry_actions(
        &mut self,
        resp: &egui::Response,
        index: usize,
        entry: &FileEntry,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        let archive_member = entry.archive_member.clone();
        let zip_file =
            archive_member.is_none() && !entry.is_dir && entry.extension.as_deref() == Some("zip");
        if resp.clicked() {
            // Clicking the list drops text-field focus (address bar) so shortcuts work.
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            self.select_click(index, ui.input(|i| i.modifiers));
        }
        // Explorer semantics: right-click on an unselected row selects just it.
        if resp.secondary_clicked() && !self.tabs[self.active].selection.contains(&index) {
            let tab = &mut self.tabs[self.active];
            tab.selection.clear();
            tab.selection.insert(index);
            tab.anchor = Some(index);
            tab.cursor = index;
        }
        if resp.double_clicked() {
            if let Some(member) = &archive_member {
                if entry.is_dir {
                    self.navigate_zip(self.active, member.clone(), ctx);
                }
            } else if entry.is_dir || zip_file {
                self.navigate(self.active, entry.path.clone(), ctx);
            } else {
                let _ = crate::platform::shell_open(&entry.path);
            }
        }
        if self.tabs[self.active].cursor == index && self.scroll_to_cursor {
            ui.scroll_to_rect(resp.rect, None);
        }

        resp.context_menu(|ui| {
            style::compact_context_menu(ui);
            if let Some(member) = &archive_member {
                if entry.is_dir {
                    if ui.button(self.t("ctx.open").to_owned()).clicked() {
                        self.navigate_zip(self.active, member.clone(), ctx);
                        ui.close();
                    }
                    if ui.button(self.t("zip.extract_folder").to_owned()).clicked() {
                        self.extract_zip_folder(member.clone(), ctx);
                        ui.close();
                    }
                } else {
                    if ui.button(self.t("zip.extract_file").to_owned()).clicked() {
                        self.extract_zip_file(member.clone(), ctx);
                        ui.close();
                    }
                }
            } else {
                if ui.button(self.t("ctx.open").to_owned()).clicked() {
                    if entry.is_dir || zip_file {
                        self.navigate(self.active, entry.path.clone(), ctx);
                    } else {
                        let _ = crate::platform::shell_open(&entry.path);
                    }
                    ui.close();
                }
                if !entry.is_dir
                    && crate::core::preview::supports_preview(&entry.path)
                    && ui.button(self.t("ctx.preview").to_owned()).clicked()
                {
                    self.open_preview(entry.path.clone(), ctx);
                    ui.close();
                }
                if (entry.is_dir || zip_file)
                    && ui.button(self.t("ctx.open_new_tab").to_owned()).clicked()
                {
                    self.queue_open_path_in_new_tab(entry.path.clone());
                    ui.close();
                }
                if zip_file && ui.button(self.t("zip.extract_all").to_owned()).clicked() {
                    self.extract_zip_all_from(entry.path.clone(), ctx);
                    ui.close();
                }
                let zip_sources = self.selected_paths();
                if !zip_sources.is_empty()
                    && ui
                        .add_enabled(
                            self.can_start_zip_operation(),
                            egui::Button::new(self.t("zip.create_archive").to_owned()),
                        )
                        .clicked()
                {
                    self.create_zip_from_selection(zip_sources, ctx);
                    ui.close();
                }
                ui.separator();
                if ui.button(self.t("ctx.copy").to_owned()).clicked() {
                    self.act_copy();
                    ui.close();
                }
                if ui.button(self.t("ctx.cut").to_owned()).clicked() {
                    self.act_cut();
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.ops_ui.clipboard.is_some(),
                        egui::Button::new(self.t("ctx.paste").to_owned()),
                    )
                    .clicked()
                {
                    self.act_paste();
                    ui.close();
                }
                ui.separator();
                if ui.button(self.t("ctx.rename").to_owned()).clicked() {
                    self.act_rename();
                    ui.close();
                }
                if ui.button(self.t("ctx.delete").to_owned()).clicked() {
                    self.act_delete(false);
                    ui.close();
                }
                ui.separator();
                if ui.button(self.t("ctx.properties").to_owned()).clicked() {
                    self.props_target = Some(entry.path.clone());
                    ui.close();
                }
            }
        });
    }

    /// Click selection honoring Ctrl/Shift modifiers (Explorer semantics).
    fn select_click(&mut self, index: usize, mods: egui::Modifiers) {
        let tab = &mut self.tabs[self.active];
        tab.cursor = crate::ui::tabs::apply_click(
            &mut tab.selection,
            &mut tab.anchor,
            index,
            mods.ctrl,
            mods.shift,
        );
    }
}
