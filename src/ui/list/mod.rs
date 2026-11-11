mod actions;
mod columns;
mod grid;
mod grouping;
mod rows;

pub(super) use columns::next_sort_order;
use columns::{COL_GAP, paint_column_separators, table_rows_content_margin};
#[cfg(test)]
use columns::{column_positions, column_separator_x};
use grouping::{ListRow, item_order, move_group_order};
#[cfg(test)]
use grouping::{group_drag_threshold_crossed, table_rows, table_rows_sorted};
use rows::entry_at;

use crate::core::format::format_size;
use crate::core::fs::{FileEntry, SortKey};
use crate::core::settings::ViewMode;
use crate::platform::format_local_time;
use crate::ui::FileManagerApp;
use crate::ui::icons;
use crate::ui::style;

pub(crate) const ROW_HEIGHT: f32 = 24.0;
pub(crate) const ICON_WIDTH: f32 = 26.0;
impl FileManagerApp {
    /// Заголовок и виртуализированные строки выбранного режима отображения.
    pub(crate) fn file_list(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        style::dense_vertical_spacing(ui.style_mut());

        let count = self.tabs[self.active].entries.len();
        let table_view = self.store.settings.view_mode == ViewMode::Table;
        if !table_view {
            self.tabs[self.active].selection_marquee = None;
        }
        // Background hit-test under the whole list: rows drawn later win their
        // rects, empty space falls through to this response.
        let bg = ui.available_rect_before_wrap();
        let bg_sense = if table_view {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        };
        let bg_resp = ui.interact(bg, ui.scope_id().with("empty_list_area"), bg_sense);
        let mut group_drop: Option<(Vec<String>, String, bool)> = None;
        let mut marquee_rect = None;
        match self.store.settings.view_mode {
            ViewMode::Table => {
                self.list_header(ui);
                ui.separator();
                let rows_area = ui.available_rect_before_wrap().intersect(ui.clip_rect());
                let primary_down =
                    ctx.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
                let primary_released =
                    ctx.input(|input| input.pointer.button_released(egui::PointerButton::Primary));
                if !primary_down && !primary_released {
                    self.tabs[self.active].selection_marquee = None;
                }
                if bg_resp.drag_started_by(egui::PointerButton::Primary)
                    && self.group_press.is_none()
                    && let Some(start) = ctx
                        .input(|input| input.pointer.press_origin())
                        .or_else(|| bg_resp.interact_pointer_pos())
                    && rows_area.contains(start)
                {
                    let tab = &mut self.tabs[self.active];
                    if !ctx.input(|input| input.modifiers.command) {
                        tab.selection.clear();
                        tab.anchor = None;
                    }
                    tab.selection_marquee = Some(crate::ui::tabs::SelectionMarquee {
                        start,
                        current: start,
                        added: std::collections::BTreeSet::new(),
                    });
                }
                if let Some(tab) = self.tabs.get_mut(self.active)
                    && let Some(marquee) = &mut tab.selection_marquee
                {
                    if let Some(pointer) = ctx.pointer_latest_pos() {
                        marquee.current = pointer;
                    }
                    // На каждом кадре обновляем только элементы, затронутые рамкой.
                    let previous_additions = std::mem::take(&mut marquee.added);
                    for key in previous_additions {
                        tab.selection.remove(&key);
                    }
                    if tab
                        .anchor
                        .as_ref()
                        .is_some_and(|key| !tab.selection.contains(key))
                    {
                        tab.anchor = None;
                    }
                    marquee_rect = Some(
                        egui::Rect::from_two_pos(marquee.start, marquee.current)
                            .expand(1.0)
                            .intersect(rows_area),
                    );
                }
                let rows = self.build_rows();
                let visual_order = self.store.settings.group_by_type.then(|| item_order(&rows));
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
                    .content_margin(table_rows_content_margin())
                    .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
                        for index in range {
                            match &rows[index] {
                                ListRow::Item(i) => {
                                    self.file_row(
                                        ui,
                                        *i,
                                        ctx,
                                        visual_order.as_deref(),
                                        marquee_rect,
                                    );
                                }
                                // Заголовок группы отображается, но не попадает в выбор.
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
                            self.list_row(ui, index, ctx, None);
                        }
                    });
            }
            ViewMode::Grid => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.grid_view(ui, ctx, None));
            }
        }
        if let Some(rect) = marquee_rect {
            let visuals = ui.visuals();
            ui.painter().rect_filled(
                rect,
                style::RADIUS_ICON_FINE,
                visuals.selection.bg_fill.gamma_multiply(0.28),
            );
            ui.painter().rect_stroke(
                rect,
                style::RADIUS_ICON_FINE,
                egui::Stroke::new(1.0, visuals.selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        if ctx.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
            self.tabs[self.active].selection_marquee = None;
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
        if let (Some((_, label)), Some(pointer)) = (&self.group_drag, ctx.pointer_latest_pos()) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, is_dir: bool, extension: Option<&str>) -> FileEntry {
        FileEntry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            is_dir,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: None,
            modified: None,
            created: None,
            extension: extension.map(str::to_owned),
            archive_member: None,
        }
    }

    fn keys_for(
        entries: &[FileEntry],
        indices: &[usize],
    ) -> BTreeSet<crate::ui::tabs::SelectionKey> {
        indices
            .iter()
            .map(|&index| crate::ui::tabs::SelectionKey::from_entry(&entries[index]))
            .collect()
    }

    fn names_in_group(rows: &[ListRow], entries: &[FileEntry], group_key: &str) -> Vec<String> {
        let mut current_group = None;
        let mut names = Vec::new();
        for row in rows {
            match row {
                ListRow::Header { key, .. } => current_group = Some(key.as_str()),
                ListRow::Item(index) if current_group == Some(group_key) => {
                    names.push(entries[*index].name.clone());
                }
                ListRow::Item(_) => {}
            }
        }
        names
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
                ListRow::Header {
                    key: "folder".into(),
                    label: "Folders".into()
                },
                ListRow::Item(3),
                ListRow::Item(1),
                ListRow::Header {
                    key: "file".into(),
                    label: "Files".into()
                },
                ListRow::Item(2),
                ListRow::Header {
                    key: "ext:md".into(),
                    label: "MD".into()
                },
                ListRow::Item(4),
                ListRow::Header {
                    key: "ext:txt".into(),
                    label: "TXT".into()
                },
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
    fn grouped_shift_range_and_marquee_use_only_visual_item_rows() {
        let entries = vec![
            entry("z.txt", false, Some("txt")),
            entry("B folder", true, None),
            entry("readme", false, None),
            entry("A folder", true, None),
            entry("a.md", false, Some("md")),
            entry("a.txt", false, Some("txt")),
        ];
        let rows = table_rows(&entries, true, &[], "Folders", "Files");
        let visual_order = item_order(&rows);
        assert_eq!(visual_order, [3, 1, 2, 4, 5, 0]);

        let mut selection = BTreeSet::new();
        let mut anchor = None;
        crate::ui::tabs::apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            1,
            false,
            false,
        );
        assert_eq!(selection, keys_for(&entries, &[1]));
        crate::ui::tabs::apply_click(
            &mut selection,
            &mut anchor,
            &entries,
            Some(&visual_order),
            5,
            false,
            true,
        );
        let expected: BTreeSet<_> = [1, 2, 4, 5]
            .into_iter()
            .map(|index| crate::ui::tabs::SelectionKey::from_entry(&entries[index]))
            .collect();
        assert_eq!(selection, expected);

        let mut marquee = crate::ui::tabs::SelectionMarquee {
            start: egui::pos2(0.0, 0.0),
            current: egui::pos2(10.0, 10.0),
            added: BTreeSet::new(),
        };
        let mut marquee_selection = BTreeSet::new();
        for &index in &visual_order[1..4] {
            crate::ui::tabs::add_marquee_entry(
                &mut marquee_selection,
                &mut marquee,
                &entries[index],
            );
        }
        let marquee_expected: BTreeSet<_> = [1, 2, 4]
            .into_iter()
            .map(|index| crate::ui::tabs::SelectionKey::from_entry(&entries[index]))
            .collect();
        assert_eq!(marquee_selection, marquee_expected);
        assert_eq!(marquee.added, marquee_expected);
    }

    #[test]
    fn grouping_toggle_keeps_selection_on_the_same_entries() {
        let entries = vec![
            entry("z.txt", false, Some("txt")),
            entry("folder", true, None),
            entry("a.md", false, Some("md")),
        ];
        let selected: BTreeSet<_> = [0, 2]
            .into_iter()
            .map(|index| crate::ui::tabs::SelectionKey::from_entry(&entries[index]))
            .collect();
        let grouped = item_order(&table_rows(&entries, true, &[], "Folders", "Files"));
        let flat = item_order(&table_rows(&entries, false, &[], "Folders", "Files"));
        assert_ne!(grouped, flat);
        let selected_names = |order: &[usize]| {
            order
                .iter()
                .filter_map(|&index| {
                    let entry = &entries[index];
                    selected
                        .contains(&crate::ui::tabs::SelectionKey::from_entry(entry))
                        .then(|| entry.name.clone())
                })
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(
            selected_names(&grouped),
            BTreeSet::from(["a.md".to_owned(), "z.txt".to_owned()])
        );
        assert_eq!(
            selected_names(&flat),
            BTreeSet::from(["a.md".to_owned(), "z.txt".to_owned()])
        );
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
                ListRow::Header {
                    key: "ext:txt".into(),
                    label: "TXT".into()
                },
                ListRow::Item(1),
                ListRow::Item(0),
            ]
        );
        assert!(table_rows(&[], true, &[], "Folders", "Files").is_empty());
    }

    #[test]
    fn list_filters_keep_only_nonempty_type_groups() {
        let from = UNIX_EPOCH + Duration::from_secs(10_000);
        let before = from + Duration::from_secs(2 * 24 * 60 * 60);
        let mut matching = entry("report.txt", false, Some("txt"));
        matching.size = Some(1024);
        matching.modified = Some(from + Duration::from_secs(60));
        let mut wrong_name = entry("notes.txt", false, Some("txt"));
        wrong_name.size = Some(1024);
        wrong_name.modified = Some(from + Duration::from_secs(60));
        let mut wrong_size = entry("report-large.txt", false, Some("txt"));
        wrong_size.size = Some(4096);
        wrong_size.modified = Some(from + Duration::from_secs(60));
        let mut wrong_type = entry("report.md", false, Some("md"));
        wrong_type.size = Some(1024);
        wrong_type.modified = Some(from + Duration::from_secs(60));
        let mut wrong_date = entry("report-old.txt", false, Some("txt"));
        wrong_date.size = Some(1024);
        wrong_date.modified = Some(from - Duration::from_secs(60));

        let options = crate::core::search::SearchOptions {
            query: "report".into(),
            extensions: BTreeSet::from(["txt".to_owned()]),
            min_size: Some(512),
            max_size: Some(2048),
            modified_from: Some(from),
            modified_before: Some(before),
            ..Default::default()
        };
        let filtered: Vec<_> = [matching, wrong_name, wrong_size, wrong_type, wrong_date]
            .into_iter()
            .filter(|entry| crate::core::search::matches_entry(entry, &options))
            .collect();

        let rows = table_rows(&filtered, true, &[], "Folders", "Files");
        assert_eq!(
            rows,
            vec![
                ListRow::Header {
                    key: "ext:txt".into(),
                    label: "TXT".into()
                },
                ListRow::Item(0),
            ]
        );
        assert_eq!(
            table_rows(&filtered, false, &[], "Folders", "Files"),
            vec![ListRow::Item(0)]
        );
        assert!(table_rows(&[], true, &[], "Folders", "Files").is_empty());
    }

    #[test]
    fn grouped_rows_sort_naturally_and_follow_size_date_direction() {
        let date = UNIX_EPOCH + Duration::from_secs(10_000);
        let mut folder_10 = entry("folder10", true, None);
        folder_10.modified = Some(date + Duration::from_secs(10));
        let mut folder_2 = entry("folder2", true, None);
        folder_2.modified = Some(date + Duration::from_secs(2));
        let mut file_10 = entry("file10.txt", false, Some("txt"));
        file_10.size = Some(10);
        file_10.modified = Some(date + Duration::from_secs(10));
        let mut file_2 = entry("file2.txt", false, Some("txt"));
        file_2.size = Some(20);
        file_2.modified = Some(date + Duration::from_secs(2));
        let entries = vec![folder_10, folder_2, file_10, file_2];

        let by_name = table_rows_sorted(
            &entries,
            true,
            &[],
            "Folders",
            "Files",
            SortKey::Name,
            false,
        );
        assert_eq!(
            names_in_group(&by_name, &entries, "folder"),
            ["folder2", "folder10"]
        );
        assert_eq!(
            names_in_group(&by_name, &entries, "ext:txt"),
            ["file2.txt", "file10.txt"]
        );

        let by_name_desc =
            table_rows_sorted(&entries, true, &[], "Folders", "Files", SortKey::Name, true);
        assert_eq!(
            names_in_group(&by_name_desc, &entries, "ext:txt"),
            ["file10.txt", "file2.txt"]
        );

        let by_size = table_rows_sorted(
            &entries,
            true,
            &[],
            "Folders",
            "Files",
            SortKey::Size,
            false,
        );
        assert_eq!(
            names_in_group(&by_size, &entries, "ext:txt"),
            ["file10.txt", "file2.txt"]
        );
        let by_size_desc =
            table_rows_sorted(&entries, true, &[], "Folders", "Files", SortKey::Size, true);
        assert_eq!(
            names_in_group(&by_size_desc, &entries, "ext:txt"),
            ["file2.txt", "file10.txt"]
        );

        let by_date = table_rows_sorted(
            &entries,
            true,
            &[],
            "Folders",
            "Files",
            SortKey::Modified,
            false,
        );
        assert_eq!(
            names_in_group(&by_date, &entries, "ext:txt"),
            ["file2.txt", "file10.txt"]
        );
        let by_date_desc = table_rows_sorted(
            &entries,
            true,
            &[],
            "Folders",
            "Files",
            SortKey::Modified,
            true,
        );
        assert_eq!(
            names_in_group(&by_date_desc, &entries, "ext:txt"),
            ["file10.txt", "file2.txt"]
        );

        assert_eq!(
            table_rows_sorted(
                &entries,
                false,
                &[],
                "Folders",
                "Files",
                SortKey::Size,
                true,
            ),
            (0..entries.len()).map(ListRow::Item).collect::<Vec<_>>()
        );
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
        assert_eq!(
            headers,
            [
                ("ext:txt", "TXT"),
                ("folder", "Folders"),
                ("file", "Files"),
                ("ext:md", "MD")
            ]
        );
    }

    #[test]
    fn moving_a_group_changes_only_group_order() {
        let keys = vec![
            "folder".into(),
            "file".into(),
            "ext:md".into(),
            "ext:txt".into(),
        ];
        assert_eq!(
            move_group_order(&keys, "ext:txt", "folder", false),
            Some(
                vec!["ext:txt", "folder", "file", "ext:md"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            )
        );
        assert_eq!(move_group_order(&keys, "file", "file", true), None);
        assert_eq!(move_group_order(&keys, "missing", "folder", false), None);
    }

    #[test]
    fn group_drag_starts_only_after_a_real_pointer_move() {
        let press = egui::pos2(100.0, 20.0);
        assert!(!group_drag_threshold_crossed(
            press,
            egui::pos2(104.0, 20.0)
        ));
        assert!(!group_drag_threshold_crossed(
            press,
            egui::pos2(100.0, 25.0)
        ));
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
    fn table_rows_do_not_add_horizontal_scroll_margin_that_shifts_columns() {
        let margin = table_rows_content_margin();
        assert_eq!(margin.left, 0);
        assert_eq!(margin.right, 0);
        assert_eq!(margin.top, style::SPACE_SM as i8);
        assert_eq!(margin.bottom, style::SPACE_SM as i8);
    }

    #[test]
    fn stale_virtual_row_index_is_ignored_safely() {
        let entries = vec![entry("still valid.txt", false, Some("txt"))];
        assert_eq!(entry_at(&entries, 0).unwrap().name, "still valid.txt");
        assert!(entry_at(&entries, 4).is_none());
        assert!(entry_at(&[], 0).is_none());
    }

    #[test]
    fn trash_rows_group_and_sort_by_card_fields_not_the_painted_suffix() {
        let mut notes = entry("notes.txt", false, Some("txt"));
        notes.size = Some(10);
        notes.modified = Some(UNIX_EPOCH + Duration::from_secs(20));
        let mut bigger = entry("bigger.txt", false, Some("txt"));
        bigger.size = Some(30);
        bigger.modified = Some(UNIX_EPOCH + Duration::from_secs(5));
        let painted = format!("{} · Откуда: D:\\docs • повреждена", notes.name);
        assert_ne!(notes.name, painted);
        assert_eq!(notes.extension.as_deref(), Some("txt"));
        let entries = vec![bigger.clone(), notes.clone()];
        let rows = table_rows_sorted(&entries, true, &[], "Папка", "Файл", SortKey::Name, false);
        let names: Vec<_> = item_order(&rows)
            .into_iter()
            .map(|index| entries[index].name.as_str())
            .collect();
        assert_eq!(names, vec!["bigger.txt", "notes.txt"]);
        let by_size = table_rows_sorted(&entries, true, &[], "Папка", "Файл", SortKey::Size, true);
        let sized: Vec<_> = item_order(&by_size)
            .into_iter()
            .map(|index| entries[index].size)
            .collect();
        assert_eq!(sized, vec![Some(30), Some(10)]);
        let by_time = table_rows_sorted(
            &entries,
            true,
            &[],
            "Папка",
            "Файл",
            SortKey::Modified,
            false,
        );
        let times: Vec<_> = item_order(&by_time)
            .into_iter()
            .map(|index| entries[index].modified)
            .collect();
        assert_eq!(times, vec![bigger.modified, notes.modified]);
        assert!(rows.iter().any(|row| matches!(
            row,
            ListRow::Header { key, .. } if key == "ext:txt"
        )));
    }
}
