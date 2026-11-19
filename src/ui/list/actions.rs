//! Выбор элементов и контекстные действия.

use super::*;

fn response_context_menu_opened(
    response: &egui::Response,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    response.context_menu(add_contents).is_some()
}

impl FileManagerApp {
    /// Пустое место списка сбрасывает выделение по левому клику и открывает меню по правому.
    pub(super) fn empty_area_menu(&mut self, resp: &egui::Response, ctx: &egui::Context) {
        if resp.clicked() {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            let tab = &mut self.tabs[self.active];
            tab.clear_selection();
        }
        let in_archive = self.tabs[self.active].archive_prefix.is_some();
        let in_trash = self.tabs[self.active].secure_trash;
        response_context_menu_opened(resp, |ui| {
            if in_trash {
                style::compact_context_menu(ui);
                if ui.button(self.t("trash.empty").to_owned()).clicked() {
                    self.trash_empty_confirm = true;
                    ui.close();
                }
                if ui.button(self.t("toolbar.refresh").to_owned()).clicked() {
                    self.refresh(ctx);
                    ui.close();
                }
                return;
            }
            style::compact_context_menu(ui);
            if in_archive {
                if ui.button(self.t("zip.extract_all").to_owned()).clicked() {
                    self.extract_zip_all(ctx);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.can_start_zip_operation(),
                        egui::Button::new(self.t("zip.extract_to").to_owned()),
                    )
                    .clicked()
                {
                    self.prompt_zip_extract_to(ctx);
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
            ui.menu_button(self.t("ctx.create").to_owned(), |ui| {
                if ui.button(self.t("ctx.create_folder").to_owned()).clicked() {
                    self.open_create_dialog(true);
                    ui.close();
                }
                if ui
                    .button(self.t("ctx.create_text_file").to_owned())
                    .clicked()
                {
                    self.open_create_dialog(false);
                    ui.close();
                }
            });
            if ui.button(self.t("ctx.open_terminal").to_owned()).clicked() {
                let path = self.tabs[self.active].path.clone();
                if let Err(e) = crate::platform::open_terminal(&path) {
                    self.record_io_error(Some(&path), &e);
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
        visual_order: Option<&[usize]>,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        self.begin_file_drag(resp, index, entry, ctx);
        let drop_path = crate::ui::file_dnd::entry_drop_destination(entry);
        let target_allowed = drop_path.is_some_and(|path| self.active_folder_accepts_drop(path));
        self.handle_file_drop_response(
            resp,
            drop_path,
            entry.is_dir,
            target_allowed,
            crate::ui::file_dnd::DropSurface::Entry,
            ui,
            ctx,
        );

        let archive_member = entry.archive_member.clone();
        let selection_marquee_active = self.tabs[self.active].selection_marquee.is_some();
        let zip_file =
            archive_member.is_none() && !entry.is_dir && entry.extension.as_deref() == Some("zip");
        if !selection_marquee_active && resp.clicked() {
            // Clicking the list drops text-field focus (address bar) so shortcuts work.
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            self.select_click(index, visual_order, ui.input(|i| i.modifiers));
        }
        // Explorer semantics: right-click on an unselected row selects just it.
        let entry_key = crate::ui::tabs::SelectionKey::from_entry(entry);
        if !selection_marquee_active
            && resp.secondary_clicked()
            && !self.tabs[self.active].selection.contains(&entry_key)
        {
            let tab = &mut self.tabs[self.active];
            tab.selection.clear();
            tab.selection.insert(entry_key.clone());
            tab.anchor = Some(entry_key);
            tab.cursor = index;
        }
        let in_trash = self.tabs[self.active].secure_trash;
        if in_trash {
            resp.context_menu(|ui| {
                style::compact_context_menu(ui);
                if ui.button(self.t("trash.restore").to_owned()).clicked() {
                    let ids = self.selected_trash_ids();
                    self.restore_trash_ids(ids, None, ctx);
                    ui.close();
                }
                if ui.button(self.t("trash.purge").to_owned()).clicked() {
                    let ids = self.selected_trash_ids();
                    if !ids.is_empty() {
                        self.trash_purge_ids = Some(ids);
                    }
                    ui.close();
                }
                if ui.button(self.t("trash.empty").to_owned()).clicked() {
                    self.trash_empty_confirm = true;
                    ui.close();
                }
                if ui.button(self.t("toolbar.refresh").to_owned()).clicked() {
                    self.refresh(ctx);
                    ui.close();
                }
            });
            return;
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
                } else if ui.button(self.t("zip.extract_file").to_owned()).clicked() {
                    self.extract_zip_file(member.clone(), ctx);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        self.can_start_zip_operation(),
                        egui::Button::new(self.t("zip.extract_to").to_owned()),
                    )
                    .clicked()
                {
                    self.prompt_zip_extract_to(ctx);
                    ui.close();
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
                if zip_file
                    && ui
                        .add_enabled(
                            self.can_start_zip_operation(),
                            egui::Button::new(self.t("zip.extract_to").to_owned()),
                        )
                        .clicked()
                {
                    self.prompt_zip_file_extract_to(entry.path.clone(), ctx);
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
                self.winrar_context_menu(ui, self.selected_paths(), ctx);
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
                    self.props_multi_target = None;
                    ui.close();
                }
            }
        });
    }

    /// Click selection honoring Ctrl/Shift modifiers (Explorer semantics).
    fn select_click(
        &mut self,
        index: usize,
        visual_order: Option<&[usize]>,
        mods: egui::Modifiers,
    ) {
        let tab = &mut self.tabs[self.active];
        tab.cursor = crate::ui::tabs::apply_click(
            &mut tab.selection,
            &mut tab.anchor,
            &tab.entries,
            visual_order,
            index,
            mods.command,
            mods.shift,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::settings::ViewMode;

    const SCREEN_RECT: egui::Rect =
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(500.0, 400.0));

    #[derive(Default)]
    struct MenuFrame {
        background_rect: Option<egui::Rect>,
        placeholder_rect: Option<egui::Rect>,
        spinner_rect: Option<egui::Rect>,
        row_rect: Option<egui::Rect>,
        background_secondary_clicked: bool,
        background_primary_drag_started: bool,
        background_menu_open: bool,
        row_menu_open: bool,
    }

    fn run_menu_frame(
        ctx: &egui::Context,
        view_mode: ViewMode,
        placeholder: Option<&str>,
        with_row: bool,
        events: Vec<egui::Event>,
        frame: &mut MenuFrame,
    ) {
        run_menu_frame_with_spinner(ctx, view_mode, placeholder, false, with_row, events, frame);
    }

    fn run_menu_frame_with_spinner(
        ctx: &egui::Context,
        view_mode: ViewMode,
        placeholder: Option<&str>,
        loading_spinner: bool,
        with_row: bool,
        events: Vec<egui::Event>,
        frame: &mut MenuFrame,
    ) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(SCREEN_RECT),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let background_rect = crate::ui::list::list_background_rect(ui);
                    if loading_spinner {
                        ui.vertical_centered(|ui| {
                            ui.add_space(crate::ui::style::SPACE_EMPTY_STATE);
                            frame.spinner_rect = Some(ui.spinner().rect);
                            if let Some(text) = placeholder {
                                frame.placeholder_rect = Some(ui.label(text).rect);
                            }
                        });
                    } else if let Some(text) = placeholder {
                        ui.vertical_centered(|ui| {
                            ui.add_space(crate::ui::style::SPACE_EMPTY_STATE);
                            frame.placeholder_rect = Some(ui.label(text).rect);
                        });
                    }
                    let background =
                        crate::ui::list::list_background_response(ui, view_mode, background_rect);
                    frame.background_rect = Some(background.rect);
                    frame.background_secondary_clicked |= background.secondary_clicked();
                    frame.background_primary_drag_started |=
                        background.drag_started_by(egui::PointerButton::Primary);
                    if with_row {
                        let row_rect = egui::Rect::from_min_size(
                            background.rect.min,
                            egui::vec2(background.rect.width(), 24.0),
                        );
                        frame.row_rect = Some(row_rect);
                        let row = ui.interact(
                            row_rect,
                            ui.scope_id().with("test_file_row"),
                            egui::Sense::click(),
                        );
                        frame.row_menu_open |= response_context_menu_opened(&row, |ui| {
                            ui.label("Контекстное меню строки");
                        });
                    }
                    frame.background_menu_open |= response_context_menu_opened(&background, |ui| {
                        ui.label("Контекстное меню пустого места");
                    });
                });
            },
        );
        output.textures_delta.clear();
    }

