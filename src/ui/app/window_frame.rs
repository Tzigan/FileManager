use super::*;

pub(super) const WINDOW_FRAME_HEIGHT: f32 = 40.0;
pub(super) const WINDOW_CONTROL_WIDTH: f32 = 46.0;
pub(super) const WINDOW_CONTROL_GAP: f32 = 2.0;
pub(super) const WINDOW_CONTROLS_WIDTH: f32 = WINDOW_CONTROL_WIDTH * 3.0 + WINDOW_CONTROL_GAP * 2.0;
pub(super) const WINDOW_CONTROL_VERTICAL_GAP: f32 = 2.0;
pub(super) const WINDOW_DRAG_AREA_MIN_WIDTH: f32 = 28.0;
pub(super) const MAIN_WINDOW_DRAG_AREA_MIN_WIDTH: f32 = 64.0;
pub(super) const WINDOW_RESIZE_HANDLE_WIDTH: f32 = 6.0;
const MAIN_WINDOW_DRAG_RESIZE_GAP: f32 = 1.0;
const MAIN_WINDOW_DRAG_INSET: f32 = WINDOW_RESIZE_HANDLE_WIDTH + MAIN_WINDOW_DRAG_RESIZE_GAP;
pub(super) const WINDOW_FRAME_SIDE_INSET: i8 = 8;
pub(super) const WINDOW_CONTROL_HEIGHT: f32 =
    WINDOW_FRAME_HEIGHT - WINDOW_RESIZE_HANDLE_WIDTH * 2.0 - WINDOW_CONTROL_VERTICAL_GAP * 2.0;
pub(super) const WINDOW_CONTROL_HOVER_TIME: f32 = 0.14;
pub(super) const WINDOW_CONTROL_PRESS_TIME: f32 = 0.06;
pub(super) const WINDOW_CONTROL_HOVER_BLEND: f32 = 0.72;
pub(super) const WINDOW_CONTROL_CLOSE_HOVER_BLEND: f32 = 0.24;
pub(super) const WINDOW_CONTROL_CLOSE_PRESSED_BLEND: f32 = 0.42;
pub(super) const WINDOW_CONTROL_ICON_SIZE: f32 = 12.0;
pub(super) const WINDOW_CONTROL_RESTORE_ICON_SIZE: f32 = 10.0;
pub(super) const WINDOW_CONTROL_RESTORE_OFFSET: f32 = 2.0;
pub(super) const WINDOW_CONTROL_ICON_STROKE: f32 = 1.4;
pub(super) const WINDOW_CONTROL_CROSS_FACTOR: f32 = 0.7;

pub(super) const TAB_DRAG_THRESHOLD: f32 = 4.0;
pub(super) const TAB_PLUS_BUTTON_WIDTH: f32 = 30.0;
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

