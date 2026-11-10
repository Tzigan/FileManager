use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const TAB_DRAG_PREVIEW_TITLE_PREFIX: &str = "FileManager Tab Drag Preview";
const TAB_DRAG_PREVIEW_SIZE: egui::Vec2 = egui::vec2(260.0, 52.0);
const TAB_DRAG_PREVIEW_CREATE_RETRIES: u8 = 3;
const TAB_DRAG_PREVIEW_REPAINT_MS: u64 = 16;
static NEXT_TAB_DRAG_PREVIEW_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(super) struct TabDragPreviewState {
    tab_id: Option<u64>,
    viewport_id: Option<egui::ViewportId>,
    window_title: Option<String>,
    window: Option<crate::platform::TabDragPreviewWindow>,
    placed: bool,
    captured: bool,
    create_retries: u8,
    frame_rect: Option<egui::Rect>,
    tab_rects: Vec<(usize, egui::Rect)>,
}

impl TabDragPreviewState {
    pub(super) fn begin(&mut self, tab_id: u64) {
        if self.tab_id == Some(tab_id) {
            return;
        }

        let generation = NEXT_TAB_DRAG_PREVIEW_ID.fetch_add(1, Ordering::Relaxed);
        *self = Self {
            tab_id: Some(tab_id),
            viewport_id: Some(egui::ViewportId::from_hash_of((
                "filemanager-tab-drag-preview",
                std::process::id(),
                generation,
            ))),
            window_title: Some(format!(
                "{TAB_DRAG_PREVIEW_TITLE_PREFIX} {} {generation}",
                std::process::id(),
            )),
            ..Default::default()
        };
    }

    pub(super) fn is_active(&self) -> bool {
        self.tab_id.is_some()
    }

    pub(super) fn pointer_pos(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
        if self.captured {
            let screen_pos = crate::platform::cursor_screen_position()?;
            let inner_rect = ctx.input(|input| input.viewport().inner_rect)?;
            Some(screen_position_to_viewport(
                screen_pos,
                inner_rect.min,
                ctx.pixels_per_point(),
            ))
        } else {
            ctx.pointer_latest_pos()
        }
    }

    pub(super) fn set_drop_geometry(
        &mut self,
        frame_rect: egui::Rect,
        tab_rects: Vec<(usize, egui::Rect)>,
    ) {
        if self.is_active() {
            self.frame_rect = Some(frame_rect);
            self.tab_rects = tab_rects;
        }
    }
}

pub(super) fn screen_position_to_viewport(
    screen_position: (i32, i32),
    viewport_inner_min: egui::Pos2,
    pixels_per_point: f32,
) -> egui::Pos2 {
    let pixels_per_point = pixels_per_point.max(f32::EPSILON);
    // Переводим физические экранные координаты в points именно основного viewport.
    egui::pos2(
        screen_position.0 as f32 / pixels_per_point - viewport_inner_min.x,
        screen_position.1 as f32 / pixels_per_point - viewport_inner_min.y,
    )
}

pub(super) fn preview_window_position(
    cursor_screen_position: (i32, i32),
    cursor_offset_points: egui::Vec2,
    pixels_per_point: f32,
) -> (i32, i32) {
    let pixels_per_point = pixels_per_point.max(f32::EPSILON);
    (
        (cursor_screen_position.0 as f32 - cursor_offset_points.x * pixels_per_point).round()
            as i32,
        (cursor_screen_position.1 as f32 - cursor_offset_points.y * pixels_per_point).round()
            as i32,
    )
}

pub(super) fn tab_drop_slot_at(
    pointer: egui::Pos2,
    tab_rects: &[(usize, egui::Rect)],
) -> Option<usize> {
    tab_rects
        .iter()
        .find(|(_, rect)| rect.contains(pointer))
        .map(|(display_index, rect)| {
            if pointer.x < rect.center().x {
                *display_index
            } else {
                *display_index + 1
            }
        })
}

