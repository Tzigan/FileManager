use super::*;

pub(super) const WINDOW_FRAME_HEIGHT: f32 = 40.0;
pub(super) const WINDOW_CONTROL_WIDTH: f32 = 46.0;
pub(super) const WINDOW_CONTROLS_WIDTH: f32 = WINDOW_CONTROL_WIDTH * 3.0;
pub(super) const WINDOW_CONTROL_VERTICAL_GAP: f32 = 2.0;
pub(super) const WINDOW_DRAG_AREA_MIN_WIDTH: f32 = 28.0;
pub(super) const WINDOW_RESIZE_HANDLE_WIDTH: f32 = 6.0;

pub(super) const TAB_DRAG_THRESHOLD: f32 = 5.0;
pub(super) const TAB_PLUS_AREA_WIDTH: f32 = 30.0;
pub(super) const TAB_PIN_WIDTH: f32 = 16.0;
pub(super) const TAB_PIN_ICON_SIZE: f32 = 12.0;
pub(super) const TAB_PIN_ICON_OFFSET: f32 = 4.0;
pub(super) const TAB_CLOSE_WIDTH: f32 = 18.0;
pub(super) const TAB_HORIZONTAL_PADDING: f32 = 7.0;
pub(super) const TAB_TEXT_EXTRA_WIDTH: f32 = 14.0;
pub(super) const TAB_VERTICAL_EXTRA_HEIGHT: f32 = 6.0;
pub(super) const TAB_INSERTION_STROKE: f32 = 2.0;
pub(super) const TAB_CLOSE_CROSS_RADIUS: f32 = 4.0;
pub(super) const TAB_CLOSE_CROSS_STROKE: f32 = 1.4;
pub(super) const TAB_CLOSE_HIGHLIGHT_INSET: f32 = 2.0;
pub(super) const TAB_DRAG_CHIP_OFFSET_X: f32 = 10.0;
pub(super) const TAB_DRAG_CHIP_OFFSET_Y: f32 = -16.0;
pub(super) const TAB_DRAG_CHIP_STROKE: f32 = 1.0;
pub(super) const DETACHED_WINDOW_INNER_SIZE: [f32; 2] = [1000.0, 640.0];
pub(super) const DETACHED_WINDOW_MIN_SIZE: [f32; 2] = [640.0, 420.0];
pub(super) const DETACHED_TAB_MIN_WIDTH: f32 = 100.0;
pub(super) const DETACHED_TREE_DEFAULT_WIDTH: f32 = 210.0;
pub(super) const DETACHED_TREE_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 140.0..=360.0;
pub(super) const DETACHED_PROPS_DEFAULT_WIDTH: f32 = 340.0;
pub(super) const DETACHED_PROPS_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 260.0..=460.0;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DetachedTabWindow {
    pub(super) placed: bool,
    pub(super) confirmed: bool,
}