pub(super) fn new_tab_button_size(text_height: f32, min_interact_height: f32) -> egui::Vec2 {
    egui::vec2(
        TAB_PLUS_BUTTON_WIDTH,
        (text_height + TAB_VERTICAL_EXTRA_HEIGHT).max(min_interact_height),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowControlIcon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

pub(super) fn maximize_restore_icon(maximized: bool) -> WindowControlIcon {
    if maximized {
        WindowControlIcon::Restore
    } else {
        WindowControlIcon::Maximize
    }
}

pub(super) fn window_control_background(
    visuals: &egui::Visuals,
    icon: WindowControlIcon,
    enabled: bool,
    hover_progress: f32,
    pressed_progress: f32,
) -> egui::Color32 {
    if !enabled {
        return egui::Color32::TRANSPARENT;
    }

    let is_close = icon == WindowControlIcon::Close;
    let hover_color = if is_close {
        visuals
            .panel_fill
            .lerp_to_gamma(visuals.error_fg_color, WINDOW_CONTROL_CLOSE_HOVER_BLEND)
    } else {
        visuals
            .panel_fill
            .lerp_to_gamma(visuals.widgets.hovered.bg_fill, WINDOW_CONTROL_HOVER_BLEND)
    };
    let pressed_color = if is_close {
        visuals
            .panel_fill
            .lerp_to_gamma(visuals.error_fg_color, WINDOW_CONTROL_CLOSE_PRESSED_BLEND)
    } else {
        visuals.widgets.active.bg_fill
    };

    egui::Color32::TRANSPARENT
        .lerp_to_gamma(hover_color, hover_progress.clamp(0.0, 1.0))
        .lerp_to_gamma(pressed_color, pressed_progress.clamp(0.0, 1.0))
}

fn draw_window_control_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: WindowControlIcon,
    color: egui::Color32,
) {
    let center = rect.center();
    let stroke = egui::Stroke::new(WINDOW_CONTROL_ICON_STROKE, color);
    let half_icon = WINDOW_CONTROL_ICON_SIZE * 0.5;

    match icon {
        WindowControlIcon::Minimize => {
            painter.line_segment(
                [
                    egui::pos2(center.x - half_icon, center.y),
                    egui::pos2(center.x + half_icon, center.y),
                ],
                stroke,
            );
        }
        WindowControlIcon::Maximize => {
            draw_window_control_square(
                painter,
                egui::Rect::from_center_size(
                    center,
                    egui::vec2(WINDOW_CONTROL_ICON_SIZE, WINDOW_CONTROL_ICON_SIZE),
                ),
                stroke,
            );
        }
        WindowControlIcon::Restore => {
            let size = egui::vec2(
                WINDOW_CONTROL_RESTORE_ICON_SIZE,
                WINDOW_CONTROL_RESTORE_ICON_SIZE,
            );
            draw_window_control_square(
                painter,
                egui::Rect::from_center_size(
                    center
                        + egui::vec2(
                            WINDOW_CONTROL_RESTORE_OFFSET,
                            -WINDOW_CONTROL_RESTORE_OFFSET,
                        ),
                    size,
                ),
                stroke,
            );
            draw_window_control_square(
                painter,
                egui::Rect::from_center_size(
                    center
                        - egui::vec2(
                            WINDOW_CONTROL_RESTORE_OFFSET,
                            -WINDOW_CONTROL_RESTORE_OFFSET,
                        ),
                    size,
                ),
                stroke,
            );
        }
        WindowControlIcon::Close => {
            let half_cross = half_icon * WINDOW_CONTROL_CROSS_FACTOR;
            painter.line_segment(
                [
                    egui::pos2(center.x - half_cross, center.y - half_cross),
                    egui::pos2(center.x + half_cross, center.y + half_cross),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - half_cross, center.y + half_cross),
                    egui::pos2(center.x + half_cross, center.y - half_cross),
                ],
                stroke,
            );
        }
    }
}

