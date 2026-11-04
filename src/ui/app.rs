use crate::core::i18n::{I18n, Language};
use crate::core::paths::AppDirs;
use crate::core::settings::{SettingsStore, Theme};

pub struct FileManagerApp {
    store: SettingsStore,
    i18n: I18n,
    dirs: AppDirs,
    show_about: bool,
}

impl FileManagerApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        store: SettingsStore,
        i18n: I18n,
        dirs: AppDirs,
    ) -> Self {
        Self {
            store,
            i18n,
            dirs,
            show_about: false,
        }
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

    fn menu_bar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
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

        _ = ctx;
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let title = self.t("about.title").to_owned();
        let body = self.t("welcome.body").to_owned();
        let mut open = self.show_about;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.heading(format!("FileManager v{}", env!("CARGO_PKG_VERSION")));
                ui.label("Rust + egui/eframe");
                ui.label(body);
            });
        self.show_about = open;
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

        egui::Panel::top("menu_bar").show(ui, |ui| self.menu_bar(&ctx, ui));

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.label(self.t("status.ready").to_owned());
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.heading(self.t("welcome.title").to_owned());
                ui.add_space(8.0);
                ui.label(self.t("welcome.body").to_owned());
                ui.add_space(24.0);
                ui.label(format!(
                    "{}: {}",
                    self.t("label.config_dir"),
                    self.dirs.config_dir.display()
                ));
                ui.label(format!(
                    "{}: {}",
                    self.t("label.log_dir"),
                    self.dirs.log_dir.display()
                ));
            });
        });

        self.about_window(&ctx);
    }
}