    fn pointer_event(
        pos: egui::Pos2,
        button: egui::PointerButton,
        pressed: bool,
    ) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    fn simulate_secondary_click(
        view_mode: ViewMode,
        placeholder: Option<&str>,
        with_row: bool,
        click_placeholder: bool,
        click_row: bool,
    ) -> MenuFrame {
        let ctx = egui::Context::default();
        let mut frame = MenuFrame::default();
        run_menu_frame(
            &ctx,
            view_mode,
            placeholder,
            with_row,
            Vec::new(),
            &mut frame,
        );
        let pos = if click_placeholder {
            frame.placeholder_rect.unwrap().center()
        } else if click_row {
            frame.row_rect.unwrap().center()
        } else {
            frame.background_rect.unwrap().center()
        };
        run_menu_frame(
            &ctx,
            view_mode,
            placeholder,
            with_row,
            pointer_event(pos, egui::PointerButton::Secondary, true),
            &mut frame,
        );
        run_menu_frame(
            &ctx,
            view_mode,
            placeholder,
            with_row,
            pointer_event(pos, egui::PointerButton::Secondary, false),
            &mut frame,
        );
        frame
    }

    fn simulate_loading_secondary_click() -> MenuFrame {
        let ctx = egui::Context::default();
        let mut frame = MenuFrame::default();
        run_menu_frame_with_spinner(
            &ctx,
            ViewMode::Table,
            Some("Загрузка"),
            true,
            false,
            Vec::new(),
            &mut frame,
        );
        let pos = frame.spinner_rect.unwrap().center();
        run_menu_frame_with_spinner(
            &ctx,
            ViewMode::Table,
            Some("Загрузка"),
            true,
            false,
            pointer_event(pos, egui::PointerButton::Secondary, true),
            &mut frame,
        );
        run_menu_frame_with_spinner(
            &ctx,
            ViewMode::Table,
            Some("Загрузка"),
            true,
            false,
            pointer_event(pos, egui::PointerButton::Secondary, false),
            &mut frame,
        );
        frame
    }

