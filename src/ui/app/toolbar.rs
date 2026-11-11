//! Главное меню, вкладки и панели навигации.

use super::*;

impl FileManagerApp {
    // ------------------------------------------------------------------
    // Chrome: menus, tab bar, toolbar, breadcrumbs
    // ------------------------------------------------------------------

    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
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
                if ui
                    .button(self.t("integrate.open_recycle").to_owned())
                    .clicked()
                {
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
                            .selectable_label(
                                self.store.settings.theme == theme,
                                self.t(key).to_owned(),
                            )
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

    pub(super) fn tab_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        configure_tab_scroll_style(ui.style_mut());
        let available = ui.available_rect_before_wrap();
        let frame_rect = egui::Rect::from_min_size(
            available.min,
            egui::vec2(
                ui.available_width(),
                super::window_frame::WINDOW_FRAME_HEIGHT,
            ),
        )
        .expand2(egui::vec2(style::FRAME_PADDING_X as f32, 0.0));
        self.advance_tab_drag_state(ctx);
        let pointer_pos = self.tab_drag_preview.pointer_pos(ctx);
        let mut tab_rects = Vec::new();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = style::SPACE_XS;
            let mut act: Option<(&str, usize)> = None;
            let mut dragging_tab = self.tab_drag_state.is_dragging();
            let primary_press_origin = ctx.input(|input| {
                input
                    .pointer
                    .button_pressed(egui::PointerButton::Primary)
                    .then(|| input.pointer.press_origin())
                    .flatten()
            });
            let button_font = egui::TextStyle::Button.resolve(ui.style());
            let tab_text_color = ui.visuals().text_color();
            // Галлеи задают фактическую ширину вкладок в текущем кадре.
            let metas: Vec<_> = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| !self.is_tab_detached(tab.id))
                .map(|(index, tab)| {
                    let title = tab.title();
                    let galley = ui.painter().layout_no_wrap(
                        title,
                        button_font.clone(),
                        tab_text_color,
                    );
                    (index, tab.pinned, galley)
                })
                .collect();

            let spacing = ui.spacing().item_spacing.x;
            let plus_galley = ui.painter().layout_no_wrap(
                "+".to_owned(),
                egui::TextStyle::Small.resolve(ui.style()),
                ui.visuals().text_color(),
            );
            let plus_size = super::window_frame::new_tab_button_size(
                plus_galley.size().y,
                ui.spacing().interact_size.y,
            );
            let tab_content_width = metas
                .iter()
                .map(|(_, pinned, galley)| {
                    let pin_width = if *pinned {
                        super::window_frame::TAB_PIN_WIDTH
                    } else {
                        0.0
                    };
                    let close_width = if *pinned {
                        0.0
                    } else {
                        super::window_frame::TAB_CLOSE_WIDTH
                    };
                    galley.size().x
                        + super::window_frame::TAB_TEXT_EXTRA_WIDTH
                        + pin_width
                        + close_width
                })
                .sum::<f32>()
                + spacing * metas.len().saturating_sub(1) as f32;
            // Кнопка «+» и интервал перед ней входят в ширину центрируемого блока.
            let block_width = tab_content_width + spacing + plus_size.x;
            let center_area_width = (ui.available_width()
                - super::window_frame::WINDOW_CONTROLS_WIDTH
                - super::window_frame::MAIN_WINDOW_DRAG_AREA_MIN_WIDTH
                - spacing * 2.0)
                .max(0.0);
            let scroll_width = (center_area_width - plus_size.x - spacing).max(1.0);
            let center_offset = tab_strip_center_offset(block_width, center_area_width);
            let centered = center_offset.is_some();
            if let Some(offset) = center_offset {
                ui.add_space(offset);
            }

