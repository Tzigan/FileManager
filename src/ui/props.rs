use std::collections::HashSet;

use crate::core::format::format_size;
use crate::platform::format_local_time;
use crate::ui::FileManagerApp;
use crate::ui::tabs::SelectionKey;

impl FileManagerApp {
    /// Right-side properties panel (~380 px): details of the selected
    /// entry, a multi-selection summary, or the current folder.
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

        match selected.len() {
            0 => self.props_folder_info(ui),
            1 => self.props_single(ui, selected[0]),
            n => self.props_multi(ui, n, &selected),
        }
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
        egui::Grid::new("props_folder")
            .num_columns(2)
            .show(ui, |ui| {
                self.prop_row(ui, "props.name", &name);
                self.prop_row(ui, "props.location", &path);
                self.prop_row(ui, "status.items", &count.to_string());
            });
    }

    fn props_single(&self, ui: &mut egui::Ui, entry: &crate::core::fs::FileEntry) {
        let name = entry.name.clone();
        let location = entry
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let type_text = if entry.is_dir {
            self.t("type.folder").to_owned()
        } else {
            match &entry.extension {
                Some(ext) => format!("{} {}", ext.to_uppercase(), self.t("type.file")),
                None => self.t("type.file").to_owned(),
            }
        };
        let size = entry.size.map(format_size).unwrap_or_default();
        let language = self.i18n.language();
        let created = entry
            .created
            .map(|time| format_local_time(time, language))
            .unwrap_or_default();
        let modified = entry
            .modified
            .map(|time| format_local_time(time, language))
            .unwrap_or_default();
        let mut attrs = Vec::new();
        if entry.hidden {
            attrs.push(self.t("attr.hidden").to_owned());
        }
        if entry.readonly {
            attrs.push(self.t("attr.readonly").to_owned());
        }
        let attrs = attrs.join(", ");

        egui::Grid::new("props_single")
            .num_columns(2)
            .show(ui, |ui| {
                self.prop_row(ui, "props.name", &name);
                self.prop_row(ui, "props.location", &location);
                self.prop_row(ui, "props.type", &type_text);
                if !entry.is_dir {
                    self.prop_row(ui, "props.size", &size);
                }
                self.prop_row(ui, "props.created", &created);
                self.prop_row(ui, "props.modified", &modified);
                for (label, value) in self.heavy_detail_lines(&entry.path) {
                    self.prop_row_text(ui, &label, &value);
                }
                if !attrs.is_empty() {
                    self.prop_row(ui, "props.attributes", &attrs);
                }
            });
    }

    fn props_multi(
        &self,
        ui: &mut egui::Ui,
        count: usize,
        selected: &[&crate::core::fs::FileEntry],
    ) {
        let total: u64 = selected.iter().filter_map(|entry| entry.size).sum();
        egui::Grid::new("props_multi")
            .num_columns(2)
            .show(ui, |ui| {
                self.prop_row(ui, "props.selected_count", &count.to_string());
                self.prop_row(ui, "props.size", &format_size(total));
            });
    }

    fn prop_row(&self, ui: &mut egui::Ui, key: &str, value: &str) {
        self.prop_row_text(ui, self.t(key), value);
    }

    fn prop_row_text(&self, ui: &mut egui::Ui, label: &str, value: &str) {
        ui.label(label.to_owned());
        ui.label(value.to_owned());
        ui.end_row();
    }

    /// Строки, которые появляются только после отдельного запроса, не из списка папки.
    pub(crate) fn heavy_detail_lines(&self, path: &std::path::Path) -> Vec<(String, String)> {
        if self.entry_details_path.as_deref() != Some(path) {
            return Vec::new();
        }
        if self.entry_details_pending {
            return vec![(
                self.t("props.size_on_disk").to_owned(),
                self.t("props.details_loading").to_owned(),
            )];
        }
        let Some(details) = &self.entry_details else {
            return Vec::new();
        };
        let unavailable = self.t("props.unavailable");
        let size_on_disk = match details.size_on_disk {
            crate::core::entry_details::Detail::Known(size) => {
                crate::core::format::format_size(size)
            }
            crate::core::entry_details::Detail::Unavailable => unavailable.to_owned(),
        };
        let accessed = match details.accessed {
            crate::core::entry_details::Detail::Known(time) => {
                crate::platform::format_local_time(time, self.i18n.language())
            }
            crate::core::entry_details::Detail::Unavailable => unavailable.to_owned(),
        };
        let owner = match &details.owner {
            crate::core::entry_details::Detail::Known(name) => name.clone(),
            crate::core::entry_details::Detail::Unavailable => unavailable.to_owned(),
        };
        let access = match details.access {
            crate::core::entry_details::Detail::Known(
                crate::core::entry_details::AccessList::Null,
            ) => self.t("props.access_null").to_owned(),
            crate::core::entry_details::Detail::Known(
                crate::core::entry_details::AccessList::Empty,
            ) => self.t("props.access_empty").to_owned(),
            crate::core::entry_details::Detail::Known(
                crate::core::entry_details::AccessList::Present { entries },
            ) => format!("{} ({entries})", self.t("props.access_present")),
            crate::core::entry_details::Detail::Unavailable => unavailable.to_owned(),
        };
        let mut lines = vec![
            (self.t("props.size_on_disk").to_owned(), size_on_disk),
            (self.t("props.accessed").to_owned(), accessed),
            (self.t("props.owner").to_owned(), owner),
            (self.t("props.access").to_owned(), access),
        ];
        if details.folder_contents_not_counted {
            lines.insert(
                1,
                (
                    self.t("props.folder_contents").to_owned(),
                    self.t("props.folder_contents_value").to_owned(),
                ),
            );
        }
        lines
    }
}