pub(super) fn resize_direction_at(
    window_rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<egui::viewport::ResizeDirection> {
    if !window_rect.contains(pointer) {
        return None;
    }
    let left = pointer.x <= window_rect.left() + WINDOW_RESIZE_HANDLE_WIDTH;
    let right = pointer.x >= window_rect.right() - WINDOW_RESIZE_HANDLE_WIDTH;
    let top = pointer.y <= window_rect.top() + WINDOW_RESIZE_HANDLE_WIDTH;
    let bottom = pointer.y >= window_rect.bottom() - WINDOW_RESIZE_HANDLE_WIDTH;
    match (top, right, bottom, left) {
        (true, _, _, true) => Some(egui::viewport::ResizeDirection::NorthWest),
        (true, true, _, _) => Some(egui::viewport::ResizeDirection::NorthEast),
        (_, true, true, _) => Some(egui::viewport::ResizeDirection::SouthEast),
        (_, _, true, true) => Some(egui::viewport::ResizeDirection::SouthWest),
        (true, _, _, _) => Some(egui::viewport::ResizeDirection::North),
        (_, true, _, _) => Some(egui::viewport::ResizeDirection::East),
        (_, _, true, _) => Some(egui::viewport::ResizeDirection::South),
        (_, _, _, true) => Some(egui::viewport::ResizeDirection::West),
        _ => None,
    }
}

pub(super) fn release_is_outside_frame(
    frame_rect: egui::Rect,
    pointer: Option<egui::Pos2>,
) -> bool {
    pointer.is_none_or(|position| !frame_rect.contains(position))
}

pub(super) fn handle_resize_edges(ctx: &egui::Context) {
    let (maximized, press_origin) = ctx.input(|input| {
        (
            input.viewport().maximized.unwrap_or(false),
            input
                .pointer
                .button_pressed(egui::PointerButton::Primary)
                .then(|| input.pointer.press_origin())
                .flatten(),
        )
    });
    if maximized {
        return;
    }
    let Some(pointer) = press_origin else {
        return;
    };
    let Some(direction) = resize_direction_at(ctx.content_rect(), pointer) else {
        return;
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
}

pub(super) fn toggle_maximized(ctx: &egui::Context) {
    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
}

impl FileManagerApp {
    pub(super) fn is_tab_detached(&self, tab_id: u64) -> bool {
        self.detached_tabs
            .get(&tab_id)
            .is_some_and(|window| window.confirmed)
    }

    pub(super) fn visible_tab_indices(&self) -> Vec<usize> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| (!self.is_tab_detached(tab.id)).then_some(index))
            .collect()
    }

    pub(super) fn request_detach_tab(&mut self, tab_id: u64) {
        if self.tabs.iter().any(|tab| tab.id == tab_id) && !self.detached_tabs.contains_key(&tab_id)
        {
            self.detached_tabs
                .insert(tab_id, DetachedTabWindow::default());
            self.tab_detach_error = None;
        }
    }

    pub(super) fn show_detached_tab_windows(&mut self, ctx: &egui::Context) {
        // Окно показывает тот же объект Tab, поэтому история, поиск и выделение не копируются.
        let tab_ids: Vec<u64> = self.detached_tabs.keys().copied().collect();
        for tab_id in tab_ids {
            let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
                self.detached_tabs.remove(&tab_id);
                continue;
            };
            let state = self.detached_tabs.get(&tab_id).copied().unwrap_or_default();
            let title = self.tabs[index].title();
            let viewport_id = egui::ViewportId::from_hash_of(("filemanager-detached-tab", tab_id));
            let builder =
                detached_viewport_builder(ctx, format!("FileManager — {title}"), state.placed);
            let root_active_id = self.tabs.get(self.active).map(|tab| tab.id);
            let mut callback_ran = false;
            let mut close_requested = false;
            let mut confirmed_now = false;
            let mut persist_after = false;
            self.active = index;
            ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, _class| {
                callback_ran = true;
                let viewport_ctx = viewport_ui.ctx().clone();
                if viewport_ctx.input(|input| input.viewport().close_requested()) {
                    viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    close_requested = true;
                    return;
                }

                handle_resize_edges(&viewport_ctx);
                self.handle_detached_keys(&viewport_ctx);
                self.handle_dropped_files(&viewport_ctx);
                egui::Panel::top("detached_window_frame")
                    .frame(
                        egui::Frame::new()
                            .fill(viewport_ui.visuals().panel_fill)
                            .inner_margin(egui::Margin::symmetric(
                                WINDOW_RESIZE_HANDLE_WIDTH as i8,
                                0,
                            )),
                    )
                    .show(viewport_ui, |ui| {
                        self.detached_titlebar(&viewport_ctx, ui, tab_id, &title);
                    });
                egui::Panel::top("detached_menu_bar").show(viewport_ui, |ui| self.menu_bar(ui));
                egui::Panel::top("detached_toolbar")
                    .show(viewport_ui, |ui| self.toolbar(&viewport_ctx, ui));
                egui::Panel::top("detached_search").show(viewport_ui, |ui| self.search_bar(ui));
                self.dispatch_due_searches(&viewport_ctx);
                egui::Panel::left("detached_tree")
                    .resizable(true)
                    .default_size(DETACHED_TREE_DEFAULT_WIDTH)
                    .size_range(DETACHED_TREE_WIDTH_RANGE)
                    .show(viewport_ui, |ui| self.tree_panel(&viewport_ctx, ui));
                egui::Panel::right("detached_props")
                    .resizable(true)
                    .default_size(DETACHED_PROPS_DEFAULT_WIDTH)
                    .size_range(DETACHED_PROPS_WIDTH_RANGE)
                    .show(viewport_ui, |ui| self.props_panel(ui));
                egui::Panel::bottom("detached_status").show(viewport_ui, |ui| {
                    self.show_active_tab_status(ui);
                });
                egui::CentralPanel::default().show(viewport_ui, |ui| {
                    self.show_active_file_content(&viewport_ctx, ui)
                });
                self.flush_pending_open_tabs(&viewport_ctx);
            });

            if close_requested {
                self.detached_tabs.remove(&tab_id);
                persist_after = true;
                ctx.request_repaint();
            } else if callback_ran {
                if let Some(window) = self.detached_tabs.get_mut(&tab_id) {
                    window.placed = true;
                    confirmed_now = !window.confirmed;
                    window.confirmed = true;
                } else {
                    persist_after = true;
                }
            } else {
                self.detached_tabs.remove(&tab_id);
                self.tab_detach_error = Some(self.t("tab.detach_failed").to_owned());
                ctx.request_repaint();
            }

            self.active = root_active_id
                .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
                .unwrap_or_else(|| self.visible_tab_indices().first().copied().unwrap_or(0));
            self.ensure_visible_root_tab(ctx);
            if confirmed_now || persist_after {
                self.persist_tabs();
                ctx.request_repaint();
            }
        }
    }

    fn ensure_visible_root_tab(&mut self, ctx: &egui::Context) {
        let visible = self.visible_tab_indices();
        if visible.is_empty() {
            let path = directories::UserDirs::new()
                .map(|dirs| dirs.home_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            self.tabs.push(Tab::new(path));
            self.active = self.tabs.len() - 1;
            self.navigate_fresh(self.active, ctx);
        } else if !visible.contains(&self.active) {
            self.active = visible[0];
        }
    }

    pub(super) fn window_controls(&self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.allocate_ui_with_layout(
            egui::vec2(WINDOW_CONTROLS_WIDTH, WINDOW_FRAME_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let button_height = WINDOW_FRAME_HEIGHT
                    - WINDOW_RESIZE_HANDLE_WIDTH * 2.0
                    - WINDOW_CONTROL_VERTICAL_GAP * 2.0;
                if ui
                    .add_sized(
                        egui::vec2(WINDOW_CONTROL_WIDTH, button_height),
                        egui::Button::new("−").frame(false),
                    )
                    .on_hover_text(self.t("window.minimize"))
                    .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                let maximize_label = if maximized { "❐" } else { "□" };
                let maximize_hint = if maximized {
                    self.t("window.restore")
                } else {
                    self.t("window.maximize")
                };
                if ui
                    .add_sized(
                        egui::vec2(WINDOW_CONTROL_WIDTH, button_height),
                        egui::Button::new(maximize_label).frame(false),
                    )
                    .on_hover_text(maximize_hint)
                    .clicked()
                {
                    toggle_maximized(ctx);
                }
                if ui
                    .add_sized(
                        egui::vec2(WINDOW_CONTROL_WIDTH, button_height),
                        egui::Button::new("×").frame(false),
                    )
                    .on_hover_text(self.t("window.close"))
                    .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
        );
    }

    pub(super) fn detached_titlebar(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        tab_id: u64,
        title: &str,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let pinned = self.tabs[index].pinned;
        let mut action = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = style::SPACE_XS;
            let galley = ui.painter().layout_no_wrap(
                title.to_owned(),
                egui::TextStyle::Button.resolve(ui.style()),
                ui.visuals().text_color(),
            );
            let pin_width = if pinned { TAB_PIN_WIDTH } else { 0.0 };
            let close_width = if pinned { 0.0 } else { TAB_CLOSE_WIDTH };
            let desired_size = galley.size()
                + egui::vec2(
                    TAB_TEXT_EXTRA_WIDTH + pin_width + close_width,
                    TAB_VERTICAL_EXTRA_HEIGHT,
                );
            let max_tab_width = (ui.available_width()
                - WINDOW_CONTROLS_WIDTH
                - WINDOW_DRAG_AREA_MIN_WIDTH
                - ui.spacing().item_spacing.x * 2.0)
                .max(DETACHED_TAB_MIN_WIDTH);
            let size = egui::vec2(desired_size.x.min(max_tab_width), desired_size.y);
            let (tab_rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            let response = response.on_hover_text(title);
            let close_rect = egui::Rect::from_min_max(
                egui::pos2(tab_rect.max.x - close_width, tab_rect.min.y),
                tab_rect.max,
            );
            let visuals = ui.visuals();
            ui.painter()
                .rect_filled(tab_rect, style::RADIUS_ROW, visuals.selection.bg_fill);
            let color = visuals.strong_text_color();
            let text_left = tab_rect.left() + TAB_HORIZONTAL_PADDING + pin_width;
            let text_right = tab_rect.right() - TAB_HORIZONTAL_PADDING - close_width;
            let text_rect = egui::Rect::from_min_max(
                egui::pos2(text_left, tab_rect.min.y),
                egui::pos2(text_right, tab_rect.max.y),
            );
            ui.painter().with_clip_rect(text_rect).galley(
                egui::pos2(
                    (text_left + text_right - galley.size().x) * 0.5,
                    tab_rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                color,
            );
            if pinned {
                let pin_size = TAB_PIN_ICON_SIZE;
                icon_pin(
                    ui.painter(),
                    egui::Rect::from_min_size(
                        egui::pos2(
                            tab_rect.left() + TAB_PIN_ICON_OFFSET,
                            tab_rect.center().y - pin_size * 0.5,
                        ),
                        egui::vec2(pin_size, pin_size),
                    ),
                    color,
                );
            } else {
                let cross = TAB_CLOSE_CROSS_RADIUS;
                let close_hovered = response
                    .interact_pointer_pos()
                    .is_some_and(|position| close_rect.contains(position));
                if close_hovered {
                    ui.painter().rect_filled(
                        close_rect.shrink(TAB_CLOSE_HIGHLIGHT_INSET),
                        style::RADIUS_ROW,
                        visuals.error_fg_color.gamma_multiply(0.25),
                    );
                }
                let close_color = if close_hovered {
                    visuals.error_fg_color
                } else {
                    visuals.weak_text_color()
                };
                ui.painter().line_segment(
                    [
                        close_rect.center() + egui::vec2(-cross, -cross),
                        close_rect.center() + egui::vec2(cross, cross),
                    ],
                    egui::Stroke::new(TAB_CLOSE_CROSS_STROKE, close_color),
                );
                ui.painter().line_segment(
                    [
                        close_rect.center() + egui::vec2(-cross, cross),
                        close_rect.center() + egui::vec2(cross, -cross),
                    ],
                    egui::Stroke::new(TAB_CLOSE_CROSS_STROKE, close_color),
                );
                if response.clicked()
                    && response
                        .interact_pointer_pos()
                        .is_some_and(|position| close_rect.contains(position))
                {
                    action = Some("close");
                }
                if response.middle_clicked() {
                    action = Some("close");
                }
            }
            response.context_menu(|ui| {
                style::compact_context_menu(ui);
                if !pinned && ui.button(self.t("tab.close").to_owned()).clicked() {
                    action = Some("close");
                    ui.close();
                }
                if ui.button(self.t("tab.duplicate").to_owned()).clicked() {
                    action = Some("duplicate");
                    ui.close();
                }
                let pin_label = if pinned { "tab.unpin" } else { "tab.pin" };
                if ui.button(self.t(pin_label).to_owned()).clicked() {
                    action = Some("pin");
                    ui.close();
                }
            });
            if response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            let drag_width =
                (ui.available_width() - WINDOW_CONTROLS_WIDTH - ui.spacing().item_spacing.x)
                    .max(0.0);
            let (drag_rect, drag_response) = ui.allocate_exact_size(
                egui::vec2(drag_width, WINDOW_FRAME_HEIGHT),
                egui::Sense::click_and_drag(),
            );
            let _ = drag_rect;
            if drag_response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag_response.double_clicked() {
                toggle_maximized(ctx);
            }
            self.window_controls(ctx, ui);
        });
        match action {
            Some("close") => self.close_detached_tab(tab_id, ctx),
            Some("duplicate") => self.duplicate_tab(index, ctx),
            Some("pin") => self.toggle_pin(index),
            _ => {}
        }
    }
}

pub(super) fn detached_viewport_builder(
    ctx: &egui::Context,
    title: String,
    already_placed: bool,
) -> egui::ViewportBuilder {
    let inner_size = egui::vec2(DETACHED_WINDOW_INNER_SIZE[0], DETACHED_WINDOW_INNER_SIZE[1]);
    let mut builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_resizable(true)
        .with_min_inner_size(DETACHED_WINDOW_MIN_SIZE);
    if !already_placed {
        let parent_rect = ctx
            .input(|input| input.viewport().outer_rect)
            .unwrap_or_else(|| ctx.content_rect());
        builder = builder
            .with_inner_size(inner_size)
            .with_position(parent_rect.center() - inner_size / 2.0);
    }
    builder
}
