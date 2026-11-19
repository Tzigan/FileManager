//! Отрисовка табличных строк и компактного списка.

use super::*;

/// В списке размер виден у файла. У папки колонка пустая и в корзине, и в обычной папке.
pub(crate) fn shows_entry_size(is_dir: bool) -> bool {
    !is_dir
}

pub(super) fn entry_at(entries: &[FileEntry], index: usize) -> Option<FileEntry> {
    entries.get(index).cloned()
}

pub(super) fn name_column_text(entry: &FileEntry) -> &str {
    &entry.name
}

impl FileManagerApp {
    pub(super) fn file_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        ctx: &egui::Context,
        visual_order: Option<&[usize]>,
        marquee_rect: Option<egui::Rect>,
    ) {
        let Some(entry) = entry_at(&self.tabs[self.active].entries, index) else {
            return;
        };
        let width = ui.available_width();
        let size = egui::vec2(width, ROW_HEIGHT);
        let (_, rect) = ui.allocate_space(size);
        let resp = ui.interact(
            rect,
            crate::ui::file_dnd::file_row_id(self.tabs[self.active].id, &entry),
            crate::ui::file_dnd::file_row_sense(),
        );

        if marquee_rect.is_some_and(|marquee| marquee.intersects(rect)) {
            let tab = &mut self.tabs[self.active];
            if let Some(marquee) = &mut tab.selection_marquee {
                let key = crate::ui::tabs::add_marquee_entry(&mut tab.selection, marquee, &entry);
                if tab.anchor.is_none() {
                    tab.anchor = Some(key);
                }
                tab.cursor = index;
            }
        }
        let selected = self.tabs[self.active].is_selected(&entry);
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
            painter.rect_filled(
                rect,
                style::RADIUS_ROW,
                ui.visuals().widgets.hovered.bg_fill,
            );
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
            name_column_text(&entry),
            font.clone(),
            name_color,
        );

        if let Some(sx) = size_x {
            // Как в обычной папке: у каталога колонка размера пустая.
            // Сумма из карточки остаётся в подтверждении очистки.
            let size_text = if !shows_entry_size(entry.is_dir) {
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
        self.entry_actions(&resp, index, &entry, visual_order, ui, ctx);
    }

    /// Compact list view: icon + name, no columns.
    pub(super) fn list_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        ctx: &egui::Context,
        visual_order: Option<&[usize]>,
    ) {
        let Some(entry) = entry_at(&self.tabs[self.active].entries, index) else {
            return;
        };
        let width = ui.available_width();
        let size = egui::vec2(width, ROW_HEIGHT);
        let (_, rect) = ui.allocate_space(size);
        let resp = ui.interact(
            rect,
            crate::ui::file_dnd::file_row_id(self.tabs[self.active].id, &entry),
            crate::ui::file_dnd::file_row_sense(),
        );
        let selected = self.tabs[self.active].is_selected(&entry);
        let focused = self.tabs[self.active].cursor == index;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, style::RADIUS_ROW, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(
                rect,
                style::RADIUS_ROW,
                ui.visuals().widgets.hovered.bg_fill,
            );
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
            name_column_text(&entry),
            font,
            ui.visuals().text_color(),
        );
        self.entry_actions(&resp, index, &entry, visual_order, ui, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::name_column_text;
    use crate::core::fs::FileEntry;

    #[test]
    fn secure_trash_name_column_shows_only_file_name() {
        let entry = FileEntry {
            name: "LLamaApp_0.13.0.0_x64.msix".to_owned(),
            path: r"C:\trash\item".into(),
            is_dir: false,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: Some(1),
            modified: None,
            created: None,
            extension: Some("msix".to_owned()),
            archive_member: Some("card-id".to_owned()),
        };
        assert_eq!(name_column_text(&entry), "LLamaApp_0.13.0.0_x64.msix");
        assert!(include_str!("rows.rs").contains("name_column_text(&entry)"));
        assert!(include_str!("grid.rs").contains("rows::name_column_text(&entry)"));
    }
}
