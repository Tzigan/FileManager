use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ui::icons;
use crate::ui::style;

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

    /// Toggles a node and returns its new expanded state.
    pub(crate) fn toggle_expanded(&mut self, path: &Path) -> bool {
        if self.expanded.remove(path) {
            false
        } else {
            self.expanded.insert(path.to_path_buf());
            true
        }
    }

    /// Starts at most one lazy child request for an expanded, uncached node.
    pub(crate) fn begin_children_load(&mut self, path: &Path) -> bool {
        if !self.expanded.contains(path)
            || self.children.contains_key(path)
            || self.pending.contains(path)
        {
            return false;
        }
        self.pending.insert(path.to_path_buf());
        true
    }

    /// Settles a lazy request. Failed loads are cached as empty so the UI does
    /// not spin or issue the same failing request on every frame.
    pub(crate) fn complete_children_load(&mut self, path: PathBuf, result: std::io::Result<Vec<PathBuf>>) {
        self.pending.remove(&path);
        self.children.insert(path, result.unwrap_or_default());
    }
}

impl crate::ui::FileManagerApp {
    /// Renders the left panel: bookmarks section and the drive tree.
    pub(crate) fn tree_panel(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                style::dense_vertical_spacing(ui.style_mut());
                self.bookmark_section(ctx, ui);
                ui.separator();
                let heading = self.t("tree.this_pc").to_owned();
                ui.label(egui::RichText::new(heading).strong());
                ui.add_space(style::SPACE_XS);
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
        ui.add_space(style::SPACE_XS);
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
                style::compact_context_menu(ui);
                if ui.button(self.t("ctx.open").to_owned()).clicked() {
                    self.navigate(self.active, bm.clone(), ctx);
                    ui.close();
                }
                if ui.button(self.t("ctx.open_new_tab").to_owned()).clicked() {
                    self.queue_open_path_in_new_tab(bm.clone());
                    ui.close();
                }
                ui.separator();
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
        let indent = depth as f32 * style::TREE_INDENT;
        let expanded = self.tree.expanded.contains(path);
        let kids = self.tree.children.get(path).cloned();
        let loading = self.tree.pending.contains(path);
        // Before the first load we optimistically assume the dir has children.
        let has_arrow = expanded || kids.as_deref().map(|k| !k.is_empty()).unwrap_or(true);

        ui.horizontal(|ui| {
            ui.add_space(indent);
            if has_arrow {
                if tree_disclosure_button(ui, expanded) {
                    self.tree.toggle_expanded(path);
                }
            } else {
                ui.add_space(style::TREE_DISCLOSURE_SIZE);
            }
            let is_current = path == self.tabs[self.active].path.as_path();
            paint_tree_icon(ui, depth == 0, is_current);
            let resp = ui.selectable_label(is_current, name);
            if resp.clicked() {
                self.navigate(self.active, path.to_path_buf(), ctx);
            }
            let is_bookmarked = self.store.settings.bookmarks.iter().any(|bookmark| bookmark == path);
            resp.context_menu(|ui| {
                style::compact_context_menu(ui);
                if ui.button(self.t("ctx.open").to_owned()).clicked() {
                    self.navigate(self.active, path.to_path_buf(), ctx);
                    ui.close();
                }
                if ui.button(self.t("ctx.open_new_tab").to_owned()).clicked() {
                    self.queue_open_path_in_new_tab(path.to_path_buf());
                    ui.close();
                }
                ui.separator();
                let bookmark_action = if is_bookmarked {
                    "ctx.remove_bookmark"
                } else {
                    "ctx.add_bookmark"
                };
                if ui.button(self.t(bookmark_action).to_owned()).clicked() {
                    self.act_bookmark_path(path);
                    ui.close();
                }
                ui.separator();
                let expand_action = if expanded { "tree.collapse" } else { "tree.expand" };
                if ui.button(self.t(expand_action).to_owned()).clicked() {
                    self.tree.toggle_expanded(path);
                    ui.close();
                }
            });
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
                    // Expanded but children not loaded yet - request them.
                    if self.tree.begin_children_load(path) {
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
    fn expand_and_collapse_toggle_the_requested_node_only() {
        let a = PathBuf::from("C:\\a");
        let b = PathBuf::from("C:\\b");
        let mut tree = TreeState::new(vec![]);
        assert!(tree.toggle_expanded(&a));
        assert!(tree.expanded.contains(&a));
        assert!(!tree.expanded.contains(&b));
        assert!(!tree.toggle_expanded(&a));
        assert!(tree.expanded.is_empty());
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

    #[test]
    fn lazy_child_load_is_requested_once_until_it_is_settled() {
        let root = PathBuf::from("C:\\root");
        let mut tree = TreeState::new(vec![root.clone()]);
        assert!(!tree.begin_children_load(&root));

        tree.expanded.insert(root.clone());
        assert!(tree.begin_children_load(&root));
        assert!(tree.pending.contains(&root));
        assert!(!tree.begin_children_load(&root));

        tree.complete_children_load(
            root.clone(),
            Ok(vec![root.join("a"), root.join("b")]),
        );
        assert!(!tree.pending.contains(&root));
        assert_eq!(tree.children.get(&root).unwrap().len(), 2);
        assert!(!tree.begin_children_load(&root));
    }

    #[test]
    fn failed_tree_load_is_settled_as_empty_instead_of_retried_forever() {
        let root = PathBuf::from("C:\\inaccessible");
        let mut tree = TreeState::new(vec![root.clone()]);
        tree.expanded.insert(root.clone());
        assert!(tree.begin_children_load(&root));
        tree.complete_children_load(
            root.clone(),
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied")),
        );
        assert!(!tree.pending.contains(&root));
        assert_eq!(tree.children.get(&root), Some(&Vec::new()));
        assert!(!tree.begin_children_load(&root));
    }
}

/// Paints a compact disclosure chevron so the tree does not depend on font
/// support for Unicode arrow glyphs.
fn tree_disclosure_button(ui: &mut egui::Ui, expanded: bool) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(
            egui::vec2(style::TREE_DISCLOSURE_SIZE, style::TREE_DISCLOSURE_SIZE),
            egui::Sense::click(),
        );
    let color = if response.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let c = rect.center();
    let points = if expanded {
        vec![
            egui::pos2(c.x - 4.0, c.y - 2.0),
            egui::pos2(c.x, c.y + 2.0),
            egui::pos2(c.x + 4.0, c.y - 2.0),
        ]
    } else {
        vec![
            egui::pos2(c.x - 2.0, c.y - 4.0),
            egui::pos2(c.x + 2.0, c.y),
            egui::pos2(c.x - 2.0, c.y + 4.0),
        ]
    };
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(1.6, color),
    ));
    response.clicked()
}

/// Paints folder/drive icons in vector form instead of relying on emoji fonts.
fn paint_tree_icon(ui: &mut egui::Ui, is_drive: bool, current: bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(style::TREE_ICON_WIDTH, style::TREE_ICON_HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    if is_drive {
        let frame = egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 11.0));
        let fill = if current {
            ui.visuals().selection.bg_fill
        } else {
            egui::Color32::from_rgb(105, 122, 139)
        };
        painter.rect_filled(frame, style::RADIUS_ICON, fill);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(frame.left() + 2.0, frame.top() + 2.0),
                egui::pos2(frame.right() - 2.0, frame.bottom() - 3.0),
            ),
            style::RADIUS_ICON,
            egui::Color32::from_rgb(45, 57, 69),
        );
        painter.circle_filled(
            egui::pos2(frame.right() - 3.0, frame.bottom() - 1.5),
            style::RADIUS_ICON * 0.5,
            egui::Color32::from_rgb(125, 218, 140),
        );
        return;
    }

    icons::paint_folder_icon(painter, rect);
}
