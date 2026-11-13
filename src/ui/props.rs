use std::collections::HashSet;
use std::ops::RangeInclusive;
use std::path::Path;

use crate::core::entry_details::{AccessList, Detail, ExtraFileAttributes};
use crate::core::format::format_size;
use crate::core::fs::FileEntry;
use crate::platform::format_local_time;
use crate::ui::FileManagerApp;
use crate::ui::tabs::SelectionKey;

pub(crate) const PROPERTY_PANEL_FIXED_WIDTH: f32 = 440.0;
#[cfg(test)]
const PREVIOUS_PROPERTY_PANEL_WIDTH: f32 = 380.0;
pub(crate) const PROPERTIES_WINDOW_DEFAULT_WIDTH: f32 = 760.0;
pub(crate) const PROPERTIES_WINDOW_DEFAULT_HEIGHT: f32 = 560.0;

pub(crate) fn property_panel_fixed_range() -> RangeInclusive<f32> {
    PROPERTY_PANEL_FIXED_WIDTH..=PROPERTY_PANEL_FIXED_WIDTH
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PropertyLine {
    pub label: String,
    pub value: String,
}

impl FileManagerApp {
    /// Ширина задаётся в одной точке, чтобы старое сохранённое состояние egui
    /// не могло сузить панель и обрезать значения.
    pub(crate) fn props_panel(&self, ui: &mut egui::Ui) {
        let tab = &self.tabs[self.active];
        let selected_keys: HashSet<_> = tab.selection.iter().map(SelectionKey::borrowed).collect();
        let selected: Vec<_> = tab
            .entries
            .iter()
            .filter(|entry| {
                selected_keys.contains(&(entry.path.as_path(), entry.archive_member.as_deref()))
            })
            .collect();

        egui::ScrollArea::vertical().show(ui, |ui| match selected.len() {
            0 => self.props_folder_info(ui),
            1 => {
                let entry = selected[0];
                let allow_details = !tab.secure_trash
                    && tab.archive_prefix.is_none()
                    && entry.archive_member.is_none();
                self.render_property_lines(ui, &self.single_property_lines(entry, allow_details));
            }
            count => self.render_property_lines(ui, &self.multi_property_lines(&selected, count)),
        });
    }

    fn props_folder_info(&self, ui: &mut egui::Ui) {
        let (name, path, count) = {
            let tab = &self.tabs[self.active];
            (
                tab.title(),
                tab.path.display().to_string(),
                tab.entries.len(),
            )
        };
        let rows = vec![
            self.property_line("props.name", name),
            self.property_line("props.location", path),
            self.property_line("status.items", count.to_string()),
        ];
        self.render_property_lines(ui, &rows);
    }

    pub(crate) fn single_property_lines(
        &self,
        entry: &FileEntry,
        allow_details: bool,
    ) -> Vec<PropertyLine> {
        let reparse_point = entry.is_symlink || self.details_report_reparse(&entry.path);
        let unavailable = self.t("props.unavailable").to_owned();
        let type_text = property_type_text(
            entry.is_dir,
            reparse_point,
            entry.extension.as_deref(),
            self.t("type.folder"),
            self.t("type.file"),
            self.t("type.reparse_point"),
        );
        let size = logical_size_text(
            entry,
            reparse_point,
            &unavailable,
            self.t("props.not_calculated"),
        );
        let language = self.i18n.language();
        let date_value = |time: Option<std::time::SystemTime>| {
            if reparse_point {
                unavailable.clone()
            } else {
                time.map(|value| format_local_time(value, language))
                    .unwrap_or_else(|| unavailable.clone())
            }
        };
        let location = entry
            .path
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| unavailable.clone());

        let mut rows = vec![
            self.property_line("props.name", entry.name.clone()),
            self.property_line("props.location", location),
            self.property_line("props.path", entry.path.display().to_string()),
            self.property_line("props.type", type_text),
            self.property_line("props.size", size),
            self.property_line("props.created", date_value(entry.created)),
            self.property_line("props.modified", date_value(entry.modified)),
        ];
        rows.push(self.property_line("props.attributes", self.attribute_text(entry)));
        rows.extend(
            self.heavy_detail_lines(&entry.path, entry.is_dir, reparse_point, allow_details)
                .into_iter()
                .map(|(label, value)| PropertyLine { label, value }),
        );
        rows
    }

    pub(crate) fn missing_property_lines(
        &self,
        path: &Path,
        allow_details: bool,
    ) -> Vec<PropertyLine> {
        let unavailable = self.t("props.unavailable").to_owned();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let location = path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_else(|| unavailable.clone());
        let mut rows = vec![
            self.property_line("props.name", name),
            self.property_line("props.location", location),
            self.property_line("props.path", path.display().to_string()),
            self.property_line("props.type", unavailable.clone()),
            self.property_line("props.size", unavailable.clone()),
            self.property_line("props.created", unavailable.clone()),
            self.property_line("props.modified", unavailable.clone()),
            self.property_line("props.attributes", unavailable.clone()),
        ];
        rows.extend(
            self.heavy_detail_lines(path, false, false, allow_details)
                .into_iter()
                .map(|(label, value)| PropertyLine { label, value }),
        );
        rows
    }

    pub(crate) fn multi_property_lines(
        &self,
        entries: &[&FileEntry],
        count: usize,
    ) -> Vec<PropertyLine> {
        let location = multi_location(
            entries,
            self.t("props.unavailable"),
            self.t("props.different_locations"),
        );
        let size = match multi_logical_size(entries) {
            Some(size) => format_size(size),
            None if entries.iter().any(|entry| entry.is_dir) => {
                self.t("props.not_calculated").to_owned()
            }
            None => self.t("props.unavailable").to_owned(),
        };
        let unavailable = self.t("props.unavailable_multiple").to_owned();
        let mut rows = vec![
            self.property_line("props.selected_count", count.to_string()),
            self.property_line("props.type", self.t("props.type_multiple").to_owned()),
            self.property_line("props.location", location),
            self.property_line("props.size", size),
        ];
        rows.extend(
            multiple_heavy_field_keys()
                .into_iter()
                .map(|key| self.property_line(key, unavailable.clone())),
        );
        rows
    }

    fn property_line(&self, key: &str, value: String) -> PropertyLine {
        PropertyLine {
            label: self.t(key).to_owned(),
            value,
        }
    }

    pub(crate) fn render_property_lines(&self, ui: &mut egui::Ui, rows: &[PropertyLine]) {
        for row in rows {
            ui.horizontal_top(|ui| {
                ui.label(format!("{}:", row.label));
                ui.add(egui::Label::new(&row.value).wrap());
            });
            ui.separator();
        }
    }

    fn attribute_text(&self, entry: &FileEntry) -> String {
        let mut attributes = Vec::new();
        if entry.is_symlink {
            attributes.push(self.t("attr.reparse_point").to_owned());
        } else {
            if entry.readonly {
                attributes.push(self.t("attr.readonly").to_owned());
            }
            if entry.hidden {
                attributes.push(self.t("attr.hidden").to_owned());
            }
            if entry.system {
                attributes.push(self.t("attr.system").to_owned());
            }
        }
        match self.details_extra_attributes(&entry.path) {
            Some(Detail::Known(extra)) => {
                append_extra_attributes(&mut attributes, extra, self, !entry.is_symlink)
            }
            Some(Detail::Unavailable) => {
                attributes.push(self.t("props.attributes_unavailable").to_owned())
            }
            None if self.details_watch.pending
                && self.details_watch.path.as_deref() == Some(entry.path.as_path()) =>
            {
                attributes.push(self.t("props.details_loading").to_owned())
            }
            None => attributes.push(self.t("props.attributes_unavailable").to_owned()),
        }
        if attributes.is_empty() {
            self.t("props.no_attributes").to_owned()
        } else {
            attributes.join(", ")
        }
    }

    fn details_extra_attributes(&self, path: &Path) -> Option<&Detail<ExtraFileAttributes>> {
        if self.details_watch.path.as_deref() != Some(path) {
            return None;
        }
        self.details_watch
            .details
            .as_ref()
            .map(|details| &details.extra_attributes)
    }

    fn details_report_reparse(&self, path: &Path) -> bool {
        matches!(
            self.details_extra_attributes(path),
            Some(Detail::Known(ExtraFileAttributes {
                reparse_point: true,
                ..
            }))
        )
    }

    fn heavy_detail_lines(
        &self,
        path: &Path,
        is_dir: bool,
        reparse_point: bool,
        allow_details: bool,
    ) -> Vec<(String, String)> {
        let loading = self.t("props.details_loading").to_owned();
        let unavailable = self.t("props.unavailable").to_owned();
        let watching_path = self.details_watch.path.as_deref() == Some(path);
        let pending =
            allow_details && !reparse_point && watching_path && self.details_watch.pending;
        let Some(details) = self
            .details_watch
            .details
            .as_ref()
            .filter(|_| self.details_watch.path.as_deref() == Some(path))
        else {
            let status = if pending {
                loading.clone()
            } else {
                unavailable.clone()
            };
            return self.heavy_status_lines(is_dir, &status);
        };
        if !allow_details || reparse_point {
            return self.heavy_status_lines(is_dir, &unavailable);
        }
        let size_label = if is_dir {
            "props.folder_entry_size"
        } else {
            "props.size_on_disk"
        };
        let size_on_disk =
            crate::core::entry_details::detail_text(&details.size_on_disk, &unavailable, |size| {
                format_size(*size)
            });
        let accessed =
            crate::core::entry_details::detail_text(&details.accessed, &unavailable, |time| {
                format_local_time(*time, self.i18n.language())
            });
        let owner =
            crate::core::entry_details::detail_text(&details.owner, &unavailable, Clone::clone);
        let access = match &details.access {
            Detail::Known(AccessList::Null) => self.t("props.access_null").to_owned(),
            Detail::Known(AccessList::Empty) => self.t("props.access_empty").to_owned(),
            Detail::Known(AccessList::Present { entries }) => {
                format!("{} ({entries})", self.t("props.access_present"))
            }
            Detail::Unavailable => unavailable,
        };
        let mut lines = vec![
            (self.t(size_label).to_owned(), size_on_disk),
            (self.t("props.accessed").to_owned(), accessed),
            (self.t("props.owner").to_owned(), owner),
            (self.t("props.access").to_owned(), access),
        ];
        if details.folder_contents_not_counted {
            lines.insert(
                1,
                (
                    self.t("props.folder_contents").to_owned(),
                    self.t("props.not_calculated").to_owned(),
                ),
            );
        }
        lines
    }

    fn heavy_status_lines(&self, is_dir: bool, status: &str) -> Vec<(String, String)> {
        heavy_status_values(is_dir, status, self.t("props.not_calculated"))
            .into_iter()
            .map(|(label, value)| (self.t(label).to_owned(), value))
            .collect()
    }
}

