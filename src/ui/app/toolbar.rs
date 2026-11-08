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
        ui.horizontal(|ui| {
            let mut act: Option<(&str, usize)> = None;
            let mut drop_dst = None;
            let mut drag_src = None;
            let mut dragging_tab =
                is_tab_dragging(self.tab_drag, self.tab_press, ui.ctx().pointer_latest_pos());
            let metas: Vec<(bool, String)> =
                self.tabs.iter().map(|t| (t.pinned, t.title())).collect();

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
                                let x_rect = egui::Rect::from_min_max(
                                    egui::pos2(rect.max.x - close_w, rect.min.y),
                                    rect.max,
                                );
                                let over_x = tab_close_button_hovered(
                                    dragging_tab,
                                    *pinned,
                                    x_rect,
                                    ui.ctx().pointer_latest_pos(),
                                );
                                let visuals = ui.visuals();
                                let painter = ui.painter();
                                if i == self.active {
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
                                    let pin_txt = self
                                        .t(if *pinned { "tab.unpin" } else { "tab.pin" })
                                        .to_owned();
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
                                                egui::Stroke::new(
                                                    2.0,
                                                    ui.visuals().selection.stroke.color,
                                                ),
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
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        ui.visuals().selection.stroke.color,
                                    ))
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
}
