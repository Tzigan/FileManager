use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::core::format::format_size;
use crate::core::fs::{self, FileEntry, ReadDirError};
use crate::core::i18n::{I18n, Language};
use crate::core::paths::AppDirs;
use crate::core::settings::{SettingsStore, Theme};
use crate::platform::format_local_time;

const ROW_HEIGHT: f32 = 24.0;
const ICON_WIDTH: f32 = 26.0;
const SIZE_COL: f32 = 90.0;
const TYPE_COL: f32 = 110.0;
const MODIFIED_COL: f32 = 140.0;
const COL_GAP: f32 = 8.0;

struct DirMessage {
    generation: u64,
    result: Result<Vec<FileEntry>, ReadDirError>,
}

#[derive(PartialEq)]
enum ListStatus {
    Ready,
    Loading,
}

pub struct FileManagerApp {
    store: SettingsStore,
    i18n: I18n,
    dirs: AppDirs,
    show_about: bool,

    current_path: PathBuf,
    address_input: String,
    entries: Vec<FileEntry>,
    selected: Option<PathBuf>,
    status: ListStatus,
    generation: u64,
    cancel: Arc<AtomicBool>,
    tx: Sender<DirMessage>,
    rx: Receiver<DirMessage>,
    error: Option<String>,
}

impl FileManagerApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        store: SettingsStore,
        i18n: I18n,
        dirs: AppDirs,
    ) -> Self {
        let (tx, rx) = channel();
        let start = store
            .settings
            .last_folder
            .clone()
            .filter(|p| p.is_dir())
            .or_else(|| directories::UserDirs::new().map(|u| u.home_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        let mut app = Self {
            store,
            i18n,
            dirs,
            show_about: false,
            current_path: start.clone(),
            address_input: start.display().to_string(),
            entries: Vec::new(),
            selected: None,
            status: ListStatus::Loading,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            error: None,
        };
        app.navigate(app.current_path.clone(), &cc.egui_ctx);
        app
    }

    fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.i18n.t(key)
    }

    fn set_theme(&mut self, theme: Theme) {
        self.store.settings.theme = theme;
        self.persist();
    }

    fn set_language(&mut self, lang: Language) {
        self.store.settings.language = lang;
        self.i18n.set_language(lang);
        self.persist();
    }

    fn persist(&self) {
        if let Err(e) = self.store.save() {
            tracing::error!("failed to save settings: {e}");
        }
    }

    // ------------------------------------------------------------------
    // Navigation
    // ------------------------------------------------------------------

    fn navigate(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.generation += 1;
        let generation = self.generation;
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();

        self.status = ListStatus::Loading;
        self.error = None;
        self.selected = None;
        self.current_path = path.clone();
        self.address_input = path.display().to_string();

        if self.store.settings.last_folder.as_deref() != Some(path.as_path()) {
            self.store.settings.last_folder = Some(path.clone());
            self.persist();
        }

        let tx = self.tx.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let result = fs::read_dir(&path, &cancel);
            let _ = tx.send(DirMessage { generation, result });
            ctx2.request_repaint();
        });
    }

    fn go_up(&mut self, ctx: &egui::Context) {
        if let Some(parent) = self.current_path.parent().map(|p| p.to_path_buf()) {
            self.navigate(parent, ctx);
        }
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.navigate(self.current_path.clone(), ctx);
    }

    fn cancel_loading(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.status = ListStatus::Ready;
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            if msg.generation != self.generation {
                continue;
            }
            match msg.result {
                Ok(entries) => {
                    self.entries = entries;
                    self.status = ListStatus::Ready;
                }
                Err(ReadDirError::Cancelled) => {}
                Err(ReadDirError::Io(e)) => {
                    tracing::warn!(path = %self.current_path.display(), "read_dir failed: {e}");
                    self.entries.clear();
                    self.status = ListStatus::Ready;
                    self.error = Some(self.error_text(&e));
                }
            }
        }
    }

    fn error_text(&self, e: &std::io::Error) -> String {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => self.t("error.access_denied").to_owned(),
            std::io::ErrorKind::NotFound => self.t("error.not_found").to_owned(),
            _ => format!("{}: {}", self.t("error.open_dir"), e),
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Backspace)) {
            self.go_up(ctx);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            let target = self
                .selected
                .as_ref()
                .and_then(|sel| self.entries.iter().find(|e| &e.path == sel))
                .filter(|e| e.is_dir)
                .map(|e| e.path.clone());
            if let Some(dir) = target {
                self.navigate(dir, ctx);
            }
        }
    }

    // ------------------------------------------------------------------
    // UI parts
    // ------------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(self.t("menu.file").to_owned(), |ui| {
                if ui.button(self.t("file.quit").to_owned()).clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            ui.menu_button(self.t("menu.view").to_owned(), |ui| {
                ui.menu_button(self.t("view.theme").to_owned(), |ui| {
                    for (theme, key) in [
                        (Theme::System, "theme.system"),
                        (Theme::Light, "theme.light"),
                        (Theme::Dark, "theme.dark"),
                    ] {
                        let selected = self.store.settings.theme == theme;
                        if ui.selectable_label(selected, self.t(key).to_owned()).clicked() {
                            self.set_theme(theme);
                            ui.close();
                        }
                    }
                });
                ui.menu_button(self.t("view.language").to_owned(), |ui| {
                    for lang in [Language::Ru, Language::En] {
                        let selected = self.i18n.language() == lang;
                        if ui.selectable_label(selected, lang.label()).clicked() {
                            self.set_language(lang);
                            ui.close();
                        }
                    }
                });
            });

            ui.menu_button(self.t("menu.help").to_owned(), |ui| {
                if ui.button(self.t("help.about").to_owned()).clicked() {
                    self.show_about = true;
                    ui.close();
                }
            });
        });
    }

    fn toolbar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let has_parent = self.current_path.parent().is_some();
            if ui
                .add_enabled(has_parent, egui::Button::new(self.t("toolbar.up").to_owned()))
                .clicked()
            {
                self.go_up(ctx);
            }
            if ui.button(self.t("toolbar.refresh").to_owned()).clicked() {
                self.refresh(ctx);
            }
            if self.status == ListStatus::Loading {
                ui.spinner();
                if ui.button(self.t("toolbar.cancel").to_owned()).clicked() {
                    self.cancel_loading();
                }
            }

            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.address_input)
                    .desired_width(f32::INFINITY),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let path = PathBuf::from(self.address_input.trim());
                self.navigate(path, ctx);
            }
        });
    }

    fn file_list(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        self.list_header(ui);
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, self.entries.len(), |ui, range| {
                for index in range {
                    self.file_row(ui, index, ctx);
                }
            });
    }

    fn column_x(&self, width: f32) -> (f32, f32, f32, f32) {
        let mod_x = width - MODIFIED_COL;
        let type_x = mod_x - TYPE_COL;
        let size_x = type_x - SIZE_COL;
        let name_x = ICON_WIDTH;
        (name_x, size_x, type_x, mod_x)
    }

    fn list_header(&self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::hover());
        let (name_x, size_x, type_x, mod_x) = self.column_x(width);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let color = ui.visuals().strong_text_color();
        let painter = ui.painter();
        let y = rect.center().y;

        for (x, key) in [
            (name_x, "col.name"),
            (size_x, "col.size"),
            (type_x, "col.type"),
            (mod_x, "col.modified"),
        ] {
            painter.text(
                egui::pos2(rect.min.x + x, y),
                egui::Align2::LEFT_CENTER,
                self.t(key).to_owned(),
                font.clone(),
                color,
            );
        }
    }

    fn file_row(&mut self, ui: &mut egui::Ui, index: usize, ctx: &egui::Context) {
        let entry = self.entries[index].clone();
        let width = ui.available_width();
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());

        let selected = self.selected.as_ref() == Some(&entry.path);
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
        } else if resp.hovered() {
            painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.bg_fill);
        }

        paint_icon(painter, rect, entry.is_dir, entry.is_symlink);

        let font = egui::TextStyle::Body.resolve(ui.style());
        let visuals = ui.visuals();
        let name_color = if entry.hidden {
            visuals.weak_text_color()
        } else {
            visuals.text_color()
        };
        let weak = visuals.weak_text_color();
        let y = rect.center().y;
        let (name_x, size_x, type_x, mod_x) = self.column_x(width);

        let name_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + name_x, rect.min.y),
            egui::pos2(rect.min.x + size_x - COL_GAP, rect.max.y),
        );
        painter.with_clip_rect(name_rect).text(
            egui::pos2(rect.min.x + name_x, y),
            egui::Align2::LEFT_CENTER,
            &entry.name,
            font.clone(),
            name_color,
        );

        let size_text = if entry.is_dir {
            String::new()
        } else {
            entry.size.map(format_size).unwrap_or_default()
        };
        painter.text(
            egui::pos2(rect.min.x + type_x - COL_GAP, y),
            egui::Align2::RIGHT_CENTER,
            size_text,
            font.clone(),
            weak,
        );

        let type_text = if entry.is_dir {
            self.t("type.folder").to_owned()
        } else {
            match &entry.extension {
                Some(ext) => format!("{} {}", ext.to_uppercase(), self.t("type.file")),
                None => self.t("type.file").to_owned(),
            }
        };
        let type_rect = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + type_x, rect.min.y),
            egui::pos2(rect.min.x + mod_x - COL_GAP, rect.max.y),
        );
        painter.with_clip_rect(type_rect).text(
            egui::pos2(rect.min.x + type_x, y),
            egui::Align2::LEFT_CENTER,
            type_text,
            font.clone(),
            weak,
        );

        let modified = entry.modified.map(format_local_time).unwrap_or_default();
        painter.text(
            egui::pos2(rect.min.x + mod_x, y),
            egui::Align2::LEFT_CENTER,
            modified,
            font,
            weak,
        );

        if resp.clicked() {
            self.selected = Some(entry.path.clone());
        }
        if resp.double_clicked() && entry.is_dir {
            self.navigate(entry.path, ctx);
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let title = self.t("about.title").to_owned();
        let config_label = self.t("label.config_dir").to_owned();
        let log_label = self.t("label.log_dir").to_owned();
        let mut open = self.show_about;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.heading(format!("FileManager v{}", env!("CARGO_PKG_VERSION")));
                ui.label("Rust + egui/eframe");
                ui.add_space(8.0);
                ui.label(format!("{}: {}", config_label, self.dirs.config_dir.display()));
                ui.label(format!("{}: {}", log_label, self.dirs.log_dir.display()));
            });
        self.show_about = open;
    }
}