impl FileManagerApp {
    pub(super) fn show_tab_drag_preview(&mut self, ctx: &egui::Context) {
        let Some(tab_id) = self.tab_drag_preview.tab_id else {
            return;
        };
        let Some(tab_index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            self.cancel_tab_drag_preview(ctx);
            return;
        };
        let Some(viewport_id) = self.tab_drag_preview.viewport_id else {
            self.cancel_tab_drag_preview(ctx);
            return;
        };
        let Some(window_title) = self.tab_drag_preview.window_title.clone() else {
            self.cancel_tab_drag_preview(ctx);
            return;
        };
        let title = self.tabs[tab_index].title();

        if ctx.input(|input| input.viewport().focused == Some(false))
            || ctx.input(|input| input.key_pressed(egui::Key::Escape))
            || crate::platform::escape_key_down()
        {
            self.cancel_tab_drag_preview(ctx);
            return;
        }

        let Some(cursor_screen_position) = crate::platform::cursor_screen_position() else {
            self.cancel_tab_drag_preview(ctx);
            return;
        };
        let button_down = crate::platform::left_mouse_button_down()
            || (!self.tab_drag_preview.captured
                && ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary)));
        if !button_down {
            self.finish_tab_drag(ctx, tab_id, cursor_screen_position);
            return;
        }

        let root_pixels_per_point = ctx.pixels_per_point();
        let cursor_offset = egui::vec2(
            super::window_frame::TAB_DRAG_CHIP_OFFSET_X,
            super::window_frame::TAB_DRAG_CHIP_OFFSET_Y,
        );
        let initial_position = egui::pos2(
            cursor_screen_position.0 as f32 / root_pixels_per_point - cursor_offset.x,
            cursor_screen_position.1 as f32 / root_pixels_per_point - cursor_offset.y,
        );
        let placed = self.tab_drag_preview.placed;
        let mut builder = egui::ViewportBuilder::default()
            .with_title(window_title.clone())
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
            .with_always_on_top()
            .with_transparent(true)
            .with_has_shadow(false);
        if !placed {
            builder = builder
                .with_inner_size(TAB_DRAG_PREVIEW_SIZE)
                .with_position(initial_position);
        }

        let state = &mut self.tab_drag_preview;
        let mut callback_ran = false;
        let mut preview_failed = false;
        ctx.show_viewport_immediate(viewport_id, builder, |preview_ui, _class| {
            callback_ran = true;
            let preview_ctx = preview_ui.ctx().clone();
            if preview_ctx.input(|input| input.viewport().close_requested()) {
                preview_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                preview_failed = true;
                return;
            }

            let window = state
                .window
                .filter(|window| window.is_valid())
                .or_else(|| crate::platform::TabDragPreviewWindow::find(&window_title));
            if let Some(window) = window {
                state.window = Some(window);
                if !state.captured || !window.owns_mouse_capture() {
                    if !window.capture_mouse() {
                        preview_failed = true;
                        return;
                    }
                    state.captured = true;
                }

                let position = preview_window_position(
                    cursor_screen_position,
                    cursor_offset,
                    preview_ctx.pixels_per_point(),
                );
                if !window.set_position(position.0, position.1) {
                    preview_failed = true;
                    return;
                }
            } else {
                state.create_retries = state.create_retries.saturating_add(1);
                if state.create_retries >= TAB_DRAG_PREVIEW_CREATE_RETRIES {
                    preview_failed = true;
                }
            }

            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::TRANSPARENT))
                .show(preview_ui, |ui| {
                    egui::Frame::new()
                        .fill(ui.visuals().extreme_bg_color)
                        .corner_radius(style::RADIUS_ROW)
                        .inner_margin(egui::Margin::symmetric(
                            style::FRAME_PADDING_X,
                            style::FRAME_PADDING_Y,
                        ))
                        .stroke(egui::Stroke::new(
                            super::window_frame::TAB_DRAG_CHIP_STROKE,
                            ui.visuals().selection.stroke.color,
                        ))
                        .show(ui, |ui| {
                            ui.add_sized(
                                egui::vec2(TAB_DRAG_PREVIEW_SIZE.x - 24.0, 22.0),
                                egui::Label::new(title.clone()).truncate(),
                            );
                        });
                });
        });

        if callback_ran {
            self.tab_drag_preview.placed = true;
        } else {
            self.tab_drag_preview.create_retries =
                self.tab_drag_preview.create_retries.saturating_add(1);
            preview_failed |=
                self.tab_drag_preview.create_retries >= TAB_DRAG_PREVIEW_CREATE_RETRIES;
        }
        if preview_failed {
            tracing::debug!("tab drag preview unavailable; canceling drag");
            self.cancel_tab_drag_preview(ctx);
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(TAB_DRAG_PREVIEW_REPAINT_MS));
    }

    fn finish_tab_drag(
        &mut self,
        ctx: &egui::Context,
        tab_id: u64,
        cursor_screen_position: (i32, i32),
    ) {
        let Some(inner_rect) = ctx.input(|input| input.viewport().inner_rect) else {
            self.cancel_tab_drag_preview(ctx);
            return;
        };
        let pointer = screen_position_to_viewport(
            cursor_screen_position,
            inner_rect.min,
            ctx.pixels_per_point(),
        );
        let Some(frame_rect) = self.tab_drag_preview.frame_rect else {
            self.cancel_tab_drag_preview(ctx);
            return;
        };
        if !super::window_frame::release_is_outside_frame(frame_rect, Some(pointer)) {
            if let Some(drop_slot) = tab_drop_slot_at(pointer, &self.tab_drag_preview.tab_rects)
                && let Some(source) = self.tabs.iter().position(|tab| tab.id == tab_id)
            {
                self.move_visible_tab(source, drop_slot);
            }
        } else {
            self.request_detach_tab(tab_id);
        }
        self.clear_tab_drag_preview(ctx);
    }

    fn cancel_tab_drag_preview(&mut self, ctx: &egui::Context) {
        self.clear_tab_drag_preview(ctx);
    }

    fn clear_tab_drag_preview(&mut self, ctx: &egui::Context) {
        if let Some(window) = self.tab_drag_preview.window {
            window.release_mouse_capture();
        }
        if let Some(viewport_id) = self.tab_drag_preview.viewport_id {
            ctx.send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Close);
        }
        self.tab_drag_preview = TabDragPreviewState::default();
        self.tab_drag = None;
        self.tab_press = None;
    }
}
