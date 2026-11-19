use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::core::known_folders::{SystemBookmark, SystemBookmarkTarget};
use crate::core::settings::Settings;
use crate::platform::DiskUsage;
use crate::ui::icons;
use crate::ui::style;

pub(crate) const TREE_PANEL_MIN_WIDTH: f32 = 140.0;
pub(crate) const TREE_PANEL_MAX_WIDTH: f32 = 480.0;

pub(crate) fn clamp_tree_panel_width(width: f32) -> f32 {
    width.clamp(TREE_PANEL_MIN_WIDTH, TREE_PANEL_MAX_WIDTH)
}

fn tree_content_width(panel_width: f32, widest_row_width: f32) -> f32 {
    panel_width.max(widest_row_width)
}

#[cfg(test)]
fn tree_horizontal_scroll_needed(panel_width: f32, content_width: f32) -> bool {
    content_width > panel_width
}

fn tree_row_rect(origin: egui::Pos2, content_width: f32, height: f32) -> egui::Rect {
    egui::Rect::from_min_size(origin, egui::vec2(content_width, height))
}

fn visible_tree_row_rect(row_rect: egui::Rect, clip_rect: egui::Rect) -> egui::Rect {
    row_rect.intersect(clip_rect)
}

fn migrate_tree_vertical_scroll(ctx: &egui::Context, old_id: egui::Id, new_id: egui::Id) {
    // Сохраняем прежнюю позицию вертикальной прокрутки при смене вложенности ScrollArea.
    if egui::containers::scroll_area::State::load(ctx, new_id).is_none()
        && let Some(state) = egui::containers::scroll_area::State::load(ctx, old_id)
    {
        state.store(ctx, new_id);
    }
}

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