fn paint_icon(painter: &egui::Painter, row: egui::Rect, is_dir: bool, _is_symlink: bool) {
    let icon = egui::Rect::from_center_size(
        row.left_center() + egui::vec2(ICON_WIDTH / 2.0, 0.0),
        egui::vec2(17.0, 15.0),
    );
    if is_dir {
        let back = egui::Color32::from_rgb(226, 152, 52);
        let front = egui::Color32::from_rgb(255, 199, 90);
        let tab = egui::Rect::from_min_max(icon.min, egui::pos2(icon.min.x + 7.0, icon.min.y + 4.0));
        let body = egui::Rect::from_min_max(egui::pos2(icon.min.x, icon.min.y + 4.0), icon.max);
        painter.rect_filled(tab, 1.5, back);
        painter.rect_filled(body, 1.5, front);
    } else {
        let doc = icon.shrink(1.0);
        painter.rect_filled(doc, 1.0, egui::Color32::from_rgb(190, 195, 205));
        painter.rect_stroke(
            doc,
            1.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(130, 135, 150)),
            egui::StrokeKind::Inside,
        );
    }
}

impl eframe::App for FileManagerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.set_theme(match self.store.settings.theme {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        });

        self.drain_messages();
        self.handle_keys(&ctx);

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(&ctx, ui));

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.status == ListStatus::Loading {
                    ui.label(self.t("status.loading").to_owned());
                } else {
                    ui.label(format!("{}: {}", self.t("status.items"), self.entries.len()));
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(err) = self.error.clone() {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                    ui.add_space(12.0);
                    if ui.button(self.t("toolbar.up").to_owned()).clicked() {
                        self.go_up(&ctx);
                    }
                });
                return;
            }
            if self.entries.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    if self.status == ListStatus::Loading {
                        ui.spinner();
                        ui.label(self.t("status.loading").to_owned());
                    } else {
                        ui.label(self.t("folder.empty").to_owned());
                    }
                });
                return;
            }
            self.file_list(&ctx, ui);
        });

        self.about_window(&ctx);
    }
}
