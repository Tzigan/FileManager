//! Отрисовка плиток сетки и эскизов файлов.

use super::*;

impl FileManagerApp {
    pub(super) fn grid_view(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        visual_order: Option<&[usize]>,
    ) {
        let cell = 112.0f32;
        let cols = (ui.available_width() / cell).floor().max(1.0) as usize;
        let count = self.tabs[self.active].entries.len();
        let row_count = count.div_ceil(cols);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 74.0, row_count, |ui, rows| {
                for row in rows {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = style::SPACE_SM;
                        for col in 0..cols {
                            let index = row * cols + col;
                            if index < count {
                                self.grid_tile(ui, index, ctx, visual_order);
                            }
                        }
                    });
                    ui.add_space(6.0);
                }
            });
    }

    fn grid_tile(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        ctx: &egui::Context,
        visual_order: Option<&[usize]>,
    ) {
        let Some(entry) = entry_at(&self.tabs[self.active].entries, index) else {
            return;
        };
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(104.0, 68.0), egui::Sense::click());
        let selected = self.tabs[self.active].is_selected(&entry);
        let focused = self.tabs[self.active].cursor == index;
        if selected {
            ui.painter()
                .rect_filled(rect, style::RADIUS_ROW, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            ui.painter().rect_filled(
                rect,
                style::RADIUS_ROW,
                ui.visuals().widgets.hovered.bg_fill,
            );
        }
        if focused {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                style::RADIUS_ROW,
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.min.y + 23.0),
            egui::vec2(38.0, 40.0),
        );
        if !self.paint_thumbnail(ui, &entry, icon_rect) {
            icons::paint_entry_icon(
                ui.painter(),
                icon_rect,
                entry.is_dir,
                entry.extension.as_deref(),
                ui.visuals().dark_mode,
            );
        }
        let font = egui::TextStyle::Body.resolve(ui.style());
        ui.painter().with_clip_rect(rect.shrink(4.0)).text(
            egui::pos2(rect.center().x, rect.max.y - 14.0),
            egui::Align2::CENTER_CENTER,
            &entry.name,
            font,
            ui.visuals().text_color(),
        );
        self.entry_actions(&resp, index, &entry, visual_order, ui, ctx);
    }
}