            let scroll_target = self.tab_strip_scroll_target;
            let mut scroll_request_sent = false;
            egui::ScrollArea::horizontal()
                .id_salt("tab_strip_scroll")
                .max_width(scroll_width)
                .auto_shrink([true, true])
                .content_margin(tab_strip_content_margin(centered))
                .scroll_source(tab_scroll_source())
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    let mut scroll_target_rect = None;
                    ui.horizontal(|ui| {
                        for (display_index, (i, pinned, galley)) in metas.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = style::SPACE_XXS;

                                // Клик переключает вкладку, а перетаскивание меняет её порядок.
                                // Крестик закрытия расположен внутри вкладки справа.
                                let pin_w = if *pinned {
                                    super::window_frame::TAB_PIN_WIDTH
                                } else {
                                    0.0
                                };
                                let close_w = if *pinned {
                                    0.0
                                } else {
                                    super::window_frame::TAB_CLOSE_WIDTH
                                };
                                let size = galley.size()
                                    + egui::vec2(
                                        super::window_frame::TAB_TEXT_EXTRA_WIDTH + pin_w + close_w,
                                        super::window_frame::TAB_VERTICAL_EXTRA_HEIGHT,
                                    );
                                // Обычный клик сохраняет работу глобальных сочетаний клавиш.
                                // Drag-ответ остаётся активным, даже если указатель вышел за вкладку.
                                let (rect, resp) =
                                    ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                                tab_rects.push((display_index, rect));
                                if scroll_target == Some(self.tabs[*i].id) {
                                    scroll_target_rect = Some(rect);
                                }
                                let x_rect = egui::Rect::from_min_max(
                                    egui::pos2(rect.max.x - close_w, rect.min.y),
                                    rect.max,
                                );
                                // Начало нажатия проверяется по текущей геометрии вкладки.
                                if matches!(
                                    self.tab_drag_state,
                                    super::tab_drag_preview::TabDragState::Idle
                                ) && let Some(press_origin) = primary_press_origin
                                    && ctx.layer_id_at(press_origin) == Some(resp.layer_id)
                                    && super::tab_drag_preview::tab_drag_press_hits_body(
                                        resp.interact_rect,
                                        x_rect.intersect(resp.interact_rect),
                                        press_origin,
                                    )
                                {
                                    let pixels_per_point = ctx.pixels_per_point();
                                    let start_screen_position = ctx
                                        .input(|input| input.viewport().inner_rect)
                                        .map(|inner_rect| {
                                            super::tab_drag_preview::viewport_position_to_screen(
                                                press_origin,
                                                inner_rect.min,
                                                pixels_per_point,
                                            )
                                        })
                                        .or_else(crate::platform::cursor_screen_position);
                                    if let Some(start_screen_position) = start_screen_position {
                                        let tab_id = self.tabs[*i].id;
                                        self.tab_drag_state =
                                            super::tab_drag_preview::TabDragState::Pressed {
                                                tab_id,
                                                start_screen_position,
                                                start_pixels_per_point: pixels_per_point,
                                            };
                                        self.advance_tab_drag_state(ctx);
                                    }
                                }
                                dragging_tab = self.tab_drag_state.is_dragging();
                                let over_x = tab_close_button_hovered(
                                    dragging_tab,
                                    *pinned,
                                    x_rect,
                                    pointer_pos,
                                );
                                let visuals = ui.visuals();
                                let painter = ui.painter();
                                if *i == self.active {
                                    painter.rect_filled(
                                        rect,
                                        style::RADIUS_ROW,
                                        visuals.selection.bg_fill,
                                    );
                                } else if resp.hovered() {
                                    painter.rect_filled(
                                        rect,
                                        style::RADIUS_ROW,
                                        visuals.widgets.hovered.bg_fill,
                                    );
                                }
                                let color = if *i == self.active {
                                    visuals.strong_text_color()
                                } else {
                                    visuals.text_color()
                                };
                                let text_left = rect.left()
                                    + super::window_frame::TAB_HORIZONTAL_PADDING
                                    + pin_w;
                                let text_right = rect.right()
                                    - super::window_frame::TAB_HORIZONTAL_PADDING
                                    - close_w;
                                painter.galley(
                                    egui::pos2(
                                        (text_left + text_right - galley.size().x) * 0.5,
                                        rect.center().y - galley.size().y * 0.5,
                                    ),
                                    galley.clone(),
                                    color,
                                );
                                if *pinned {
                                    let pin_size = super::window_frame::TAB_PIN_ICON_SIZE;
                                    let pin_rect = egui::Rect::from_min_size(
                                        egui::pos2(
                                            rect.left() + super::window_frame::TAB_PIN_ICON_OFFSET,
                                            rect.center().y - pin_size * 0.5,
                                        ),
                                        egui::vec2(pin_size, pin_size),
                                    );
                                    icon_pin(painter, pin_rect, color);
                                }
                                if !*pinned {
                                    if over_x {
                                        painter.rect_filled(
                                            x_rect.shrink(
                                                super::window_frame::TAB_CLOSE_HIGHLIGHT_INSET,
                                            ),
                                            style::RADIUS_ROW,
                                            visuals.error_fg_color.gamma_multiply(0.25),
                                        );
                                    }
                                    let x_color = if over_x {
                                        visuals.error_fg_color
                                    } else {
                                        visuals.weak_text_color()
                                    };
                                    let cross = super::window_frame::TAB_CLOSE_CROSS_RADIUS;
                                    painter.line_segment(
                                        [
                                            x_rect.center() + egui::vec2(-cross, -cross),
                                            x_rect.center() + egui::vec2(cross, cross),
                                        ],
                                        egui::Stroke::new(
                                            super::window_frame::TAB_CLOSE_CROSS_STROKE,
                                            x_color,
                                        ),
                                    );
                                    painter.line_segment(
                                        [
                                            x_rect.center() + egui::vec2(-cross, cross),
                                            x_rect.center() + egui::vec2(cross, -cross),
                                        ],
                                        egui::Stroke::new(
                                            super::window_frame::TAB_CLOSE_CROSS_STROKE,
                                            x_color,
                                        ),
                                    );
                                }

                                resp.context_menu(|ui| {
                                    style::compact_context_menu(ui);
                                    let close_txt = self.t("tab.close").to_owned();
                                    let others_txt = self.t("tab.close_others").to_owned();
                                    let dup_txt = self.t("tab.duplicate").to_owned();
                                    let pin_txt = self
                                        .t(if *pinned { "tab.unpin" } else { "tab.pin" })
                                        .to_owned();
                                    if !*pinned && ui.button(close_txt).clicked() {
                                        act = Some(("close", *i));
                                        ui.close();
                                    }
                                    if ui.button(others_txt).clicked() {
                                        act = Some(("close_others", *i));
                                        ui.close();
                                    }
                                    if ui.button(dup_txt).clicked() {
                                        act = Some(("dup", *i));
                                        ui.close();
                                    }
                                    if ui.button(pin_txt).clicked() {
                                        act = Some(("pin", *i));
                                        ui.close();
                                    }
                                });
                                let suppress_tab_clicks = dragging_tab
                                    || matches!(
                                        self.tab_drag_state,
                                        super::tab_drag_preview::TabDragState::Cancelled
                                    );
                                if !suppress_tab_clicks && resp.clicked() {
                                    let in_x = resp
                                        .interact_pointer_pos()
                                        .map(|p| x_rect.contains(p))
                                        .unwrap_or(false);
                                    if in_x {
                                        act = Some(("close", *i));
                                    } else {
                                        act = Some(("switch", *i));
                                    }
                                }
                                if !suppress_tab_clicks && resp.middle_clicked() && !*pinned {
                                    act = Some(("close", *i));
                                }
                                if self.tab_drag_state.is_dragging()
                                    && let Some(pos) = pointer_pos
                                    && rect.contains(pos)
                                {
                                    let before = pos.x < rect.center().x;
                                    // Показываем место вставки рядом с наведённой вкладкой.
                                    ui.painter().vline(
                                        if before { rect.left() } else { rect.right() },
                                        rect.y_range(),
                                        egui::Stroke::new(
                                            super::window_frame::TAB_INSERTION_STROKE,
                                            ui.visuals().selection.stroke.color,
                                        ),
                                    );
                                }
                            });
                        }
                    });
                    if let Some(rect) = scroll_target_rect {
                        if let Some(align) = tab_strip_scroll_alignment(rect, ui.clip_rect()) {
                            ui.scroll_to_rect(rect, Some(align));
                        }
                        scroll_request_sent = true;
                    }
                });
            if scroll_request_sent && self.tab_strip_scroll_target == scroll_target {
                self.tab_strip_scroll_target = None;
            } else if let Some(tab_id) = scroll_target
                && (!self.tabs.iter().any(|tab| tab.id == tab_id) || self.is_tab_detached(tab_id))
            {
                self.tab_strip_scroll_target = None;
            }

            // При переполнении кнопка остаётся рядом с видимой областью прокрутки;
            // без переполнения список сжимается до вкладок, и кнопка идёт сразу за ними.
            if ui
                .add_sized(plus_size, egui::Button::new("+").small())
                .clicked()
            {
                self.new_tab(ctx);
            }
            let drag_width =
                (ui.available_width() - super::window_frame::WINDOW_CONTROLS_WIDTH - spacing)
                    .max(0.0);
            // Здесь остаётся пустое место между вкладками и кнопками рамки.
            let (drag_id, drag_rect) = ui.allocate_space(egui::vec2(
                drag_width,
                super::window_frame::WINDOW_FRAME_HEIGHT,
            ));
            let drag_response = ui.interact(
                super::window_frame::main_window_drag_hit_rect(drag_rect),
                drag_id,
                egui::Sense::click_and_drag(),
            );
            if drag_response.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag_response.double_clicked_by(egui::PointerButton::Primary) {
                super::window_frame::toggle_maximized(ctx);
            }
            self.window_controls(ctx, ui);
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
        self.tab_drag_preview
            .set_drop_geometry(frame_rect, tab_rects);
        if matches!(
            self.tab_drag_state,
            super::tab_drag_preview::TabDragState::Cancelled
        ) && !primary_button_held(ctx)
        {
            self.tab_drag_state = super::tab_drag_preview::TabDragState::Idle;
        }
        if matches!(
            self.tab_drag_state,
            super::tab_drag_preview::TabDragState::Pressed { .. }
                | super::tab_drag_preview::TabDragState::Cancelled
        ) {
            ctx.request_repaint_after(std::time::Duration::from_millis(
                super::tab_drag_preview::TAB_DRAG_PREVIEW_REPAINT_MS,
            ));
        }
    }

    fn advance_tab_drag_state(&mut self, ctx: &egui::Context) {
        let super::tab_drag_preview::TabDragState::Pressed { tab_id, .. } = self.tab_drag_state
        else {
            return;
        };
        if !self.tabs.iter().any(|tab| tab.id == tab_id) {
            self.tab_drag_state = super::tab_drag_preview::TabDragState::Cancelled;
            return;
        }

        let escape_pressed = ctx.input(|input| input.key_pressed(egui::Key::Escape))
            || crate::platform::escape_key_down();
        let next = super::tab_drag_preview::advance_tab_drag_state(
            self.tab_drag_state,
            current_screen_position(ctx),
            primary_button_held(ctx),
            escape_pressed,
        );
        if let super::tab_drag_preview::TabDragState::Dragging { tab_id } = next {
            self.tab_drag_preview.begin(tab_id);
        }
        self.tab_drag_state = next;
    }

    pub(super) fn toolbar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
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
            if icon_button(
                ui,
                can_back,
                self.t("toolbar.back").to_owned(),
                |p, r, c| {
                    icon_arrow(p, r, egui::vec2(-1.0, 0.0), c);
                },
            )
            .clicked()
            {
                self.go_back(ctx);
            }
            if icon_button(
                ui,
                can_fwd,
                self.t("toolbar.forward").to_owned(),
                |p, r, c| {
                    icon_arrow(p, r, egui::vec2(1.0, 0.0), c);
                },
            )
            .clicked()
            {
                self.go_forward(ctx);
            }
            if icon_button(
                ui,
                has_parent,
                self.t("toolbar.up").to_owned(),
                |p, r, c| {
                    icon_arrow(p, r, egui::vec2(0.0, -1.0), c);
                },
            )
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
            let marked = !in_archive
                && self
                    .store
                    .settings
                    .bookmarks
                    .contains(&self.tabs[self.active].path);
            let star_color = ui.visuals().selection.stroke.color;
            if icon_button(
                ui,
                !in_archive,
                self.t("toolbar.bookmark").to_owned(),
                |p, r, c| {
                    icon_star(p, r, if marked { star_color } else { c }, marked);
                },
            )
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
                        .add_enabled(
                            self.ops_ui.clipboard.is_some(),
                            egui::Button::new(self.t("ctx.paste").to_owned()),
                        )
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

        // Breadcrumbs прокручиваются в оставшейся ширине, кнопки режимов закреплены справа.
        let path = self.tabs[self.active].path.clone();
        let archive_prefix = self.tabs[self.active].archive_prefix.clone();
        let modes = [
            (ViewMode::Table, "view.table"),
            (ViewMode::List, "view.list"),
            (ViewMode::Grid, "view.grid"),
        ];
        let mode_labels: Vec<String> = modes
            .iter()
            .map(|(_, key)| self.t(key).to_owned())
            .collect();
        let row_spacing = style::SPACE_XXS;
        let button_font = egui::TextStyle::Button.resolve(ui.style());
        let button_padding = ui.spacing().button_padding.x * 2.0;
        let mode_switcher_width: f32 = mode_labels
            .iter()
            .map(|label| {
                let text_width = ui
                    .painter()
                    .layout_no_wrap(
                        label.clone(),
                        button_font.clone(),
                        ui.visuals().text_color(),
                    )
                    .size()
                    .x;
                (text_width + button_padding).max(ui.spacing().interact_size.x)
            })
            .sum::<f32>()
            + row_spacing * 2.0;
        let breadcrumbs = breadcrumb_segments(&path);
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = row_spacing;
            let gap = ui.spacing().item_spacing.x;
            let breadcrumbs_width = (ui.available_width() - mode_switcher_width - gap).max(0.0);
            if breadcrumbs_width > 0.0 {
                egui::ScrollArea::horizontal()
                    .id_salt("breadcrumb_path_scroll")
                    .max_width(breadcrumbs_width)
                    .auto_shrink([false, true])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (label, seg_path) in &breadcrumbs {
                                if ui.selectable_label(false, label).clicked() {
                                    self.navigate(self.active, seg_path.clone(), ctx);
                                }
                                ui.label("›");
                            }
                            if let Some(prefix) = archive_prefix
                                .as_deref()
                                .filter(|prefix| !prefix.is_empty())
                            {
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
                    });
            }
            ui.horizontal(|ui| {
                for ((mode, _), label) in modes.iter().zip(mode_labels) {
                    let active = self.store.settings.view_mode == *mode;
                    if ui.selectable_label(active, label).clicked() {
                        self.set_view(*mode);
                    }
                }
            });
        });
    }
}