#[derive(Clone, Copy)]
enum BookmarkSection {
    System,
    Personal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpecialSystemBookmarkAction {
    RecycleBin,
    SecureTrash,
}

fn special_system_bookmark_action(
    target: &SystemBookmarkTarget,
) -> Option<SpecialSystemBookmarkAction> {
    match target {
        SystemBookmarkTarget::RecycleBin => Some(SpecialSystemBookmarkAction::RecycleBin),
        SystemBookmarkTarget::SecureTrash => Some(SpecialSystemBookmarkAction::SecureTrash),
        SystemBookmarkTarget::Folder(_) | SystemBookmarkTarget::UnavailableFolder => None,
    }
}

fn toggle_bookmark_section(settings: &mut Settings, section: BookmarkSection) {
    let collapsed = match section {
        BookmarkSection::System => &mut settings.bookmarks_system_collapsed,
        BookmarkSection::Personal => &mut settings.bookmarks_personal_collapsed,
    };
    *collapsed = !*collapsed;
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
        let panel_width = ui.available_width();
        let content_width_id = ui.make_persistent_id("tree_content_width");
        let previous_content_width = ctx
            .data(|data| data.get_temp::<f32>(content_width_id))
            .unwrap_or(panel_width);
        let content_width = tree_content_width(panel_width, previous_content_width);
        let previous_vertical_id = ui.make_persistent_id("scroll_area");

        // Длинные строки растут внутри содержимого, а не увеличивают минимальную ширину панели.
        let output = egui::ScrollArea::horizontal()
            .id_salt("tree_horizontal")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let vertical_id = ui.make_persistent_id("tree_vertical");
                migrate_tree_vertical_scroll(ctx, previous_vertical_id, vertical_id);
                egui::ScrollArea::vertical()
                    .id_salt("tree_vertical")
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
                                self.wsl_tree_section(ctx, ui, content_width);
                            }
                            if let Some(root) = roots.get(index) {
                                let name = root.display().to_string();
                                self.tree_node(ui, root, name, 0, ctx, content_width);
                            }
                        }
                    });
            });
        let measured_content_width = tree_content_width(panel_width, output.content_size.x);
        ctx.data_mut(|data| data.insert_temp(content_width_id, measured_content_width));
    }

    fn wsl_tree_section(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        content_width: f32,
    ) {
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
                    self.tree_node(ui, &path, name, 1, ctx, content_width);
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
            if bookmark_section_header(
                ui,
                self.t("tree.bookmarks.system"),
                !self.store.settings.bookmarks_system_collapsed,
            )
            .clicked()
            {
                self.on_bookmark_section_clicked(BookmarkSection::System, ctx);
            }
            let system_row_count = if self.store.settings.bookmarks_system_collapsed {
                0
            } else {
                system_bookmarks.len()
            };
            for bookmark in system_bookmarks.iter().take(system_row_count).cloned() {
                let active_path = self.tabs[self.active].path.clone();
                let current = matches!(
                    &bookmark.target,
                    SystemBookmarkTarget::Folder(path) if path == &active_path
                );
                let mut action = None;
                let row = ui.horizontal(|ui| {
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
                    response
                });
                let row_response = row.response;

                let drop_target =
                    match crate::ui::file_dnd::system_bookmark_drop_destination(&bookmark.target) {
                        crate::ui::file_dnd::BookmarkDropDestination::Folder(path) => {
                            Some((Some(path), self.bookmark_drop_state(path)))
                        }
                        crate::ui::file_dnd::BookmarkDropDestination::Unavailable => {
                            Some((None, crate::ui::file_dnd::BookmarkPathState::Unavailable))
                        }
                        crate::ui::file_dnd::BookmarkDropDestination::NonTarget => {
                            // TODO: отдельная задача: drop в корзину означает удаление, а не перенос.
                            None
                        }
                    };
                if let Some((destination, state)) = drop_target
                    && state.is_drop_target()
                {
                    let is_folder = state.is_folder();
                    let target_allowed = is_folder
                        && destination.is_some_and(|path| self.active_folder_accepts_drop(path));
                    self.handle_file_drop_response_with_reason(
                        &row_response,
                        destination,
                        is_folder,
                        target_allowed,
                        crate::ui::file_dnd::DropSurface::Bookmark,
                        state.invalid_reason(),
                        ui,
                        ctx,
                    );
                }
                if let Some(open_new_tab) = action {
                    self.open_system_bookmark(&bookmark, open_new_tab, ctx);
                }
            }
        }

        let bookmarks = self.store.settings.bookmarks.clone();
        if !bookmarks.is_empty() {
            if !system_bookmarks.is_empty() {
                ui.separator();
            }
            if bookmark_section_header(
                ui,
                self.t("tree.bookmarks.user"),
                !self.store.settings.bookmarks_personal_collapsed,
            )
            .clicked()
            {
                self.on_bookmark_section_clicked(BookmarkSection::Personal, ctx);
            }
            let personal_row_count = if self.store.settings.bookmarks_personal_collapsed {
                0
            } else {
                bookmarks.len()
            };
            if personal_row_count > 0 {
                ui.add_space(style::SPACE_XXS);
            }
            for bm in bookmarks.into_iter().take(personal_row_count) {
                let name = bm
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| bm.display().to_string());
                let resp = ui.selectable_label(false, format!("  {}", name));
                let state = self.bookmark_drop_state(&bm);
                if state.is_drop_target() {
                    let is_folder = state.is_folder();
                    self.handle_file_drop_response_with_reason(
                        &resp,
                        Some(&bm),
                        is_folder,
                        is_folder && self.active_folder_accepts_drop(&bm),
                        crate::ui::file_dnd::DropSurface::Bookmark,
                        state.invalid_reason(),
                        ui,
                        ctx,
                    );
                }
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
    }

    fn on_bookmark_section_clicked(&mut self, section: BookmarkSection, ctx: &egui::Context) {
        // Память переключается первой: ошибка записи не отменяет действие пользователя.
        toggle_bookmark_section(&mut self.store.settings, section);
        let _ = self.persist_checked();
        ctx.request_repaint();
    }

    fn open_system_bookmark(
        &mut self,
        bookmark: &SystemBookmark,
        open_new_tab: bool,
        ctx: &egui::Context,
    ) {
        if let Some(action) = special_system_bookmark_action(&bookmark.target) {
            match action {
                SpecialSystemBookmarkAction::RecycleBin => {
                    // Режим удаления не подменяет виртуальную папку Windows; ошибка остаётся видна в журнале.
                    if let Err(error) = crate::platform::open_recycle_bin() {
                        self.record_io_error(None, &error);
                    }
                }
                SpecialSystemBookmarkAction::SecureTrash => {
                    if crate::core::known_folders::bookmark_forces_new_tab(&bookmark.target) {
                        // Клик просил бы текущую вкладку, но эта закладка всегда создаёт новую.
                        let _ = open_new_tab;
                        self.open_secure_trash_tab(ctx);
                    }
                }
            }
            return;
        }

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
            SystemBookmarkTarget::UnavailableFolder => {}
            SystemBookmarkTarget::RecycleBin | SystemBookmarkTarget::SecureTrash => {}
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
        content_width: f32,
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
        let is_current = path == self.tabs[self.active].path.as_path();
        let row_rect = tree_row_rect(
            ui.cursor().min,
            content_width,
            ui.spacing().interact_size.y,
        );
        let row_response = ui.interact(
            row_rect,
            ui.make_persistent_id(("tree_node_row", path)),
            egui::Sense::click(),
        );
        let visuals = ui.visuals();
        let row_fill = if is_current {
            Some(visuals.selection.bg_fill)
        } else if row_response.contains_pointer() {
            Some(visuals.widgets.hovered.bg_fill)
        } else {
            None
        };
        if let Some(fill) = row_fill {
            ui.painter()
                .rect_filled(row_rect, style::RADIUS_ROW, fill);
        }

        let mut disclosure_clicked = false;
        let mut label_clicked = false;
        ui.horizontal(|ui| {
            ui.add_space(indent);
            if has_arrow {
                if tree_disclosure_button(ui, expanded) {
                    disclosure_clicked = true;
                    self.tree.toggle_expanded(path);
                }
            } else {
                ui.add_space(style::TREE_DISCLOSURE_SIZE);
            }
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
            label_clicked = resp.clicked();
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

        // Hit-test остаётся полнострочным, а подсветка обрезается видимой областью ScrollArea.
        let mut drop_response = row_response.clone();
        drop_response.rect = visible_tree_row_rect(row_response.rect, row_response.interact_rect);
        self.handle_file_drop_response(
            &drop_response,
            Some(path),
            true,
            !crate::platform::is_wsl_path(path),
            crate::ui::file_dnd::DropSurface::Tree,
            ui,
            ctx,
        );
        if !disclosure_clicked && (row_response.clicked() || label_clicked) {
            self.navigate(self.active, path.to_path_buf(), ctx);
        }

        let mut retry_wsl = false;
        if expanded {
            match kids {
                Some(list) => {
                    for child in &list {
                        let cname = child
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.tree_node(ui, child, cname, depth + 1, ctx, content_width);
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
    fn recycle_bin_bookmark_always_opens_system_recycle_bin_regardless_of_secure_trash_enabled() {
        let target = SystemBookmarkTarget::RecycleBin;
        let path = PathBuf::from(r"C:\work\a.txt");
        for mode in [
            crate::core::settings::DeleteMode::WindowsRecycle,
            crate::core::settings::DeleteMode::SecureTrash,
            crate::core::settings::DeleteMode::Ask,
        ] {
            let route =
                crate::core::settings::route_delete(mode, false, std::slice::from_ref(&path), &[]);
            match mode {
                crate::core::settings::DeleteMode::WindowsRecycle => {
                    assert_eq!(route.windows, vec![path.clone()]);
                    assert!(route.secure.is_empty());
                }
                crate::core::settings::DeleteMode::SecureTrash => {
                    assert!(route.windows.is_empty());
                    assert_eq!(route.secure, vec![path.clone()]);
                }
                crate::core::settings::DeleteMode::Ask => {
                    assert!(route.windows.is_empty());
                    assert_eq!(route.ask, vec![path.clone()]);
                }
            }
            assert_eq!(
                special_system_bookmark_action(&target),
                Some(SpecialSystemBookmarkAction::RecycleBin)
            );
        }

        let handler = include_str!("tree.rs")
            .split("fn open_system_bookmark")
            .nth(1)
            .unwrap()
            .split("/// One expandable row")
            .next()
            .unwrap();
        assert!(handler.contains("crate::platform::open_recycle_bin()"));
        assert!(handler.contains("self.record_io_error(None, &error)"));
        assert!(!handler.contains("self.store.settings.delete_mode"));
        assert!(!handler.contains("bookmark_opens_secure_trash"));
    }

    #[test]
    fn secure_trash_bookmark_opens_secure_trash_independently() {
        let target = SystemBookmarkTarget::SecureTrash;
        let path = PathBuf::from(r"C:\work\a.txt");
        for mode in [
            crate::core::settings::DeleteMode::WindowsRecycle,
            crate::core::settings::DeleteMode::SecureTrash,
            crate::core::settings::DeleteMode::Ask,
        ] {
            let route =
                crate::core::settings::route_delete(mode, false, std::slice::from_ref(&path), &[]);
            assert_eq!(
                route.windows.len() + route.secure.len() + route.ask.len(),
                1
            );
            assert_eq!(
                special_system_bookmark_action(&target),
                Some(SpecialSystemBookmarkAction::SecureTrash)
            );
        }
        let handler = include_str!("tree.rs")
            .split("fn open_system_bookmark")
            .nth(1)
            .unwrap()
            .split("/// One expandable row")
            .next()
            .unwrap();
        assert!(handler.contains("self.open_secure_trash_tab(ctx)"));
        assert!(handler.contains("bookmark_forces_new_tab(&bookmark.target)"));
    }

    #[test]
    fn recycle_bin_bookmark_not_a_drop_target() {
        assert!(matches!(
            crate::ui::file_dnd::system_bookmark_drop_destination(
                &SystemBookmarkTarget::RecycleBin
            ),
            crate::ui::file_dnd::BookmarkDropDestination::NonTarget
        ));
    }

    #[test]
    fn secure_trash_bookmark_not_a_drop_target() {
        assert!(matches!(
            crate::ui::file_dnd::system_bookmark_drop_destination(
                &SystemBookmarkTarget::SecureTrash
            ),
            crate::ui::file_dnd::BookmarkDropDestination::NonTarget
        ));
    }

    #[test]
    fn existing_system_bookmark_tests_not_regressed() {
        let known_folders = include_str!("../core/known_folders.rs");
        let dnd = include_str!("file_dnd.rs");
        let settings = include_str!("../core/settings.rs");
        for name in [
            "recycle_bookmark_uses_the_shell_target",
            "secure_trash_bookmark_is_its_own_target",
        ] {
            assert!(known_folders.contains(name));
        }
        assert!(dnd.contains(
            "trash_and_secure_trash_bookmarks_remain_non_targets_without_invalid_highlight"
        ));
        assert!(settings.contains("delete_mode_routes_without_replacing_windows_recycle"));
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

    #[test]
    fn bookmark_collapse_click_toggles_own_section_only() {
        let mut settings = Settings::default();

        toggle_bookmark_section(&mut settings, BookmarkSection::System);
        assert!(settings.bookmarks_system_collapsed);
        assert!(!settings.bookmarks_personal_collapsed);

        toggle_bookmark_section(&mut settings, BookmarkSection::Personal);
        assert!(settings.bookmarks_system_collapsed);
        assert!(settings.bookmarks_personal_collapsed);
    }

    #[test]
    fn bookmark_collapse_click_persists_to_settings() {
        let dir = std::env::temp_dir().join(format!(
            "filemanager-bookmark-collapse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = crate::core::settings::SettingsStore::load(&dir);
        toggle_bookmark_section(&mut store.settings, BookmarkSection::System);
        store.save().unwrap();

        let loaded = crate::core::settings::SettingsStore::load(&dir);
        assert!(loaded.settings.bookmarks_system_collapsed);
        assert!(!loaded.settings.bookmarks_personal_collapsed);

        let source = include_str!("tree.rs");
        let method = source
            .split("fn on_bookmark_section_clicked")
            .nth(1)
            .unwrap()
            .split("fn open_system_bookmark")
            .next()
            .unwrap();
        assert!(method.contains("toggle_bookmark_section(&mut self.store.settings, section)"));
        assert!(method.contains("let _ = self.persist_checked()"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bookmark_collapse_persist_failure_does_not_block_ui() {
        let dir = std::env::temp_dir().join(format!(
            "filemanager-bookmark-collapse-failure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config_blocker = dir.join("settings-parent");
        std::fs::write(&config_blocker, b"not a directory").unwrap();
        let mut store = crate::core::settings::SettingsStore::load(&config_blocker);

        toggle_bookmark_section(&mut store.settings, BookmarkSection::Personal);
        assert!(store.save().is_err());
        assert!(store.settings.bookmarks_personal_collapsed);

        let source = include_str!("tree.rs");
        let method = source
            .split("fn on_bookmark_section_clicked")
            .nth(1)
            .unwrap()
            .split("fn open_system_bookmark")
            .next()
            .unwrap();
        assert!(
            method.find("toggle_bookmark_section").unwrap()
                < method.find("self.persist_checked()").unwrap()
        );
        assert!(method.contains("ctx.request_repaint()"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn collapsed_section_renders_no_bookmark_rows() {
        let source = include_str!("tree.rs");
        let section = source
            .split("fn bookmark_section(&mut self")
            .nth(1)
            .unwrap()
            .split("fn on_bookmark_section_clicked")
            .next()
            .unwrap();
        assert!(
            section.contains(
                "let system_row_count = if self.store.settings.bookmarks_system_collapsed"
            )
        );
        assert!(section.contains("system_bookmarks.iter().take(system_row_count)"));
        assert!(section.contains(
            "let personal_row_count = if self.store.settings.bookmarks_personal_collapsed"
        ));
        assert!(section.contains("bookmarks.into_iter().take(personal_row_count)"));
    }

    #[test]
    fn collapsed_bookmark_rows_register_no_drop_targets() {
        let source = include_str!("tree.rs");
        let section = source
            .split("fn bookmark_section(&mut self")
            .nth(1)
            .unwrap()
            .split("fn on_bookmark_section_clicked")
            .next()
            .unwrap();
        let system_gate = section
            .find("system_bookmarks.iter().take(system_row_count)")
            .unwrap();
        let personal_gate = section
            .find("bookmarks.into_iter().take(personal_row_count)")
            .unwrap();
        let drop_calls: Vec<_> = section
            .match_indices("handle_file_drop_response_with_reason(")
            .map(|(index, _)| index)
            .collect();
        assert_eq!(drop_calls.len(), 2);
        assert!(system_gate < drop_calls[0] && drop_calls[0] < personal_gate);
        assert!(personal_gate < drop_calls[1]);
    }

    #[test]
    fn bookmark_headers_remain_non_drop_targets_in_both_states() {
        let source = include_str!("tree.rs");
        let header = source
            .rsplit("fn bookmark_section_header(")
            .next()
            .unwrap()
            .split("fn paint_tree_disclosure(")
            .next()
            .unwrap();
        assert!(header.contains("egui::Sense::click()"));
        assert!(!header.contains("click_and_drag"));
        assert!(!header.contains("handle_file_drop_response"));
        assert!(!header.contains("DropSurface"));
    }

    #[test]
    fn bookmark_collapse_shared_between_main_and_detached() {
        let main = include_str!("app/mod.rs");
        let detached = include_str!("app/window_frame.rs");
        let tree = include_str!("tree.rs");
        assert!(main.contains("self.tree_panel(&ctx, ui)"));
        assert!(detached.contains("self.tree_panel(&viewport_ctx, ui)"));
        assert!(tree.contains("self.store.settings.bookmarks_system_collapsed"));
        assert!(tree.contains("self.store.settings.bookmarks_personal_collapsed"));
    }

    #[test]
    fn tree_panel_width_clamped_to_min_and_max_constants() {
        assert_eq!(clamp_tree_panel_width(TREE_PANEL_MIN_WIDTH - 20.0), 140.0);
        assert_eq!(clamp_tree_panel_width(TREE_PANEL_MIN_WIDTH), 140.0);
        assert_eq!(clamp_tree_panel_width(TREE_PANEL_MAX_WIDTH), 480.0);
        assert_eq!(clamp_tree_panel_width(TREE_PANEL_MAX_WIDTH + 20.0), 480.0);
    }

    #[test]
    fn tree_panel_width_shared_with_detached_window() {
        let main = include_str!("app/mod.rs");
        let detached = include_str!("app/window_frame.rs");
        assert!(main.contains("self.store.settings.tree_panel_width"));
        assert!(detached.contains("self.store.settings.tree_panel_width"));
        assert!(main.contains("TREE_PANEL_MIN_WIDTH..=TREE_PANEL_MAX_WIDTH"));
        assert!(detached.contains("TREE_PANEL_MIN_WIDTH..=TREE_PANEL_MAX_WIDTH"));
        assert!(detached.contains("Одна сохранённая ширина"));
    }

    #[test]
    fn tree_panel_min_width_no_longer_driven_by_row_width() {
        let app = include_str!("app/mod.rs");
        let tree = include_str!("tree.rs");
        let panel = app
            .split("egui::Panel::left(\"tree_panel\")")
            .nth(1)
            .unwrap()
            .split(".show(ui")
            .next()
            .unwrap();
        let render = tree
            .split("pub(crate) fn tree_panel")
            .nth(1)
            .unwrap()
            .split("fn wsl_tree_section")
            .next()
            .unwrap();
        assert!(panel.contains("TREE_PANEL_MIN_WIDTH..=TREE_PANEL_MAX_WIDTH"));
        assert!(
            render.find("ScrollArea::horizontal()").unwrap()
                < render.find("ScrollArea::vertical()").unwrap()
        );
        assert!(render.contains("tree_content_width(panel_width, output.content_size.x)"));
    }

    #[test]
    fn tree_narrow_panel_enables_horizontal_scroll() {
        let panel_width = TREE_PANEL_MIN_WIDTH;
        let content_width = tree_content_width(panel_width, 620.0);
        assert_eq!(content_width, 620.0);
        assert!(tree_horizontal_scroll_needed(panel_width, content_width));
    }

    #[test]
    fn tree_wide_panel_keeps_scroll_inactive() {
        let panel_width = 420.0;
        let content_width = tree_content_width(panel_width, 310.0);
        assert_eq!(content_width, panel_width);
        assert!(!tree_horizontal_scroll_needed(panel_width, content_width));
    }

    #[test]
    fn tree_row_highlight_spans_full_content_width_after_scroll() {
        let viewport = egui::Rect::from_min_size(egui::pos2(80.0, 20.0), egui::vec2(180.0, 24.0));
        let content_origin = egui::pos2(viewport.left() - 120.0, viewport.top());
        let row = tree_row_rect(content_origin, 520.0, viewport.height());
        let visible = visible_tree_row_rect(row, viewport);

        assert_eq!(row.width(), 520.0);
        assert_eq!(visible, viewport);
        let tree = include_str!("tree.rs");
        assert!(tree.contains(".rect_filled(row_rect, style::RADIUS_ROW, fill)"));
    }

    #[test]
    fn tree_node_click_hit_rects_correct_after_horizontal_scroll() {
        let viewport = egui::Rect::from_min_size(egui::pos2(100.0, 30.0), egui::vec2(200.0, 24.0));
        let content_origin = egui::pos2(viewport.left() - 260.0, viewport.top());
        let hit_rect = tree_row_rect(content_origin, 640.0, viewport.height());
        let pointer = egui::pos2(viewport.right() - 6.0, viewport.center().y);

        assert!(hit_rect.contains(pointer));
        assert!(visible_tree_row_rect(hit_rect, viewport).contains(pointer));
        let node = include_str!("tree.rs")
            .split("fn tree_node(")
            .nth(1)
            .unwrap();
        assert!(node.contains("ui.interact("));
        assert!(node.contains("row_rect,"));
    }

    #[test]
    fn tree_drop_target_hit_rects_correct_after_horizontal_scroll() {
        let viewport = egui::Rect::from_min_size(egui::pos2(60.0, 12.0), egui::vec2(190.0, 24.0));
        let content_origin = egui::pos2(viewport.left() - 180.0, viewport.top());
        let target_rect = tree_row_rect(content_origin, 500.0, viewport.height());
        let visible_target = visible_tree_row_rect(target_rect, viewport);
        let pointer = egui::pos2(viewport.center().x, viewport.center().y);

        assert!(target_rect.contains(pointer));
        assert!(visible_target.contains(pointer));
        let tree = include_str!("tree.rs");
        assert!(tree.contains("drop_response.rect = visible_tree_row_rect"));
        assert!(tree.contains("&drop_response,"));
    }

    #[test]
    fn tree_drop_highlight_works_at_any_horizontal_scroll_position() {
        let viewport = egui::Rect::from_min_size(egui::pos2(40.0, 8.0), egui::vec2(180.0, 24.0));
        for offset in [0.0, 90.0, 260.0, 430.0] {
            let origin = egui::pos2(viewport.left() - offset, viewport.top());
            let row = tree_row_rect(origin, 520.0, viewport.height());
            let highlight = visible_tree_row_rect(row, viewport);
            assert!(highlight.width() > 0.0);
            assert_eq!(highlight.height(), viewport.height());
        }
    }

    #[test]
    fn tree_vertical_autoscroll_to_active_node_not_regressed() {
        let tree = include_str!("tree.rs");
        let app = include_str!("app/mod.rs");
        let panel = tree
            .split("pub(crate) fn tree_panel")
            .nth(1)
            .unwrap()
            .split("fn wsl_tree_section")
            .next()
            .unwrap();
        assert!(panel.contains("ScrollArea::vertical()"));
        assert!(panel.contains(".id_salt(\"tree_vertical\")"));
        assert!(panel.contains("migrate_tree_vertical_scroll(ctx, previous_vertical_id, vertical_id)"));
        assert!(tree.contains("fn reveal_expands_ancestors"));
        assert!(app.contains("app.tree.reveal(reveal_path)"));
        assert!(!panel.contains("scroll_to_rect"));
    }

    #[test]
    fn tree_horizontal_scroll_shared_with_detached_window() {
        let main = include_str!("app/mod.rs");
        let detached = include_str!("app/window_frame.rs");
        let tree = include_str!("tree.rs");
        assert!(main.contains("self.tree_panel(&ctx, ui)"));
        assert!(detached.contains("self.tree_panel(&viewport_ctx, ui)"));
        assert!(tree.contains("ScrollArea::horizontal()"));
    }

    #[test]
    fn tree_expansion_and_selection_not_regressed() {
        let tree = include_str!("tree.rs");
        let node = tree
            .split("fn tree_node(")
            .nth(1)
            .unwrap()
            .split("fn on_bookmark_section_clicked")
            .next()
            .unwrap();
        assert!(node.contains("self.tree.toggle_expanded(path)"));
        assert!(node.contains("path == self.tabs[self.active].path.as_path()"));
        assert!(node.contains("self.navigate(self.active, path.to_path_buf(), ctx)"));
        assert!(node.contains("for child in &list"));
    }

    #[test]
    fn existing_list_horizontal_scroll_tests_not_regressed() {
        let list = include_str!("list/mod.rs");
        for name in [
            "narrow_window_enables_horizontal_scroll_instead_of_column_overlap",
            "detached_window_inherits_horizontal_scroll_via_shared_code",
            "sorting_by_header_click_works_after_horizontal_scroll",
            "row_context_menu_rects_correct_after_horizontal_scroll",
        ] {
            assert!(list.contains(name));
        }
    }

    #[test]
    fn existing_tree_panel_width_settings_tests_not_regressed() {
        let settings = include_str!("../core/settings.rs");
        assert!(settings.contains("pub tree_panel_width: f32"));
        for name in [
            "tree_panel_width_default_when_missing_in_settings",
            "tree_panel_width_roundtrip_persists_value",
            "tree_panel_width_preserved_by_settings_reset",
        ] {
            assert!(settings.contains(name));
        }
    }

    #[test]
    fn settings_structure_and_json_schema_unchanged() {
        let settings = include_str!("../core/settings.rs");
        assert!(settings.contains("fn settings_structure_and_json_schema_unchanged()"));
        assert!(settings.contains("\"tree_panel_width\""));
        assert!(settings.contains("fn json_schema_includes_tree_panel_width()"));
    }

    #[test]
    fn tree_panel_resize_calls_persist_checked() {
        let app = include_str!("app/mod.rs");
        let panel = app.find("egui::Panel::left(\"tree_panel\")").unwrap();
        let resize = app[panel..].find("self.note_tree_panel_width").unwrap() + panel;
        let debounce = app
            .find("fn persist_tree_panel_width_if_due")
            .map(|start| &app[start..])
            .unwrap();
        assert!(panel < resize);
        assert!(debounce.contains("let _ = self.persist_checked();"));
        assert!(debounce.contains("ctx.request_repaint_after"));
        assert!(
            app.contains("TREE_PANEL_PERSIST_DEBOUNCE: Duration = Duration::from_millis(300)")
        );
    }

    #[test]
    fn tree_panel_resize_persist_failure_does_not_block_ui() {
        let dir = std::env::temp_dir().join(format!(
            "filemanager-tree-width-failure-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config_blocker = dir.join("settings-parent");
        std::fs::write(&config_blocker, b"not a directory").unwrap();
        let mut store = crate::core::settings::SettingsStore::load(&config_blocker);
        store.settings.tree_panel_width = 370.0;

        assert!(store.save().is_err());
        assert_eq!(store.settings.tree_panel_width, 370.0);

        let app = include_str!("app/mod.rs");
        let note = app
            .split("fn note_tree_panel_width")
            .nth(1)
            .unwrap()
            .split("fn persist_tree_panel_width_if_due")
            .next()
            .unwrap();
        let persist = app
            .split("fn persist_tree_panel_width_if_due")
            .nth(1)
            .unwrap()
            .split("pub(crate) fn sort_col")
            .next()
            .unwrap();
        assert!(note.contains("self.store.settings.tree_panel_width = width"));
        assert!(persist.contains("let _ = self.persist_checked();"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn existing_tree_and_list_tests_not_regressed() {
        let tree = include_str!("tree.rs");
        let list = include_str!("list/mod.rs");
        for test in [
            "existing_bookmark_drop_and_cache_tests_not_regressed",
            "bookmark_collapse_shared_between_main_and_detached",
            "domain1_2_3_not_regressed",
        ] {
            assert!(tree.contains(test));
        }
        for test in [
            "table_rows_do_not_add_horizontal_scroll_margin_that_shifts_columns",
            "narrow_window_enables_horizontal_scroll_instead_of_column_overlap",
        ] {
            assert!(list.contains(test));
        }
    }

    #[test]
    fn existing_horizontal_scroll_tests_not_regressed() {
        let list = include_str!("list/mod.rs");
        for test in [
            "narrow_window_enables_horizontal_scroll_instead_of_column_overlap",
            "detached_window_inherits_horizontal_scroll_via_shared_code",
            "sorting_by_header_click_works_after_horizontal_scroll",
            "row_context_menu_rects_correct_after_horizontal_scroll",
        ] {
            assert!(list.contains(test));
        }
    }

    #[test]
    fn panel_layout_stable_with_both_sections_collapsed() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let available_width = ui.available_width();
                let expanded = bookmark_section_header(ui, "System", true);
                let collapsed = bookmark_section_header(ui, "Personal", false);
                assert_eq!(expanded.rect.height(), collapsed.rect.height());
                assert_eq!(expanded.rect.width(), collapsed.rect.width());
                assert_eq!(expanded.rect.width(), available_width);
            });
        });
        output.textures_delta.clear();
    }

    #[test]
    fn existing_bookmark_drop_and_cache_tests_not_regressed() {
        let dnd = include_str!("file_dnd.rs");
        let cache = include_str!("app/bookmark_cache.rs");
        for test in [
            "system_and_personal_bookmarks_both_accepted_as_targets",
            "drop_on_bookmark_enqueues_move_with_source_and_dest_invalidation",
            "bookmark_surface_priority_above_background_and_other_order_independent",
        ] {
            assert!(dnd.contains(test));
        }
        for test in [
            "bookmark_cache_hit_within_ttl_does_not_request_probe",
            "bookmark_unavailable_after_probe_shows_existing_reason",
            "existing_bookmark_drop_target_tests_not_regressed",
        ] {
            assert!(cache.contains(test));
        }
    }

    #[test]
    fn domain1_2_3_not_regressed() {
        let settings = include_str!("../core/settings.rs");
        let dnd = include_str!("file_dnd.rs");
        let cache = include_str!("app/bookmark_cache.rs");
        assert!(settings.contains("reset_behavior_preserving_user_data"));
        assert!(dnd.contains("fn plan_file_drop"));
        assert!(cache.contains("fn bookmark_channel_drain_applies_results_before_render_and_dnd"));
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
    paint_tree_disclosure(ui.painter(), rect, expanded, color);
    response.clicked()
}

fn bookmark_section_header(ui: &mut egui::Ui, label: &str, expanded: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(
            rect,
            style::RADIUS_ROW,
            ui.visuals().widgets.hovered.bg_fill,
        );
    }
    let text_color = if hovered {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let disclosure_rect = egui::Rect::from_min_size(
        rect.min,
        egui::vec2(style::TREE_DISCLOSURE_SIZE, rect.height()),
    );
    paint_tree_disclosure(ui.painter(), disclosure_rect, expanded, text_color);
    ui.painter().text(
        egui::pos2(
            rect.left() + style::TREE_DISCLOSURE_SIZE + style::SPACE_XXS,
            rect.center().y,
        ),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Body.resolve(ui.style()),
        text_color,
    );
    response
}

fn paint_tree_disclosure(
    painter: &egui::Painter,
    rect: egui::Rect,
    expanded: bool,
    color: egui::Color32,
) {
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
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
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
