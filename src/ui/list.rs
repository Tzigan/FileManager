use crate::core::format::format_size;
use crate::core::fs::{FileEntry, SortKey};
use crate::core::settings::ViewMode;
use crate::platform::format_local_time;
use crate::ui::FileManagerApp;

pub(crate) const ROW_HEIGHT: f32 = 24.0;
pub(crate) const ICON_WIDTH: f32 = 26.0;
const SIZE_COL: f32 = 90.0;
const TYPE_COL: f32 = 110.0;
const MODIFIED_COL: f32 = 140.0;
const COL_GAP: f32 = 8.0;

impl FileManagerApp {
    /// Virtualized file list: view switcher + header + rows in the current mode.
    pub(crate) fn file_list(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (mode, key) in [
                (ViewMode::Table, "view.table"),
                (ViewMode::List, "view.list"),
                (ViewMode::Grid, "view.grid"),
            ] {
                let cur = self.store.settings.view_mode;
                if ui.selectable_label(cur == mode, self.t(key).to_owned()).clicked() {
                    self.set_view(mode);
                }
            }
        });
        ui.separator();

        let count = self.tabs[self.active].entries.len();
        match self.store.settings.view_mode {
            ViewMode::Table => {
                self.list_header(ui);
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_HEIGHT, count, |ui, range| {
                        for index in range {
                            self.file_row(ui, index, ctx);
                        }
                    });
            }
            ViewMode::List => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_HEIGHT, count, |ui, range| {
                        for index in range {
                            self.list_row(ui, index, ctx);
                        }
                    });
            }
            ViewMode::Grid => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.grid_view(ui, ctx));
            }
        }
        self.scroll_to_cursor = false;
    }

    fn column_x(&self, width: f32) -> (f32, f32, f32, f32) {
        let mod_x = width - MODIFIED_COL;
        let type_x = mod_x - TYPE_COL;
        let size_x = type_x - SIZE_COL;
        (ICON_WIDTH, size_x, type_x, mod_x)
    }

    fn list_header(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::hover());
        let (name_x, size_x, type_x, mod_x) = self.column_x(width);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let color = ui.visuals().strong_text_color();
        let painter = ui.painter();
        let y = rect.center().y;
        let (key, desc) = {
            let t = &self.tabs[self.active];
            (t.sort_key, t.sort_desc)
        };

        let cols: [(f32, f32, SortKey, &str); 4] = [
            (name_x, size_x, SortKey::Name, "col.name"),
            (size_x, type_x, SortKey::Size, "col.size"),
            (type_x, mod_x, SortKey::Extension, "col.type"),
            (mod_x, width, SortKey::Modified, "col.modified"),
        ];
        for (x0, x1, k, i18n_key) in cols {
            let arrow = if k == key {
                if desc { " ▼" } else { " ▲" }
            } else {
                ""
            };
            painter.text(
                egui::pos2(rect.min.x + x0, y),
                egui::Align2::LEFT_CENTER,
                format!("{}{arrow}", self.t(i18n_key)),
                font.clone(),
                color,
            );
            let col_rect = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + x0, rect.min.y),
                egui::pos2(rect.min.x + x1, rect.max.y),
            );
            if ui
                .interact(col_rect, ui.id().with(i18n_key), egui::Sense::click())
                .clicked()
            {
                self.sort_col(k);
            }
        }
    }

    fn file_row(&mut self, ui: &mut egui::Ui, index: usize, ctx: &egui::Context) {
        let entry: FileEntry = self.tabs[self.active].entries[index].clone();
        let width = ui.available_width();
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());

        let selected = self.tabs[self.active].selection.contains(&index);
        let focused = self.tabs[self.active].cursor == index;
        let cut = self
            .ops_ui
            .clipboard
            .as_ref()
            .map(|c| c.cut && c.paths.contains(&entry.path))
            .unwrap_or(false);
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.bg_fill);
        }
        if focused {
            painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }

        paint_icon(painter, rect, entry.is_dir);

        let font = egui::TextStyle::Body.resolve(ui.style());
        let visuals = ui.visuals();
        let name_color = if entry.hidden || cut {
            visuals.weak_text_color()
        } else {
            visuals.text_color()
        };
        let weak = visuals.weak_text_color();
        let y = rect.center().y;
        let (name_x, size_x, type_x, mod_x) = self.column_x(width);

        let name_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + name_x, rect.min.y),
            egui::pos2(rect.min.x + size_x - COL_GAP, rect.max.y),
        );
        painter.with_clip_rect(name_rect).text(
            egui::pos2(rect.min.x + name_x, y),
            egui::Align2::LEFT_CENTER,
            &entry.name,
            font.clone(),
            name_color,
        );

        let size_text = if entry.is_dir {
            String::new()
        } else {
            entry.size.map(format_size).unwrap_or_default()
        };
        painter.text(
            egui::pos2(rect.min.x + type_x - COL_GAP, y),
            egui::Align2::RIGHT_CENTER,
            size_text,
            font.clone(),
            weak,
        );

        let type_text = if entry.is_dir {
            self.t("type.folder").to_owned()
        } else {
            match &entry.extension {
                Some(ext) => format!("{} {}", ext.to_uppercase(), self.t("type.file")),
                None => self.t("type.file").to_owned(),
            }
        };
        let type_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + type_x, rect.min.y),
            egui::pos2(rect.min.x + mod_x - COL_GAP, rect.max.y),
        );
        painter.with_clip_rect(type_rect).text(
            egui::pos2(rect.min.x + type_x, y),
            egui::Align2::LEFT_CENTER,
            type_text,
            font.clone(),
            weak,
        );

        let modified = entry.modified.map(format_local_time).unwrap_or_default();
        painter.text(
            egui::pos2(rect.min.x + mod_x, y),
            egui::Align2::LEFT_CENTER,
            modified,
            font,
            weak,
        );

        let _ = focused;
        self.entry_actions(&resp, index, &entry, ui, ctx);
    }

    /// Compact list view: icon + name, no columns.
    fn list_row(&mut self, ui: &mut egui::Ui, index: usize, ctx: &egui::Context) {
        let entry: FileEntry = self.tabs[self.active].entries[index].clone();
        let width = ui.available_width();
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
        let selected = self.tabs[self.active].selection.contains(&index);
        let focused = self.tabs[self.active].cursor == index;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.bg_fill);
        }
        if focused {
            painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        paint_icon(painter, rect, entry.is_dir);
        let font = egui::TextStyle::Body.resolve(ui.style());
        painter.text(
            egui::pos2(rect.min.x + ICON_WIDTH, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &entry.name,
            font,
            ui.visuals().text_color(),
        );
        self.entry_actions(&resp, index, &entry, ui, ctx);
    }

    fn grid_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let cell = 112.0f32;
        let cols = (ui.available_width() / cell).floor().max(1.0) as usize;
        let count = self.tabs[self.active].entries.len();
        let mut index = 0usize;
        egui::Grid::new("file_grid")
            .num_columns(cols)
            .spacing([8.0, 8.0])
            .show(ui, |ui| {
                for _row in 0..count.div_ceil(cols) {
                    for _ in 0..cols {
                        if index >= count {
                            break;
                        }
                        self.grid_tile(ui, index, ctx);
                        index += 1;
                    }
                    ui.end_row();
                }
            });
    }

    fn grid_tile(&mut self, ui: &mut egui::Ui, index: usize, ctx: &egui::Context) {
        let entry: FileEntry = self.tabs[self.active].entries[index].clone();
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(104.0, 68.0), egui::Sense::click());
        let selected = self.tabs[self.active].selection.contains(&index);
        let focused = self.tabs[self.active].cursor == index;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 4.0, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
        }
        if focused {
            painter.rect_stroke(
                rect.shrink(1.0),
                4.0,
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        let icon = if entry.is_dir { "📁" } else { "📄" };
        painter.text(
            egui::pos2(rect.center().x, rect.min.y + 20.0),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::TextStyle::Heading.resolve(ui.style()),
            ui.visuals().text_color(),
        );
        let font = egui::TextStyle::Body.resolve(ui.style());
        painter.with_clip_rect(rect.shrink(4.0)).text(
            egui::pos2(rect.center().x, rect.max.y - 14.0),
            egui::Align2::CENTER_CENTER,
            &entry.name,
            font,
            ui.visuals().text_color(),
        );
        self.entry_actions(&resp, index, &entry, ui, ctx);
    }

    /// Shared row/tile interactions: selection, right-click select, open, context menu.
    fn entry_actions(
        &mut self,
        resp: &egui::Response,
        index: usize,
        entry: &FileEntry,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        if resp.clicked() {
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
            if entry.is_dir {
                self.navigate(self.active, entry.path.clone(), ctx);
            } else {
                let _ = crate::platform::shell_open(&entry.path);
            }
        }
        if self.tabs[self.active].cursor == index && self.scroll_to_cursor {
            ui.scroll_to_rect(resp.rect, None);
        }

        resp.context_menu(|ui| {
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
        });
    }

    /// Click selection honoring Ctrl/Shift modifiers (Explorer semantics).
    fn select_click(&mut self, index: usize, mods: egui::Modifiers) {
        let tab = &mut self.tabs[self.active];
        tab.cursor = super::tabs::apply_click(
            &mut tab.selection,
            &mut tab.anchor,
            index,
            mods.ctrl,
            mods.shift,
        );
    }
}

fn paint_icon(painter: &egui::Painter, row: egui::Rect, is_dir: bool) {
    let icon = egui::Rect::from_center_size(
        row.left_center() + egui::vec2(ICON_WIDTH / 2.0, 0.0),
        egui::vec2(17.0, 15.0),
    );
    if is_dir {
        let back = egui::Color32::from_rgb(226, 152, 52);
        let front = egui::Color32::from_rgb(255, 199, 90);
        let tab = egui::Rect::from_min_max(icon.min, egui::pos2(icon.min.x + 7.0, icon.min.y + 4.0));
        let body = egui::Rect::from_min_max(egui::pos2(icon.min.x, icon.min.y + 4.0), icon.max);
        painter.rect_filled(tab, 1.5, back);
        painter.rect_filled(body, 1.5, front);
    } else {
        let doc = icon.shrink(1.0);
        painter.rect_filled(doc, 1.0, egui::Color32::from_rgb(190, 195, 205));
        painter.rect_stroke(
            doc,
            1.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(130, 135, 150)),
            egui::StrokeKind::Inside,
        );
    }
}
