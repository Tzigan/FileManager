use crate::core::format::format_size;
use crate::core::fs::{FileEntry, SortKey};
use crate::core::settings::ViewMode;
use crate::platform::format_local_time;
use crate::ui::icons;
use crate::ui::style;
use crate::ui::FileManagerApp;

/// A row in the table view: either a file entry or a group label.
#[derive(Debug, PartialEq, Eq)]
enum ListRow {
    Header { key: String, label: String },
    Item(usize),
}

#[derive(Debug, PartialEq)]
struct ColumnPositions {
    name_start: f32,
    size_start: Option<f32>,
    type_start: Option<f32>,
    modified_start: Option<f32>,
}

pub(crate) const ROW_HEIGHT: f32 = 24.0;
pub(crate) const ICON_WIDTH: f32 = 26.0;
const SIZE_COL: f32 = 90.0;
const TYPE_COL: f32 = 110.0;
const MODIFIED_COL: f32 = 140.0;
const COL_GAP: f32 = style::SPACE_SM;

fn column_positions(settings: &crate::core::settings::Settings, width: f32) -> ColumnPositions {
    let mut x = width;
    let mut slot = |w: f32, visible: bool| {
        if visible {
            x -= w;
            Some(x)
        } else {
            None
        }
    };
    let modified_start = slot(MODIFIED_COL, settings.col_modified);
    let type_start = slot(TYPE_COL, settings.col_type);
    let size_start = slot(SIZE_COL, settings.col_size);
    ColumnPositions {
        name_start: ICON_WIDTH,
        size_start,
        type_start,
        modified_start,
    }
}

fn column_separator_x(settings: &crate::core::settings::Settings, width: f32) -> Vec<f32> {
    let p = column_positions(settings, width);
    [p.size_start, p.type_start, p.modified_start]
        .into_iter()
        .flatten()
        .map(|start| start - COL_GAP / 2.0)
        .collect()
}

fn paint_column_separators(
    painter: &egui::Painter,
    row: egui::Rect,
    settings: &crate::core::settings::Settings,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.0, color.gamma_multiply(0.45));
    for x in column_separator_x(settings, row.width()) {
        painter.vline(row.min.x + x, row.y_range(), stroke);
    }
}

pub(super) fn next_sort_order(current: SortKey, descending: bool, requested: SortKey) -> (SortKey, bool) {
    if current == requested {
        (current, !descending)
    } else {
        (requested, false)
    }
}

fn table_rows(
    entries: &[FileEntry],
    group: bool,
    preferred_group_order: &[String],
    folders_label: &str,
    files_label: &str,
) -> Vec<ListRow> {
    if !group {
        return (0..entries.len()).map(ListRow::Item).collect();
    }
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&i| {
        let e = &entries[i];
        (
            !e.is_dir,
            e.extension
                .as_deref()
                .unwrap_or_default()
                .to_lowercase(),
            e.name.to_lowercase(),
        )
    });
    let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
    for i in order {
        let e = &entries[i];
        let (key, label) = if e.is_dir {
            ("folder".to_owned(), folders_label.to_owned())
        } else if let Some(extension) = e.extension.as_deref() {
            let extension = extension.to_lowercase();
            (format!("ext:{extension}"), extension.to_uppercase())
        } else {
            ("file".to_owned(), files_label.to_owned())
        };
        if groups.last().is_some_and(|(group_key, _, _)| group_key == &key) {
            groups.last_mut().unwrap().2.push(i);
        } else {
            groups.push((key, label, vec![i]));
        }
    }

    let preferred: std::collections::HashMap<&str, usize> = preferred_group_order
        .iter()
        .enumerate()
        .map(|(index, key)| (key.as_str(), index))
        .collect();
    let group_count = groups.len();
    let mut ranked: Vec<_> = groups.into_iter().enumerate().collect();
    ranked.sort_by_key(|(default_index, (key, _, _))| {
        (
            preferred
                .get(key.as_str())
                .copied()
                .unwrap_or(preferred.len() + default_index),
            *default_index,
        )
    });
    let mut rows = Vec::with_capacity(entries.len() + group_count);
    for (_, (key, label, items)) in ranked {
        rows.push(ListRow::Header { key, label });
        rows.extend(items.into_iter().map(ListRow::Item));
    }
    rows
}