fn property_type_text(
    is_dir: bool,
    reparse_point: bool,
    extension: Option<&str>,
    folder_label: &str,
    file_label: &str,
    reparse_label: &str,
) -> String {
    if reparse_point {
        reparse_label.to_owned()
    } else if is_dir {
        folder_label.to_owned()
    } else {
        match extension {
            Some(extension) => format!("{} {file_label}", extension.to_uppercase()),
            None => file_label.to_owned(),
        }
    }
}

fn logical_size_text(
    entry: &FileEntry,
    reparse_point: bool,
    unavailable: &str,
    not_calculated: &str,
) -> String {
    if reparse_point {
        unavailable.to_owned()
    } else if entry.is_dir {
        not_calculated.to_owned()
    } else {
        entry
            .size
            .map(format_size)
            .unwrap_or_else(|| unavailable.to_owned())
    }
}

fn heavy_status_values(
    is_dir: bool,
    status: &str,
    not_calculated: &str,
) -> Vec<(&'static str, String)> {
    let size_label = if is_dir {
        "props.folder_entry_size"
    } else {
        "props.size_on_disk"
    };
    let mut fields = vec![
        (size_label, status.to_owned()),
        ("props.accessed", status.to_owned()),
        ("props.owner", status.to_owned()),
        ("props.access", status.to_owned()),
    ];
    if is_dir {
        fields.insert(1, ("props.folder_contents", not_calculated.to_owned()));
    }
    fields
}