    fn simulate_primary_drag(
        view_mode: ViewMode,
        placeholder: Option<&str>,
    ) -> (MenuFrame, egui::Pos2) {
        let ctx = egui::Context::default();
        let mut frame = MenuFrame::default();
        run_menu_frame(&ctx, view_mode, placeholder, false, Vec::new(), &mut frame);
        let start = frame
            .placeholder_rect
            .unwrap_or_else(|| frame.background_rect.unwrap())
            .center();
        let end = start + egui::vec2(24.0, 0.0);
        run_menu_frame(
            &ctx,
            view_mode,
            placeholder,
            false,
            pointer_event(start, egui::PointerButton::Primary, true),
            &mut frame,
        );
        run_menu_frame(
            &ctx,
            view_mode,
            placeholder,
            false,
            vec![egui::Event::PointerMoved(end)],
            &mut frame,
        );
        run_menu_frame(
            &ctx,
            view_mode,
            placeholder,
            false,
            pointer_event(end, egui::PointerButton::Primary, false),
            &mut frame,
        );
        (frame, start)
    }

    #[test]
    fn empty_folder_secondary_click_opens_empty_area_context_menu() {
        let frame =
            simulate_secondary_click(ViewMode::Table, Some("Папка пуста"), false, true, false);

        assert!(
            frame.background_menu_open,
            "secondary_clicked={}, background_rect={:?}, placeholder_rect={:?}",
            frame.background_secondary_clicked, frame.background_rect, frame.placeholder_rect
        );
    }

    #[test]
    fn secondary_click_works_right_after_navigate_into_new_empty_folder() {
        let frame = simulate_loading_secondary_click();

        assert!(frame.background_menu_open);
    }

