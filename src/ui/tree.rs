use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::core::known_folders::{SystemBookmark, SystemBookmarkTarget};
use crate::platform::DiskUsage;
use crate::ui::icons;
use crate::ui::style;

/// Lazy folder tree shown in the left panel.
///
/// Roots are the system drives. A node's children are loaded on demand by a
/// worker thread: `pending` tracks in-flight loads so each path is requested
/// once, and results land in `children`.
pub struct TreeState {
    pub roots: Vec<PathBuf>,
    pub wsl_insert_after: usize,
    pub wsl_expanded: bool,
    pub expanded: HashSet<PathBuf>,
    pub children: HashMap<PathBuf, Vec<PathBuf>>,
    pub failed_children: HashSet<PathBuf>,
    pub pending: HashSet<PathBuf>,
    /// Кэш занятости корневых томов; строки дерева не читают системные данные сами.
    pub disk_usage: HashMap<PathBuf, DiskUsage>,
}

impl TreeState {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let wsl_insert_after = roots.len();
        Self {
            roots,
            wsl_insert_after,
            wsl_expanded: false,
            expanded: HashSet::new(),
            children: HashMap::new(),
            failed_children: HashSet::new(),
            pending: HashSet::new(),
            disk_usage: HashMap::new(),
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
    pub(crate) fn complete_children_load(
        &mut self,
        path: PathBuf,
        result: std::io::Result<Vec<PathBuf>>,
    ) {
        self.pending.remove(&path);
        match result {
            Ok(children) => {
                self.failed_children.remove(&path);
                self.children.insert(path, children);
            }
            Err(_) => {
                self.failed_children.insert(path.clone());
                self.children.insert(path, Vec::new());
            }
        }
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
                let wsl_insert_after = self.tree.wsl_insert_after.min(roots.len());
                // WSL идёт после локальных томов, до сетевых дисков.
                for index in 0..=roots.len() {
                    if index == wsl_insert_after {
                        self.wsl_tree_section(ctx, ui);
                    }
                    if let Some(root) = roots.get(index) {
                        let name = root.display().to_string();
                        self.tree_node(ui, root, name, 0, ctx);
                    }
                }
            });
    }

    fn wsl_tree_section(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        if !self.store.settings.show_wsl_in_tree || self.wsl_distributions.is_empty() {
            return;
        }
        let mut toggle = false;
        let mut refresh = false;
        let expanded = self.tree.wsl_expanded;
        ui.horizontal(|ui| {
            if tree_disclosure_button(ui, expanded) {
                toggle = true;
            }
            let (icon_rect, _) = ui.allocate_exact_size(
                egui::vec2(style::TREE_ICON_WIDTH, style::TREE_ICON_HEIGHT),
                egui::Sense::hover(),
            );
            icons::paint_wsl_penguin_icon(
                ui.painter(),
                icon_rect,
                ui.visuals().selection.stroke.color,
            );
            let response = ui.selectable_label(
                false,
                egui::RichText::new(self.t("tree.wsl.title")).strong(),
            );
            if response.clicked() {
                toggle = true;
            }
            response.context_menu(|ui| {
                style::compact_context_menu(ui);
                if ui.button(self.t("tree.wsl.refresh").to_owned()).clicked() {
                    refresh = true;
                    ui.close();
                }
            });
            if self.wsl_loading {
                ui.spinner();
            }
        });
        if toggle {
            self.tree.wsl_expanded = !expanded;
        }
        if refresh {
            self.refresh_wsl_distributions();
        }
        if self.tree.wsl_expanded {
            for name in self.wsl_distributions.clone() {
                if let Some(path) = crate::platform::wsl_distribution_root(&name) {
                    self.tree_node(ui, &path, name, 1, ctx);
                }
            }
        }
    }

    fn bookmark_section(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let heading = self.t("tree.bookmarks").to_owned();
        ui.label(egui::RichText::new(heading).strong());
        ui.add_space(style::SPACE_XS);

        let system_bookmarks = self.system_bookmarks.clone();
        if !system_bookmarks.is_empty() {
            ui.label(egui::RichText::new(self.t("tree.bookmarks.system")).weak());
            for bookmark in system_bookmarks {
                let active_path = self.tabs[self.active].path.clone();
                let current = matches!(
                    &bookmark.target,
                    SystemBookmarkTarget::Folder(path) if path == &active_path
                );
                let mut action = None;
                ui.horizontal(|ui| {
                    let (icon_rect, icon_response) = ui.allocate_exact_size(
                        egui::vec2(style::TREE_ICON_WIDTH, style::TREE_ICON_HEIGHT),
                        egui::Sense::click(),
                    );
                    icons::paint_system_bookmark_icon(
                        ui.painter(),
                        icon_rect,
                        bookmark.kind,
                        ui.visuals().dark_mode,
                        ui.visuals().selection.stroke.color,
                    );
                    if icon_response.clicked() {
                        action = Some(false);
                    }
                    let response =
                        ui.selectable_label(current, self.t(bookmark.kind.label_key()).to_owned());
                    if response.clicked() {
                        action = Some(false);
                    }
                    response.context_menu(|ui| {
                        style::compact_context_menu(ui);
                        if ui.button(self.t("ctx.open").to_owned()).clicked() {
                            action = Some(false);
                            ui.close();
                        }
                        if matches!(&bookmark.target, SystemBookmarkTarget::Folder(_))
                            && ui.button(self.t("ctx.open_new_tab").to_owned()).clicked()
                        {
                            action = Some(true);
                            ui.close();
                        }
                    });
                });
                if let Some(open_new_tab) = action {
                    self.open_system_bookmark(&bookmark, open_new_tab, ctx);
                }
            }
        }

        let bookmarks = self.store.settings.bookmarks.clone();
        if !bookmarks.is_empty() && !self.system_bookmarks.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(self.t("tree.bookmarks.user")).weak());
            ui.add_space(style::SPACE_XXS);
        }
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
                if ui
                    .button(self.t("ctx.remove_bookmark").to_owned())
                    .clicked()
                {
                    self.store.settings.bookmarks.retain(|b| b != &bm);
                    self.persist();
                    ui.close();
                }
            });
        }
    }

    fn open_system_bookmark(
        &mut self,
        bookmark: &SystemBookmark,
        open_new_tab: bool,
        ctx: &egui::Context,
    ) {
        match &bookmark.target {
            SystemBookmarkTarget::Folder(path) if path.is_dir() => {
                if open_new_tab {
                    self.queue_open_path_in_new_tab(path.clone());
                } else {
                    self.navigate(self.active, path.clone(), ctx);
                }
            }
            SystemBookmarkTarget::Folder(_) => {
                self.system_bookmarks
                    .retain(|entry| entry.kind != bookmark.kind);
            }
            SystemBookmarkTarget::RecycleBin => {
                // Secure и Ask ведут в корзину приложения. Windows-режим — в системную корзину.
                if crate::core::settings::bookmark_opens_secure_trash(
                    self.store.settings.delete_mode,
                ) {
                    self.show_secure_trash = true;
                } else if crate::platform::open_recycle_bin().is_err() {
                    self.system_bookmarks
                        .retain(|entry| entry.kind != bookmark.kind);
                }
            }
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
        // Неизвестную ёмкость скрываем: в кэше её нет, это не зелёная полоска на 0%.
        let usage = if depth == 0 {
            self.tree
                .disk_usage
                .get(path)
                .copied()
                .filter(|usage| usage.used_ratio().is_some())
        } else {
            None
        };
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
            let resp = if depth == 1 && crate::platform::is_wsl_path(path) {
                let reserved = if loading {
                    ui.spacing().interact_size.x + ui.spacing().item_spacing.x
                } else {
                    0.0
                };
                let width = (ui.available_width() - reserved).max(0.0);
                ui.add_sized(
                    egui::vec2(width, ui.spacing().interact_size.y),
                    egui::Button::selectable(is_current, name).truncate(),
                )
            } else {
                ui.selectable_label(is_current, name)
            };
            if resp.clicked() {
                self.navigate(self.active, path.to_path_buf(), ctx);
            }
            let is_bookmarked = self
                .store
                .settings
                .bookmarks
                .iter()
                .any(|bookmark| bookmark == path);
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
                let expand_action = if expanded {
                    "tree.collapse"
                } else {
                    "tree.expand"
                };
                if ui.button(self.t(expand_action).to_owned()).clicked() {
                    self.tree.toggle_expanded(path);
                    ui.close();
                }
            });
            if let Some(usage) = usage {
                let right_reserved = if loading {
                    ui.spacing().interact_size.x + ui.spacing().item_spacing.x
                } else {
                    0.0
                };
                disk_usage_indicator(ui, usage, right_reserved);
            }
            if loading {
                ui.spinner();
            }
        });

        let mut retry_wsl = false;
        if expanded {
            match kids {
                Some(list) => {
                    for child in &list {
                        let cname = child
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.tree_node(ui, child, cname, depth + 1, ctx);
                    }
                    if list.is_empty()
                        && depth == 1
                        && crate::platform::is_wsl_path(path)
                        && self.tree.failed_children.contains(path)
                    {
                        ui.horizontal(|ui| {
                            ui.add_space((depth + 1) as f32 * style::TREE_INDENT);
                            ui.label(egui::RichText::new(self.t("tree.wsl.not_running")).weak());
                            if ui.button(self.t("tree.wsl.retry").to_owned()).clicked() {
                                retry_wsl = true;
                            }
                        });
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
        if retry_wsl {
            self.tree.children.remove(path);
            self.tree.failed_children.remove(path);
            if self.tree.begin_children_load(path) {
                let _ = self.tree_req.send(path.to_path_buf());
            }
        }
    }
}

fn disk_usage_indicator(ui: &mut egui::Ui, usage: DiskUsage, right_reserved: f32) {
    // Текст округляется для чтения. Заливка и её цвет считаются по точной доле.
    let Some(ratio) = usage.used_ratio() else {
        return;
    };
    let Some(percent) = usage.used_percent() else {
        return;
    };
    let label = format!("{percent}%");
    let text_color = ui.visuals().weak_text_color();
    let font = egui::TextStyle::Small.resolve(ui.style());
    let text_width = ui
        .painter()
        .layout_no_wrap(label.clone(), font, text_color)
        .size()
        .x;
    let gap = style::SPACE_XXS;
    let minimum_bar_width = style::SPACE_XXS;
    let width = (ui.available_width() - right_reserved).max(0.0);
    if width < text_width + gap + minimum_bar_width {
        return;
    }

    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.label(
                egui::RichText::new(label)
                    .text_style(egui::TextStyle::Small)
                    .color(text_color),
            );
            let track_width = ui.available_width();
            if track_width < minimum_bar_width {
                return;
            }
            let bar_height = style::SPACE_XS;
            let (track, _) =
                ui.allocate_exact_size(egui::vec2(track_width, bar_height), egui::Sense::hover());
            let radius = egui::CornerRadius::same((bar_height / 2.0).round() as u8);
            let visuals = ui.visuals();
            ui.painter()
                .rect_filled(track, radius, visuals.widgets.inactive.bg_fill);
            if ratio > 0.0 {
                let fill = egui::Rect::from_min_max(
                    track.min,
                    egui::pos2(track.min.x + track.width() * ratio, track.max.y),
                );
                ui.painter()
                    .rect_filled(fill, radius, style::disk_usage_fill(ratio));
            }
        },
    );
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

        tree.complete_children_load(root.clone(), Ok(vec![root.join("a"), root.join("b")]));
        assert!(!tree.pending.contains(&root));
        assert_eq!(tree.children.get(&root).unwrap().len(), 2);
        assert!(!tree.failed_children.contains(&root));
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
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
        );
        assert!(!tree.pending.contains(&root));
        assert_eq!(tree.children.get(&root), Some(&Vec::new()));
        assert!(tree.failed_children.contains(&root));
        assert!(!tree.begin_children_load(&root));
    }
}

/// Paints a compact disclosure chevron so the tree does not depend on font
/// support for Unicode arrow glyphs.
fn tree_disclosure_button(ui: &mut egui::Ui, expanded: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
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
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
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
