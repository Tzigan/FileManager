//! Расчёт ширины, заголовков и разделителей колонок.

use super::*;

#[derive(Debug, PartialEq)]
pub(super) struct ColumnPositions {
    pub(super) name_start: f32,
    pub(super) size_start: Option<f32>,
    pub(super) type_start: Option<f32>,
    pub(super) modified_start: Option<f32>,
}

const SIZE_COL: f32 = 90.0;
const TYPE_COL: f32 = 110.0;
const MODIFIED_COL: f32 = 140.0;
pub(super) const COL_GAP: f32 = style::SPACE_SM;

pub(super) fn column_positions(
    settings: &crate::core::settings::Settings,
    width: f32,
) -> ColumnPositions {
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

pub(super) fn column_separator_x(
    settings: &crate::core::settings::Settings,
    width: f32,
) -> Vec<f32> {
    let p = column_positions(settings, width);
    [p.size_start, p.type_start, p.modified_start]
        .into_iter()
        .flatten()
        .map(|start| start - COL_GAP / 2.0)
        .collect()
}

pub(super) fn table_rows_content_margin() -> egui::Margin {
    egui::Margin::symmetric(0, style::SPACE_SM as i8)
}

pub(super) fn paint_column_separators(
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

pub(in crate::ui) fn next_sort_order(
    current: SortKey,
    descending: bool,
    requested: SortKey,
) -> (SortKey, bool) {
    if current == requested {
        (current, !descending)
    } else {
        (requested, false)
    }
}

impl FileManagerApp {
    /// Column x-origins; hidden columns come out `None`. Visible ones pack right.
    pub(super) fn column_x(&self, width: f32) -> (f32, Option<f32>, Option<f32>, Option<f32>) {
        let p = column_positions(&self.store.settings, width);
        (p.name_start, p.size_start, p.type_start, p.modified_start)
    }

    pub(super) fn list_header(&mut self, ui: &mut egui::Ui) {
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
            cols.push((
                sx,
                type_x.or(mod_x).unwrap_or(width),
                SortKey::Size,
                "col.size",
            ));
        }
        if let Some(tx) = type_x {
            cols.push((tx, mod_x.unwrap_or(width), SortKey::Extension, "col.type"));
        }
        if let Some(mx) = mod_x {
            let modified_key = if self.tabs[self.active].secure_trash {
                "trash.col.deleted"
            } else {
                "col.modified"
            };
            cols.push((mx, width, SortKey::Modified, modified_key));
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
            let col_resp =
                ui.interact(col_rect, ui.scope_id().with(i18n_key), egui::Sense::click());
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

    /// Right-click on a header column: visibility toggles (Explorer style).
    fn column_menu(&mut self, resp: &egui::Response) {
        let size_txt = self.t("col.size").to_owned();
        let type_txt = self.t("col.type").to_owned();
        let mod_txt = self.t("col.modified").to_owned();
        let group_txt = self.t("group.by_type").to_owned();
        resp.context_menu(|ui| {
            style::compact_context_menu(ui);
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
}