fn current_screen_position(ctx: &egui::Context) -> Option<(i32, i32)> {
    // Сначала берём экранные координаты, доступные вне окна.
    crate::platform::cursor_screen_position().or_else(|| {
        let pointer = ctx.pointer_latest_pos()?;
        let inner_min = ctx.input(|input| input.viewport().inner_rect.map(|rect| rect.min))?;
        Some(super::tab_drag_preview::viewport_position_to_screen(
            pointer,
            inner_min,
            ctx.pixels_per_point(),
        ))
    })
}

pub(super) fn tab_strip_scroll_alignment(
    target: egui::Rect,
    viewport: egui::Rect,
) -> Option<egui::Align> {
    // Уже видимую вкладку не двигаем; слишком длинную показываем с начала названия.
    if target.width() > viewport.width() {
        Some(egui::Align::Min)
    } else if target.left() < viewport.left() {
        Some(egui::Align::Min)
    } else if target.right() > viewport.right() {
        Some(egui::Align::Max)
    } else {
        None
    }
}

pub(super) fn tab_strip_center_offset(block_width: f32, available_width: f32) -> Option<f32> {
    // Переполненный блок оставляем у начала ScrollArea, чтобы не терять левый край.
    (block_width <= available_width).then(|| (available_width - block_width) * 0.5)
}

fn primary_button_held(ctx: &egui::Context) -> bool {
    if crate::platform::cursor_screen_position().is_some() {
        crate::platform::left_mouse_button_down()
    } else {
        ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary))
    }
}
