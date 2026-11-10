//! Построение строк таблицы и управление группировкой.

use super::*;

/// A row in the table view: either a file entry or a group label.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ListRow {
    Header { key: String, label: String },
    Item(usize),
}

pub(super) fn table_rows(
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
    order.sort_by_cached_key(|&i| {
        let e = &entries[i];
        (
            !e.is_dir,
            e.extension.as_deref().unwrap_or_default().to_lowercase(),
            e.name.to_lowercase(),
        )
    });
    let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
    // Заголовки создаются только по фактически оставшимся записям.
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
        if groups
            .last()
            .is_some_and(|(group_key, _, _)| group_key == &key)
        {
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

pub(super) fn move_group_order(
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

pub(super) fn group_drag_threshold_crossed(press: egui::Pos2, pointer: egui::Pos2) -> bool {
    pointer.distance(press) > 5.0
}

impl FileManagerApp {
    /// Table rows: either a flat item index list, or group headers + items
    /// (folders first, then per-extension, name-sorted inside a group).
    pub(super) fn build_rows(&self) -> Vec<ListRow> {
        let tab = &self.tabs[self.active];
        table_rows(
            &tab.entries,
            // Поиск уже отфильтровал записи в фоне; группируем только оставшиеся элементы.
            self.store.settings.group_by_type,
            &self.store.settings.group_order,
            self.t("type.folder"),
            self.t("type.file"),
        )
    }

    pub(super) fn group_header(
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
        painter.galley(
            egui::pos2(rect.min.x + 4.0, y - galley.size().y / 2.0),
            galley.clone(),
            weak,
        );
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
}
