use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Lazy folder tree shown in the left panel.
///
/// Roots are the system drives. A node's children are loaded on demand by a
/// worker thread: `pending` tracks in-flight loads so each path is requested
/// once, and results land in `children`.
pub struct TreeState {
    pub roots: Vec<PathBuf>,
    pub expanded: HashSet<PathBuf>,
    pub children: HashMap<PathBuf, Vec<PathBuf>>,
    pub pending: HashSet<PathBuf>,
}

impl TreeState {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            expanded: HashSet::new(),
            children: HashMap::new(),
            pending: HashSet::new(),
        }
    }

    /// Marks every ancestor of `path` as expanded so the current folder is
    /// reachable in the tree. Children load lazily when the nodes render.
    pub fn reveal(&mut self, path: &Path) {
        for ancestor in path.ancestors().skip(1) {
            self.expanded.insert(ancestor.to_path_buf());
        }
    }
}

impl crate::ui::FileManagerApp {
    /// Renders the left panel: bookmarks section and the drive tree.
    pub(crate) fn tree_panel(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.bookmark_section(ctx, ui);
                ui.separator();
                ui.horizontal(|ui| {
                    let heading = self.t("tree.this_pc").to_owned();
                    ui.label(egui::RichText::new(heading).strong());
                    if ui.small_button("⟳").clicked() {
                        self.tree.children.clear();
                        self.tree.pending.clear();
                    }
                });
                ui.add_space(4.0);
                let roots = self.tree.roots.clone();
                for root in roots {
                    let name = root.display().to_string();
                    self.tree_node(ui, &root, name, 0, ctx);
                }
            });
    }

    fn bookmark_section(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let heading = self.t("tree.bookmarks").to_owned();
        ui.label(egui::RichText::new(heading).strong());
        ui.add_space(4.0);
        let bookmarks = self.store.settings.bookmarks.clone();
        for bm in bookmarks {
            let name = bm
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| bm.display().to_string());
            let resp = ui.selectable_label(false, format!("  {}", name));
            if resp.clicked() {
                self.navigate(self.active, bm.clone(), ctx);
            }
            resp.context_menu(|ui| {
                if ui.button(self.t("ctx.remove_bookmark").to_owned()).clicked() {
                    self.store.settings.bookmarks.retain(|b| b != &bm);
                    self.persist();
                    ui.close();
                }
            });
        }
    }

    /// One expandable row of the folder tree.
    fn tree_node(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        name: String,
        depth: usize,
        ctx: &egui::Context,
    ) {
        if depth > 24 {
            return;
        }
        let indent = depth as f32 * 14.0;
        let expanded = self.tree.expanded.contains(path);
        let kids = self.tree.children.get(path).cloned();
        let loading = self.tree.pending.contains(path);
        // Before the first load we optimistically assume the dir has children.
        let has_arrow = expanded || kids.as_deref().map(|k| !k.is_empty()).unwrap_or(true);

        ui.horizontal(|ui| {
            ui.add_space(indent);
            if has_arrow {
                let glyph = if expanded { "▾" } else { "▸" };
                if ui.small_button(glyph).clicked() {
                    if expanded {
                        self.tree.expanded.remove(path);
                    } else {
                        self.tree.expanded.insert(path.to_path_buf());
                    }
                }
            } else {
                ui.add_space(18.0);
            }
            let is_current = path == self.tabs[self.active].path.as_path();
            let resp = ui.selectable_label(is_current, name);
            if resp.clicked() {
                self.navigate(self.active, path.to_path_buf(), ctx);
            }
            if loading {
                ui.spinner();
            }
        });

        if expanded {
            match kids {
                Some(list) => {
                    for child in list {
                        let cname = child
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.tree_node(ui, &child, cname, depth + 1, ctx);
                    }
                }
                None => {
                    // Expanded but children not loaded yet — request them.
                    if !loading {
                        self.tree.pending.insert(path.to_path_buf());
                        let _ = self.tree_req.send(path.to_path_buf());
                    }
                }
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tree_is_collapsed() {
        let t = TreeState::new(vec![PathBuf::from("C:\\")]);
        assert!(t.expanded.is_empty() && t.children.is_empty() && t.pending.is_empty());
    }

    #[test]
    fn reveal_expands_ancestors() {
        let mut t = TreeState::new(vec![PathBuf::from("C:\\")]);
        t.reveal(Path::new("C:\\a\\b\\c"));
        assert!(t.expanded.contains(Path::new("C:\\a\\b")));
        assert!(t.expanded.contains(Path::new("C:\\a")));
        assert!(t.expanded.contains(Path::new("C:\\")));
        assert!(!t.expanded.contains(Path::new("C:\\a\\b\\c")));
    }
}