fn draw_window_control_square(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    let top_left = rect.left_top();
    let top_right = rect.right_top();
    let bottom_right = rect.right_bottom();
    let bottom_left = rect.left_bottom();
    painter.line_segment([top_left, top_right], stroke);
    painter.line_segment([top_right, bottom_right], stroke);
    painter.line_segment([bottom_right, bottom_left], stroke);
    painter.line_segment([bottom_left, top_left], stroke);
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DetachedTabWindow {
    pub(super) placed: bool,
    pub(super) confirmed: bool,
    pub(super) initial_screen_position: Option<(i32, i32)>,
    pub(super) initial_pixels_per_point: Option<f32>,
    pub(super) position_requested: bool,
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

pub(super) fn main_window_drag_hit_rect(frame_rect: egui::Rect) -> egui::Rect {
    // Оставляем полосы ресайза сверху и снизу вне зоны перетаскивания.
    frame_rect.shrink2(egui::vec2(0.0, MAIN_WINDOW_DRAG_INSET))
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

fn window_control_button(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    icon: WindowControlIcon,
    tooltip: String,
    enabled: bool,
) -> egui::Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.add_sized(
                egui::vec2(WINDOW_CONTROL_WIDTH, WINDOW_CONTROL_HEIGHT),
                egui::Button::new("").frame(false),
            )
        })
        .inner
        .on_hover_text(tooltip);
    let pressed = enabled && response.is_pointer_button_down_on();
    let hovered = enabled && response.hovered();
    let hover_progress = ctx.animate_bool_with_time(
        response.id.with("window_control_hover"),
        hovered,
        WINDOW_CONTROL_HOVER_TIME,
    );
    let pressed_progress = ctx.animate_bool_with_time(
        response.id.with("window_control_pressed"),
        pressed,
        WINDOW_CONTROL_PRESS_TIME,
    );
    let visuals = ui.visuals();
    let background =
        window_control_background(visuals, icon, enabled, hover_progress, pressed_progress);
    if background.a() > 0 {
        ui.painter()
            .rect_filled(response.rect, style::RADIUS_CONTROL, background);
    }
    let icon_color = if enabled {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    draw_window_control_icon(ui.painter(), response.rect, icon, icon_color);
    response
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

    pub(super) fn request_detach_tab(
        &mut self,
        tab_id: u64,
        initial_screen_position: (i32, i32),
        initial_pixels_per_point: f32,
    ) {
        if self.tabs.iter().any(|tab| tab.id == tab_id) && !self.detached_tabs.contains_key(&tab_id)
        {
            self.detached_tabs
                .insert(
                    tab_id,
                    DetachedTabWindow {
                        initial_screen_position: Some(initial_screen_position),
                        initial_pixels_per_point: Some(initial_pixels_per_point),
                        ..DetachedTabWindow::default()
                    },
                );
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
            if !state.placed
                && (state.initial_screen_position.is_none()
                    || state.initial_pixels_per_point.is_none())
            {
                self.detached_tabs.remove(&tab_id);
                self.tab_detach_error = Some(self.t("tab.detach_failed").to_owned());
                ctx.request_repaint();
                continue;
            }
            let title = self.tabs[index].title();
            let viewport_id = egui::ViewportId::from_hash_of(("filemanager-detached-tab", tab_id));
            let initial_position = state
                .initial_screen_position
                .zip(state.initial_pixels_per_point)
                .map(|((x, y), pixels_per_point)| {
                    let pixels_per_point = pixels_per_point.max(f32::EPSILON);
                    egui::pos2(x as f32 / pixels_per_point, y as f32 / pixels_per_point)
            });
            let builder = detached_viewport_builder_at(
                format!("FileManager — {title}"),
                state.placed,
                initial_position,
            );
            let root_active_id = self.tabs.get(self.active).map(|tab| tab.id);
            let mut callback_ran = false;
            let mut close_requested = false;
            let mut confirmed_now = false;
            let mut persist_after = false;
            let mut position_requested_now = false;
            let mut embedded_viewport = false;
            self.active = index;
            ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, class| {
                callback_ran = true;
                if class == egui::ViewportClass::EmbeddedWindow {
                    embedded_viewport = true;
                    return;
                }
                let viewport_ctx = viewport_ui.ctx().clone();
                if viewport_ctx.input(|input| input.viewport().close_requested()) {
                    viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    close_requested = true;
                    return;
                }

                if !state.position_requested
                    && let Some((x, y)) = state.initial_screen_position
                {
                    let pixels_per_point = viewport_ctx.pixels_per_point().max(f32::EPSILON);
                    viewport_ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                        egui::pos2(
                            x as f32 / pixels_per_point,
                            y as f32 / pixels_per_point,
                        ),
                    ));
                    viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    position_requested_now = true;
                }

                handle_resize_edges(&viewport_ctx);
                self.handle_detached_keys(&viewport_ctx);
                self.handle_dropped_files(&viewport_ctx);
                egui::Panel::top("detached_window_frame")
                    .frame(
                        egui::Frame::new()
                            .fill(viewport_ui.visuals().panel_fill)
                            .inner_margin(egui::Margin::symmetric(WINDOW_FRAME_SIDE_INSET, 0)),
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
                    .resizable(false)
                    .default_size(crate::ui::PROPERTY_PANEL_FIXED_WIDTH)
                    .size_range(crate::ui::property_panel_fixed_range())
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
            } else if embedded_viewport {
                self.detached_tabs.remove(&tab_id);
                self.tab_detach_error = Some(self.t("tab.detach_failed").to_owned());
                ctx.request_repaint();
            } else if callback_ran {
                if let Some(window) = self.detached_tabs.get_mut(&tab_id) {
                    window.placed = true;
                    window.position_requested |= position_requested_now;
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
            self.request_tab_strip_scroll_to_active(ctx);
        } else if !visible.contains(&self.active) {
            self.active = visible[0];
        }
    }

    pub(super) fn window_controls(&self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.allocate_ui_with_layout(
            egui::vec2(WINDOW_CONTROLS_WIDTH, WINDOW_FRAME_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = WINDOW_CONTROL_GAP;
                if window_control_button(
                    ctx,
                    ui,
                    WindowControlIcon::Minimize,
                    self.t("window.minimize").to_owned(),
                    true,
                )
                .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                let maximize_icon = maximize_restore_icon(maximized);
                let maximize_hint = self
                    .t(if maximized {
                        "window.restore"
                    } else {
                        "window.maximize"
                    })
                    .to_owned();
                if window_control_button(ctx, ui, maximize_icon, maximize_hint, true).clicked() {
                    toggle_maximized(ctx);
                }
                if window_control_button(
                    ctx,
                    ui,
                    WindowControlIcon::Close,
                    self.t("window.close").to_owned(),
                    true,
                )
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

pub(super) fn detached_viewport_builder_at(
    title: String,
    already_placed: bool,
    initial_position: Option<egui::Pos2>,
) -> egui::ViewportBuilder {
    let inner_size = egui::vec2(DETACHED_WINDOW_INNER_SIZE[0], DETACHED_WINDOW_INNER_SIZE[1]);
    let mut builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_resizable(true)
        .with_min_inner_size(DETACHED_WINDOW_MIN_SIZE);
    if !already_placed {
        builder = builder.with_inner_size(inner_size);
        if let Some(position) = initial_position {
            builder = builder.with_position(position).with_visible(false);
        }
    }
    builder
}