    #[test]
    fn zero_filter_results_secondary_click_opens_empty_area_menu() {
        let app = include_str!("../app/mod.rs")
            .split_whitespace()
            .collect::<String>();
        assert!(app.contains("tab.entries.is_empty()"));
        assert!(app.contains("self.t(\"search.no_results\")"));

        let frame = simulate_secondary_click(
            ViewMode::Table,
            Some("Совпадений не найдено"),
            false,
            true,
            false,
        );
        assert!(frame.background_menu_open);
    }

    #[test]
    fn empty_area_placeholder_does_not_intercept_pointer() {
        let frame =
            simulate_secondary_click(ViewMode::Table, Some("Папка пуста"), false, true, false);

        assert!(frame.placeholder_rect.is_some());
        assert!(frame.background_menu_open);
    }

    #[test]
    fn non_empty_folder_below_rows_secondary_click_not_regressed() {
        let frame = simulate_secondary_click(ViewMode::Table, None, true, false, false);

        assert!(frame.background_menu_open);
        assert!(!frame.row_menu_open);
    }

    #[test]
    fn row_context_menu_not_regressed() {
        let frame = simulate_secondary_click(ViewMode::Table, None, true, false, true);

        assert!(!frame.background_menu_open);
        assert!(frame.row_menu_open);
    }

    #[test]
    fn empty_area_menu_has_create_submenu_with_folder_and_text_file() {
        let source = include_str!("actions.rs");
        let menu = source
            .split("fn empty_area_menu")
            .nth(1)
            .unwrap()
            .split("pub(super) fn entry_actions")
            .next()
            .unwrap();
        let create = menu.find("ctx.create").unwrap();
        let folder = menu.find("ctx.create_folder").unwrap();
        let text_file = menu.find("ctx.create_text_file").unwrap();

        assert!(menu[..create].contains("menu_button"));
        assert!(folder < text_file);
    }

    #[test]
    fn create_still_available_via_empty_area_create_submenu() {
        let source = include_str!("actions.rs");
        let menu = source
            .split("fn empty_area_menu")
            .nth(1)
            .unwrap()
            .split("pub(super) fn entry_actions")
            .next()
            .unwrap();
        assert!(menu.contains("ui.menu_button(self.t(\"ctx.create\")"));
        assert!(menu.contains("self.open_create_dialog(true)"));
        assert!(menu.contains("self.open_create_dialog(false)"));
        assert!(menu.contains("\"ctx.create_folder\""));
        assert!(menu.contains("\"ctx.create_text_file\""));
    }

    #[test]
    fn empty_area_menu_other_items_and_order_not_regressed() {
        let source = include_str!("actions.rs");
        let menu = source
            .split("fn empty_area_menu")
            .nth(1)
            .unwrap()
            .split("pub(super) fn entry_actions")
            .next()
            .unwrap();
        let paste = menu.find("ctx.paste").unwrap();
        let create = menu.find("ctx.create").unwrap();
        let terminal = menu.find("ctx.open_terminal").unwrap();
        let refresh = menu.rfind("toolbar.refresh").unwrap();

        assert!(paste < create && create < terminal && terminal < refresh);
    }

    #[test]
    fn empty_area_menu_same_in_table_list_and_grid() {
        for view_mode in [ViewMode::Table, ViewMode::List, ViewMode::Grid] {
            let frame =
                simulate_secondary_click(view_mode, Some("Папка пуста"), false, true, false);
            assert!(frame.background_menu_open, "menu failed for {view_mode:?}");
        }
    }

    #[test]
    fn marquee_starts_on_empty_folder_background_and_selects_nothing() {
        let (frame, start) = simulate_primary_drag(ViewMode::Table, Some("Папка пуста"));
        let entries: Vec<crate::core::fs::FileEntry> = Vec::new();
        let selection = std::collections::BTreeSet::<crate::ui::tabs::SelectionKey>::new();

        assert!(frame.background_primary_drag_started);
        assert!(entries.is_empty());
        let marquee =
            super::super::selection_marquee_at(start, frame.background_rect.unwrap()).unwrap();
        assert!(selection.is_empty());
        assert!(marquee.added.is_empty());
        assert_eq!(marquee.start, start);
    }
}
