//! Построение строк таблицы и управление группировкой.

use super::*;

/// A row in the table view: either a file entry or a group label.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ListRow {
    Header { key: String, label: String },
    Item(usize),
}

pub(super) fn item_order(rows: &[ListRow]) -> Vec<usize> {
    rows.iter()
        .filter_map(|row| match row {
            ListRow::Item(index) => Some(*index),
            ListRow::Header { .. } => None,
        })
        .collect()
}

#[cfg(test)]
pub(super) fn table_rows(
    entries: &[FileEntry],
    group: bool,
    preferred_group_order: &[String],
    folders_label: &str,
    files_label: &str,
) -> Vec<ListRow> {
    table_rows_sorted(
        entries,
        group,
        preferred_group_order,
        folders_label,
        files_label,
        SortKey::Name,
        false,
    )
}

pub(super) fn table_rows_sorted(
    entries: &[FileEntry],
    group: bool,
    preferred_group_order: &[String],
    folders_label: &str,
    files_label: &str,
    sort_key: SortKey,
    sort_desc: bool,
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
    for (_, (key, label, mut items)) in ranked {
        items.sort_by(|&left, &right| {
            compare_group_entries(&entries[left], &entries[right], sort_key, sort_desc)
        });
        rows.push(ListRow::Header { key, label });
        rows.extend(items.into_iter().map(ListRow::Item));
    }
    rows
}

fn compare_group_entries(
    left: &FileEntry,
    right: &FileEntry,
    sort_key: SortKey,
    sort_desc: bool,
) -> std::cmp::Ordering {
    let by_name = || natural_name_cmp(&left.name, &right.name);
    let order = match sort_key {
        SortKey::Name => by_name(),
        SortKey::Extension => natural_name_cmp(
            left.extension.as_deref().unwrap_or_default(),
            right.extension.as_deref().unwrap_or_default(),
        )
        .then_with(by_name),
        SortKey::Size => left.size.cmp(&right.size).then_with(by_name),
        SortKey::Modified => left.modified.cmp(&right.modified).then_with(by_name),
    };
    if sort_desc { order.reverse() } else { order }
}

fn natural_name_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let left_bytes = left.as_bytes();
    let right_bytes = right.as_bytes();
    let (mut left_index, mut right_index) = (0, 0);

    while left_index < left_bytes.len() && right_index < right_bytes.len() {
        let left_char = left[left_index..].chars().next().unwrap();
        let right_char = right[right_index..].chars().next().unwrap();
        if left_char.is_ascii_digit() && right_char.is_ascii_digit() {
            let left_end = digit_run_end(left_bytes, left_index);
            let right_end = digit_run_end(right_bytes, right_index);
            let left_significant = significant_digit_start(left_bytes, left_index, left_end);
            let right_significant = significant_digit_start(right_bytes, right_index, right_end);
            let order = (left_end - left_significant)
                .cmp(&(right_end - right_significant))
                .then_with(|| {
                    left_bytes[left_significant..left_end]
                        .cmp(&right_bytes[right_significant..right_end])
                })
                .then_with(|| (left_end - left_index).cmp(&(right_end - right_index)));
            if order != std::cmp::Ordering::Equal {
                return order;
            }
            left_index = left_end;
            right_index = right_end;
        } else {
            let order = left_char.to_lowercase().cmp(right_char.to_lowercase());
            if order != std::cmp::Ordering::Equal {
                return order;
            }
            left_index += left_char.len_utf8();
            right_index += right_char.len_utf8();
        }
    }
    (left_bytes.len() - left_index).cmp(&(right_bytes.len() - right_index))
}

fn digit_run_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    end
}

fn significant_digit_start(bytes: &[u8], start: usize, end: usize) -> usize {
    let mut significant = start;
    while significant + 1 < end && bytes[significant] == b'0' {
        significant += 1;
    }
    significant
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
        table_rows_sorted(
            &tab.entries,
            // Группируем текущие entries после фильтрации, не создавая пустых групп.
            self.store.settings.group_by_type,
            &self.store.settings.group_order,
            self.t("type.folder"),
            self.t("type.file"),
            tab.sort_key,
            tab.sort_desc,
        )
    }

    pub(crate) fn selection_order(&self) -> Option<Vec<usize>> {
        (self.store.settings.view_mode == crate::core::settings::ViewMode::Table
            && self.store.settings.group_by_type)
            .then(|| item_order(&self.build_rows()))
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