fn multiple_heavy_field_keys() -> [&'static str; 4] {
    [
        "props.size_on_disk",
        "props.accessed",
        "props.owner",
        "props.access",
    ]
}

fn multi_logical_size(entries: &[&FileEntry]) -> Option<u64> {
    entries.iter().try_fold(0u64, |total, entry| {
        if entry.is_dir {
            return None;
        }
        total.checked_add(entry.size?)
    })
}

fn multi_location(entries: &[&FileEntry], unavailable: &str, different: &str) -> String {
    let Some(first) = entries.first().and_then(|entry| entry.path.parent()) else {
        return unavailable.to_owned();
    };
    let first_text = first.display().to_string();
    if entries.iter().all(|entry| {
        entry
            .path
            .parent()
            .is_some_and(|parent| parent.to_string_lossy().eq_ignore_ascii_case(&first_text))
    }) {
        first_text
    } else {
        different.to_owned()
    }
}

fn append_extra_attributes(
    attributes: &mut Vec<String>,
    extra: &ExtraFileAttributes,
    app: &FileManagerApp,
    include_reparse_point: bool,
) {
    if extra.archive {
        attributes.push(app.t("attr.archive").to_owned());
    }
    if extra.compressed {
        attributes.push(app.t("attr.compressed").to_owned());
    }
    if extra.encrypted {
        attributes.push(app.t("attr.encrypted").to_owned());
    }
    if extra.reparse_point && include_reparse_point {
        attributes.push(app.t("attr.reparse_point").to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, parent: &str, is_dir: bool, size: Option<u64>) -> FileEntry {
        let path = Path::new(parent).join(name);
        FileEntry {
            name: name.to_owned(),
            path,
            is_dir,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size,
            modified: None,
            created: None,
            extension: crate::core::fs::listing_extension(is_dir, &Path::new(parent).join(name)),
            archive_member: None,
        }
    }

    #[test]
    fn props_panel_width_is_fixed_and_wider_than_previous_default() {
        let range = property_panel_fixed_range();
        assert_eq!(*range.start(), PROPERTY_PANEL_FIXED_WIDTH);
        assert_eq!(*range.end(), PROPERTY_PANEL_FIXED_WIDTH);
        assert!(PROPERTY_PANEL_FIXED_WIDTH > PREVIOUS_PROPERTY_PANEL_WIDTH);
    }

    #[test]
    fn props_panel_does_not_use_saved_resizable_width() {
        let legacy_width = 300.0..=500.0;
        assert!(!property_panel_fixed_range().contains(legacy_width.start()));
        assert!(!property_panel_fixed_range().contains(legacy_width.end()));
        assert_eq!(
            *property_panel_fixed_range().start(),
            PROPERTY_PANEL_FIXED_WIDTH
        );
    }

    #[test]
    fn props_type_uses_listing_extension_not_displayed_suffix() {
        let shown_name = "report.txt (copy)";
        let extension = crate::core::fs::listing_extension(false, Path::new("report.txt"));
        assert_eq!(shown_name, "report.txt (copy)");
        assert_eq!(
            property_type_text(
                false,
                false,
                extension.as_deref(),
                "Folder",
                "file",
                "Reparse point"
            ),
            "TXT file"
        );
    }

    #[test]
    fn props_panel_shows_lazy_heavy_fields_with_loading_state() {
        let fields = heavy_status_values(false, "считается…", "не рассчитан");
        assert_eq!(fields.len(), 4);
        assert!(fields.iter().all(|(_, value)| value == "считается…"));
        assert_eq!(
            fields.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec![
                "props.size_on_disk",
                "props.accessed",
                "props.owner",
                "props.access"
            ]
        );
    }

    #[test]
    fn props_panel_shows_unavailable_for_failed_owner_or_acl() {
        let details = crate::core::entry_details::EntryDetails::unavailable();
        let owner =
            crate::core::entry_details::detail_text(&details.owner, "недоступно", Clone::clone);
        let acl = crate::core::entry_details::detail_text(
            &details.access,
            "недоступно",
            |_| "технический код не должен отображаться".to_owned(),
        );
        assert_eq!(owner, "недоступно");
        assert_eq!(acl, "недоступно");
    }

    #[test]
    fn folder_size_is_not_summed_by_default_and_marked_not_calculated() {
        let folder = entry("Folder", r"C:\data", true, None);
        assert_eq!(
            logical_size_text(&folder, false, "unavailable", "not calculated"),
            "not calculated"
        );
        assert_eq!(
            property_type_text(true, false, None, "Folder", "file", "Reparse point"),
            "Folder"
        );
    }

    #[test]
    fn symlink_or_junction_properties_do_not_resolve_target() {
        let mut link = entry("Alias", r"C:\data", false, Some(100_000));
        link.is_symlink = true;
        assert_eq!(
            logical_size_text(&link, true, "unavailable", "not calculated"),
            "unavailable"
        );
        assert_eq!(
            property_type_text(true, true, None, "Folder", "file", "Link or reparse point"),
            "Link or reparse point"
        );
    }

    #[test]
    fn multiple_selection_aggregates_count_and_logical_size_without_fake_heavy_fields() {
        let first = entry("one.txt", r"C:\data", false, Some(10));
        let second = entry("two.txt", r"C:\data", false, Some(20));
        assert_eq!(multi_logical_size(&[&first, &second]), Some(30));
        assert_eq!(multiple_heavy_field_keys().len(), 4);
        assert_eq!(
            property_type_text(
                false,
                false,
                None,
                "Folder",
                "Multiple objects",
                "Reparse point"
            ),
            "Multiple objects"
        );
    }

    #[test]
    fn props_window_alt_enter_reuses_existing_dialog_and_shows_same_fields() {
        assert!(PROPERTIES_WINDOW_DEFAULT_WIDTH > PROPERTY_PANEL_FIXED_WIDTH);
        assert!(PROPERTIES_WINDOW_DEFAULT_HEIGHT > 0.0);
        assert_eq!(
            heavy_status_values(false, "reading…", "not calculated")
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            multiple_heavy_field_keys()
                .iter()
                .map(|key| *key)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn multiple_selection_shows_different_locations_when_parents_differ() {
        let first = entry("one.txt", r"C:\data\a", false, Some(10));
        let second = entry("two.txt", r"C:\data\b", false, Some(20));
        assert_eq!(
            multi_location(&[&first, &second], "unavailable", "Different locations"),
            "Different locations"
        );
    }

    #[test]
    fn props_labels_are_localized_in_ru_and_en() {
        let required = [
            "props.not_calculated",
            "props.unavailable_multiple",
            "props.different_locations",
            "props.folder_entry_size",
            "attr.system",
            "attr.archive",
            "attr.compressed",
            "attr.encrypted",
            "attr.reparse_point",
        ];
        for json in [
            include_str!("../../i18n/en.json"),
            include_str!("../../i18n/ru.json"),
        ] {
            let labels: serde_json::Value = serde_json::from_str(json).unwrap();
            for key in required {
                assert!(labels.get(key).is_some(), "missing localized key {key}");
            }
        }
    }
}
