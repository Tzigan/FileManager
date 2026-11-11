use std::collections::HashSet;

use crate::core::format::format_size;
use crate::platform::format_local_time;
use crate::ui::tabs::SelectionKey;
use crate::ui::FileManagerApp;

impl FileManagerApp {
    /// Right-side properties panel (~380 px): details of the selected
    /// entry, a multi-selection summary, or the current folder.
    pub(crate) fn props_panel(&self, ui: &mut egui::Ui) {
        let tab = &self.tabs[self.active];
        let selected_keys: HashSet<_> = tab
            .selection
            .iter()
            .map(SelectionKey::borrowed)
            .collect();
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
            (tab.title(), tab.path.display().to_string(), tab.entries.len())
        };
        egui::Grid::new("props_folder").num_columns(2).show(ui, |ui| {
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

        egui::Grid::new("props_single").num_columns(2).show(ui, |ui| {
            self.prop_row(ui, "props.name", &name);
            self.prop_row(ui, "props.location", &location);
            self.prop_row(ui, "props.type", &type_text);
            if !entry.is_dir {
                self.prop_row(ui, "props.size", &size);
            }
            self.prop_row(ui, "props.created", &created);
            self.prop_row(ui, "props.modified", &modified);
            if !attrs.is_empty() {
                self.prop_row(ui, "props.attributes", &attrs);
            }
        });
    }

    fn props_multi(&self, ui: &mut egui::Ui, count: usize, selected: &[&crate::core::fs::FileEntry]) {
        let total: u64 = selected
            .iter()
            .filter_map(|entry| entry.size)
            .sum();
        egui::Grid::new("props_multi").num_columns(2).show(ui, |ui| {
            self.prop_row(ui, "props.selected_count", &count.to_string());
            self.prop_row(ui, "props.size", &format_size(total));
        });
    }

    fn prop_row(&self, ui: &mut egui::Ui, key: &str, value: &str) {
        ui.label(self.t(key).to_owned());
        ui.label(value);
        ui.end_row();
    }
}