fn move_group_order(
    current_order: &[String],
    source_key: &str,
    target_key: &str,
    after: bool,
) -> Option<Vec<String>> {
    if source_key == target_key {
        return None;
    }
    let mut order = current_order.to_vec();
    let source = order.iter().position(|key| key == source_key)?;
    let group = order.remove(source);
    let target = order.iter().position(|key| key == target_key)?;
    order.insert(target + usize::from(after), group);
    (order != current_order).then_some(order)
}

fn group_drag_threshold_crossed(press: egui::Pos2, pointer: egui::Pos2) -> bool {
    pointer.distance(press) > 5.0
}

fn entry_at(entries: &[FileEntry], index: usize) -> Option<FileEntry> {
    entries.get(index).cloned()
}

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
        // Background hit-test under the whole list: rows drawn later win their
        // rects, empty space falls through to this response.
        let bg = ui.available_rect_before_wrap();
        let bg_resp = ui.interact(bg, ui.scope_id().with("empty_list_area"), egui::Sense::click());
        let mut group_drop: Option<(Vec<String>, String, bool)> = None;
        match self.store.settings.view_mode {
            ViewMode::Table => {
                self.list_header(ui);
                ui.separator();
                let rows = self.build_rows();
                let group_keys: Vec<String> = rows
                    .iter()
                    .filter_map(|row| match row {
                        ListRow::Header { key, .. } => Some(key.clone()),
                        ListRow::Item(_) => None,
                    })
                    .collect();
                let mut drop_target = None;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
                        for index in range {
                            match &rows[index] {
                                ListRow::Item(i) => self.file_row(ui, *i, ctx),
                                ListRow::Header { key, label } => {
                                    self.group_header(ui, key, label, &mut drop_target)
                                }
                            }
                        }
                    });
                if let Some((target, after)) = drop_target {
                    group_drop = Some((group_keys, target, after));
                }
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
        if ctx.input(|i| i.pointer.any_released()) {
            if let Some((source, _)) = self.group_drag.take() {
                if let Some((keys, target, after)) = group_drop {
                    if let Some(order) = move_group_order(&keys, &source, &target, after) {
                        self.store.settings.group_order = order;
                        self.persist();
                    }
                }
            }
            self.group_press = None;
        }
        if let (Some((_, label)), Some(pointer)) =
            (&self.group_drag, ctx.pointer_latest_pos())
        {
            egui::Area::new(ui.scope_id().with("group_drag_chip"))
                .order(egui::Order::Foreground)
                .current_pos(pointer + egui::vec2(12.0, -18.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(ui.visuals().extreme_bg_color)
                        .corner_radius(style::RADIUS_ROW)
                        .inner_margin(egui::Margin::symmetric(
                            style::FRAME_PADDING_X,
                            style::FRAME_PADDING_Y,
                        ))
                        .stroke(egui::Stroke::new(1.0, ui.visuals().selection.stroke.color))
                        .show(ui, |ui| {
                            ui.label(label);
                        });
                });
        }
        self.empty_area_menu(&bg_resp, ctx);
        self.scroll_to_cursor = false;
    }

    /// Column x-origins; hidden columns come out `None`. Visible ones pack right.
    fn column_x(&self, width: f32) -> (f32, Option<f32>, Option<f32>, Option<f32>) {
        let p = column_positions(&self.store.settings, width);
        (p.name_start, p.size_start, p.type_start, p.modified_start)
    }

    fn list_header(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let (rect, hr) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
        let (name_x, size_x, type_x, mod_x) = self.column_x(width);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let color = ui.visuals().strong_text_color();
        let painter = ui.painter();
        let y = rect.center().y;
        let (key, desc) = {
            let t = &self.tabs[self.active];
            (t.sort_key, t.sort_desc)
        };

        let mut cols: Vec<(f32, f32, SortKey, &str)> = Vec::new();
        let name_right = size_x.or(type_x).or(mod_x).unwrap_or(width);
        cols.push((name_x, name_right, SortKey::Name, "col.name"));
        if let Some(sx) = size_x {
            cols.push((sx, type_x.or(mod_x).unwrap_or(width), SortKey::Size, "col.size"));
        }
        if let Some(tx) = type_x {
            cols.push((tx, mod_x.unwrap_or(width), SortKey::Extension, "col.type"));
        }
        if let Some(mx) = mod_x {
            cols.push((mx, width, SortKey::Modified, "col.modified"));
        }
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
            let col_resp = ui.interact(col_rect, ui.scope_id().with(i18n_key), egui::Sense::click());
            if col_resp.clicked() {
                self.sort_col(k);
            }
            self.column_menu(&col_resp);
        }

        paint_column_separators(
            painter,
            rect,
            &self.store.settings,
            ui.visuals().weak_text_color(),
        );

        // Header gaps (right of the last column) get the same menu.
        self.column_menu(&hr);
    }

    /// Table rows: either a flat item index list, or group headers + items
    /// (folders first, then per-extension, name-sorted inside a group).
    fn build_rows(&self) -> Vec<ListRow> {
        table_rows(
            &self.tabs[self.active].entries,
            self.store.settings.group_by_type,
            &self.store.settings.group_order,
            self.t("type.folder"),
            self.t("type.file"),
        )
    }

    fn group_header(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        label: &str,
        drop_target: &mut Option<(String, bool)>,
    ) {
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
        let painter = ui.painter();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let y = rect.center().y;
        let source_is_dragged = self
            .group_drag
            .as_ref()
            .is_some_and(|(source, _)| source == key);
        let weak = ui
            .visuals()
            .weak_text_color()
            .gamma_multiply(if source_is_dragged { 0.3 } else { 1.0 });
        let galley = painter.layout_no_wrap(label.to_owned(), font, weak);
        painter.galley(egui::pos2(rect.min.x + 4.0, y - galley.size().y / 2.0), galley.clone(), weak);
        painter.hline(
            egui::Rangef::new(rect.min.x + galley.size().x + 12.0, rect.max.x - 8.0),
            y,
            egui::Stroke::new(1.0, weak.gamma_multiply(0.3)),
        );

        let pointer = ui.ctx().pointer_latest_pos();
        if response.is_pointer_button_down_on() && self.group_press.is_none() {
            if let Some(pos) = pointer {
                self.group_press = Some((key.to_owned(), label.to_owned(), pos));
            }
        }
        if ui.ctx().input(|i| i.pointer.primary_down()) {
            if let (Some((source, source_label, start)), Some(pos)) = (&self.group_press, pointer) {
                if group_drag_threshold_crossed(*start, pos) {
                    self.group_drag = Some((source.clone(), source_label.clone()));
                }
            }
        }

        if let (Some((source, _)), Some(pos)) = (&self.group_drag, pointer) {
            if rect.contains(pos) {
                let after = pos.y >= rect.center().y;
                if source != key {
                    *drop_target = Some((key.to_owned(), after));
                }
                let line_y = if after { rect.bottom() } else { rect.top() };
                ui.painter().hline(
                    rect.x_range(),
                    line_y,
                    egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                );
            }
        }
    }

    /// Right-click on a header column: visibility toggles (Explorer style).
    fn column_menu(&mut self, resp: &egui::Response) {
        let size_txt = self.t("col.size").to_owned();
        let type_txt = self.t("col.type").to_owned();
        let mod_txt = self.t("col.modified").to_owned();
        let group_txt = self.t("group.by_type").to_owned();
        resp.context_menu(|ui| {
            let s = &mut self.store.settings;
            let mut dirty = false;
            dirty |= ui.checkbox(&mut s.col_size, size_txt).changed();
            dirty |= ui.checkbox(&mut s.col_type, type_txt).changed();
            dirty |= ui.checkbox(&mut s.col_modified, mod_txt).changed();
            ui.separator();
            dirty |= ui.checkbox(&mut s.group_by_type, group_txt).changed();
            if dirty {
                self.persist();
            }
        });
    }

    fn file_row(&mut self, ui: &mut egui::Ui, index: usize, ctx: &egui::Context) {
        let Some(entry) = entry_at(&self.tabs[self.active].entries, index) else {
            return;
        };
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
            painter.rect_filled(rect, style::RADIUS_ROW, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, style::RADIUS_ROW, ui.visuals().widgets.hovered.bg_fill);
        }
        if focused {
            painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }

        let icon_rect = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(ICON_WIDTH / 2.0, 0.0),
            egui::vec2(18.0, 16.0),
        );
        icons::paint_entry_icon(
            painter,
            icon_rect,
            entry.is_dir,
            entry.extension.as_deref(),
            ui.visuals().dark_mode,
        );

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

        let name_right = size_x.or(type_x).or(mod_x).unwrap_or(width);
        let name_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + name_x, rect.min.y),
            egui::pos2(rect.min.x + name_right - COL_GAP, rect.max.y),
        );
        painter.with_clip_rect(name_rect).text(
            egui::pos2(rect.min.x + name_x, y),
            egui::Align2::LEFT_CENTER,
            &entry.name,
            font.clone(),
            name_color,
        );

        if let Some(sx) = size_x {
            let size_text = if entry.is_dir {
                String::new()
            } else {
                entry.size.map(format_size).unwrap_or_default()
            };
            let size_rect = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + sx, rect.min.y),
                egui::pos2(
                    rect.min.x + type_x.or(mod_x).unwrap_or(width) - COL_GAP,
                    rect.max.y,
                ),
            );
            painter.with_clip_rect(size_rect).text(
                egui::pos2(rect.min.x + sx, y),
                egui::Align2::LEFT_CENTER,
                size_text,
                font.clone(),
                weak,
            );
        }

        if let Some(tx) = type_x {
            let type_text = if entry.is_dir {
                self.t("type.folder").to_owned()
            } else {
                match &entry.extension {
                    Some(ext) => format!("{} {}", ext.to_uppercase(), self.t("type.file")),
                    None => self.t("type.file").to_owned(),
                }
            };
            let type_rect = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + tx, rect.min.y),
                egui::pos2(rect.min.x + mod_x.unwrap_or(width) - COL_GAP, rect.max.y),
            );
            painter.with_clip_rect(type_rect).text(
                egui::pos2(rect.min.x + tx, y),
                egui::Align2::LEFT_CENTER,
                type_text,
                font.clone(),
                weak,
            );
        }

        if let Some(mx) = mod_x {
            let language = self.i18n.language();
            let modified = entry
                .modified
                .map(|time| format_local_time(time, language))
                .unwrap_or_default();
            painter.text(
                egui::pos2(rect.min.x + mx, y),
                egui::Align2::LEFT_CENTER,
                modified,
                font,
                weak,
            );
        }
        paint_column_separators(painter, rect, &self.store.settings, weak);

        let _ = focused;
        self.entry_actions(&resp, index, &entry, ui, ctx);
    }

    /// Compact list view: icon + name, no columns.
    fn list_row(&mut self, ui: &mut egui::Ui, index: usize, ctx: &egui::Context) {
        let Some(entry) = entry_at(&self.tabs[self.active].entries, index) else {
            return;
        };
        let width = ui.available_width();
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
        let selected = self.tabs[self.active].selection.contains(&index);
        let focused = self.tabs[self.active].cursor == index;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, style::RADIUS_ROW, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, style::RADIUS_ROW, ui.visuals().widgets.hovered.bg_fill);
        }
        if focused {
            painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        let icon_rect = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(ICON_WIDTH / 2.0, 0.0),
            egui::vec2(18.0, 16.0),
        );
        icons::paint_entry_icon(
            painter,
            icon_rect,
            entry.is_dir,
            entry.extension.as_deref(),
            ui.visuals().dark_mode,
        );
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
            .spacing([style::SPACE_SM, style::SPACE_SM])
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
        let Some(entry) = entry_at(&self.tabs[self.active].entries, index) else {
            return;
        };
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(104.0, 68.0), egui::Sense::click());
        let selected = self.tabs[self.active].selection.contains(&index);
        let focused = self.tabs[self.active].cursor == index;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, style::RADIUS_ROW, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, style::RADIUS_ROW, ui.visuals().widgets.hovered.bg_fill);
        }
        if focused {
            painter.rect_stroke(
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
        icons::paint_entry_icon(
            painter,
            icon_rect,
            entry.is_dir,
            entry.extension.as_deref(),
            ui.visuals().dark_mode,
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

    /// Empty space below the rows: left-click clears the selection, right-click
    /// shows the folder menu (Explorer semantics).
    fn empty_area_menu(&mut self, resp: &egui::Response, ctx: &egui::Context) {
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
        resp.context_menu(|ui| {
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
            ui.separator();
            if ui.button(self.t("toolbar.refresh").to_owned()).clicked() {
                self.refresh(ctx);
                ui.close();
            }
        });
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
            if ui.button(self.t("ctx.open").to_owned()).clicked() {
                if entry.is_dir {
                    self.navigate(self.active, entry.path.clone(), ctx);
                } else {
                    let _ = crate::platform::shell_open(&entry.path);
                }
                ui.close();
            }
            if entry.is_dir && ui.button(self.t("ctx.open_new_tab").to_owned()).clicked() {
                self.queue_open_path_in_new_tab(entry.path.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, is_dir: bool, extension: Option<&str>) -> FileEntry {
        FileEntry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: false,
            hidden: false,
            readonly: false,
            size: None,
            modified: None,
            created: None,
            extension: extension.map(str::to_owned),
        }
    }

    #[test]
    fn ungrouped_table_keeps_all_rows_in_input_order() {
        let entries = vec![
            entry("z.txt", false, Some("txt")),
            entry("folder", true, None),
            entry("a.md", false, Some("md")),
        ];
        assert_eq!(
            table_rows(&entries, false, &[], "Folders", "Files"),
            vec![ListRow::Item(0), ListRow::Item(1), ListRow::Item(2)]
        );
    }

    #[test]
    fn grouped_table_keeps_every_item_once_and_groups_folders_then_extensions() {
        let entries = vec![
            entry("z.txt", false, Some("txt")),
            entry("B folder", true, None),
            entry("readme", false, None),
            entry("A folder", true, None),
            entry("a.md", false, Some("md")),
            entry("a.txt", false, Some("txt")),
        ];
        let rows = table_rows(&entries, true, &[], "Folders", "Files");
        assert_eq!(
            rows,
            vec![
                ListRow::Header { key: "folder".into(), label: "Folders".into() },
                ListRow::Item(3),
                ListRow::Item(1),
                ListRow::Header { key: "file".into(), label: "Files".into() },
                ListRow::Item(2),
                ListRow::Header { key: "ext:md".into(), label: "MD".into() },
                ListRow::Item(4),
                ListRow::Header { key: "ext:txt".into(), label: "TXT".into() },
                ListRow::Item(5),
                ListRow::Item(0),
            ]
        );

        let mut seen: Vec<usize> = rows
            .iter()
            .filter_map(|r| match r {
                ListRow::Item(i) => Some(*i),
                ListRow::Header { .. } => None,
            })
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..entries.len()).collect::<Vec<_>>());
    }

    #[test]
    fn grouping_merges_extension_case_and_handles_empty_directory() {
        let entries = vec![
            entry("UPPER.TXT", false, Some("TXT")),
            entry("lower.txt", false, Some("txt")),
        ];
        assert_eq!(
            table_rows(&entries, true, &[], "Folders", "Files"),
            vec![
                ListRow::Header { key: "ext:txt".into(), label: "TXT".into() },
                ListRow::Item(1),
                ListRow::Item(0),
            ]
        );
        assert!(table_rows(&[], true, &[], "Folders", "Files").is_empty());
    }

    #[test]
    fn custom_group_order_overrides_default_and_new_types_keep_default_tail_order() {
        let entries = vec![
            entry("folder", true, None),
            entry("readme", false, None),
            entry("a.md", false, Some("md")),
            entry("b.txt", false, Some("txt")),
        ];
        let rows = table_rows(
            &entries,
            true,
            &["ext:txt".into(), "folder".into()],
            "Folders",
            "Files",
        );
        let headers: Vec<(&str, &str)> = rows
            .iter()
            .filter_map(|row| match row {
                ListRow::Header { key, label } => Some((key.as_str(), label.as_str())),
                ListRow::Item(_) => None,
            })
            .collect();
        assert_eq!(headers, [("ext:txt", "TXT"), ("folder", "Folders"), ("file", "Files"), ("ext:md", "MD")]);
    }

    #[test]
    fn moving_a_group_changes_only_group_order() {
        let keys = vec!["folder".into(), "file".into(), "ext:md".into(), "ext:txt".into()];
        assert_eq!(
            move_group_order(&keys, "ext:txt", "folder", false),
            Some(vec!["ext:txt", "folder", "file", "ext:md"].into_iter().map(str::to_owned).collect())
        );
        assert_eq!(move_group_order(&keys, "file", "file", true), None);
        assert_eq!(move_group_order(&keys, "missing", "folder", false), None);
    }

    #[test]
    fn group_drag_starts_only_after_a_real_pointer_move() {
        let press = egui::pos2(100.0, 20.0);
        assert!(!group_drag_threshold_crossed(press, egui::pos2(104.0, 20.0)));
        assert!(!group_drag_threshold_crossed(press, egui::pos2(100.0, 25.0)));
        assert!(group_drag_threshold_crossed(press, egui::pos2(106.0, 20.0)));
    }

    #[test]
    fn hidden_table_columns_leave_remaining_columns_packed_without_overlap() {
        let settings = crate::core::settings::Settings::default();
        let all = column_positions(&settings, 800.0);
        assert_eq!(all.size_start, Some(460.0));
        assert_eq!(all.type_start, Some(550.0));
        assert_eq!(all.modified_start, Some(660.0));

        let partial = crate::core::settings::Settings {
            col_size: false,
            ..settings.clone()
        };
        let p = column_positions(&partial, 800.0);
        assert_eq!(p.size_start, None);
        assert_eq!(p.type_start, Some(550.0));
        assert_eq!(p.modified_start, Some(660.0));

        let none = crate::core::settings::Settings {
            col_size: false,
            col_type: false,
            col_modified: false,
            ..settings
        };
        let p = column_positions(&none, 800.0);
        assert_eq!(p.size_start, None);
        assert_eq!(p.type_start, None);
        assert_eq!(p.modified_start, None);
        assert_eq!(p.name_start, ICON_WIDTH);
    }

    #[test]
    fn selecting_a_sort_column_starts_ascending_and_repeated_clicks_toggle() {
        let (key, desc) = next_sort_order(SortKey::Name, false, SortKey::Size);
        assert_eq!(key, SortKey::Size);
        assert!(!desc);

        let (key, desc) = next_sort_order(key, desc, SortKey::Size);
        assert_eq!(key, SortKey::Size);
        assert!(desc);

        let (key, desc) = next_sort_order(key, desc, SortKey::Modified);
        assert_eq!(key, SortKey::Modified);
        assert!(!desc);
    }

    #[test]
    fn separators_follow_only_visible_column_boundaries() {
        let all = crate::core::settings::Settings::default();
        assert_eq!(column_separator_x(&all, 800.0), vec![456.0, 546.0, 656.0]);

        let without_size = crate::core::settings::Settings {
            col_size: false,
            ..all.clone()
        };
        assert_eq!(column_separator_x(&without_size, 800.0), vec![546.0, 656.0]);

        let without_type = crate::core::settings::Settings {
            col_type: false,
            ..all.clone()
        };
        assert_eq!(column_separator_x(&without_type, 800.0), vec![566.0, 656.0]);

        let no_optional_columns = crate::core::settings::Settings {
            col_size: false,
            col_type: false,
            col_modified: false,
            ..all
        };
        assert!(column_separator_x(&no_optional_columns, 800.0).is_empty());
    }

    #[test]
    fn stale_virtual_row_index_is_ignored_safely() {
        let entries = vec![entry("still valid.txt", false, Some("txt"))];
        assert_eq!(entry_at(&entries, 0).unwrap().name, "still valid.txt");
        assert!(entry_at(&entries, 4).is_none());
        assert!(entry_at(&[], 0).is_none());
    }
}
