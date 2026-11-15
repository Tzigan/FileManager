//! Диалоги настроек, свойств и обновления.

use anyhow::Context;

use super::*;

pub(super) fn log_export_error_key(
    error: &crate::core::log_export::LogExportError,
) -> &'static str {
    match error {
        crate::core::log_export::LogExportError::NoLogFiles => "logs.export_no_logs",
        crate::core::log_export::LogExportError::Io { source, .. } => {
            crate::ui::opsui::operation_error_key(source.kind())
        }
    }
}

/// Единственное поле, которое читается до создания интерфейса, требует перезапуска.
fn settings_reset_requires_restart(previous_single_instance: bool) -> bool {
    previous_single_instance != crate::core::settings::Settings::default().single_instance
}

fn settings_reset_load_warning(
    issue: Option<crate::core::settings::SettingsLoadIssue>,
) -> Option<&'static str> {
    issue.map(crate::core::settings::SettingsLoadIssue::message_key)
}

/// Категории окна настроек. Подписи берутся из локализации, не из текста виджета.
/// Новая категория добавляется сюда и в `settings_window`, без новой панели.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum SettingsCategory {
    #[default]
    General,
    Files,
    Tree,
    Navigation,
    Trash,
    Updates,
}

impl SettingsCategory {
    pub(super) const ALL: [Self; 6] = [
        Self::General,
        Self::Files,
        Self::Tree,
        Self::Navigation,
        Self::Trash,
        Self::Updates,
    ];

    fn label_key(self) -> &'static str {
        match self {
            Self::General => "settings.category.general",
            Self::Files => "settings.category.files",
            Self::Tree => "settings.category.tree",
            Self::Navigation => "settings.category.navigation",
            Self::Trash => "settings.category.trash",
            Self::Updates => "settings.category.updates",
        }
    }
}

/// Ширина левой колонки по самим подписям. Фиксированные пиксели обрезали бы
/// текст при другом масштабе и при длинном переводе.
fn settings_category_column_width(ui: &egui::Ui, labels: &[String]) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_width = labels.iter().fold(0.0_f32, |widest, label| {
        let width = ui.ctx().fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(label.clone(), font.clone(), egui::Color32::PLACEHOLDER)
                .size()
                .x
        });
        widest.max(width)
    });
    // У selectable_label есть внутренние поля, без них длинная подпись
    // переносится на вторую строку.
    text_width + ui.spacing().button_padding.x * 2.0 + style::SPACE_MD
}

impl FileManagerApp {
    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let title = self.t("about.title").to_owned();
        let config_label = self.t("label.config_dir").to_owned();
        let log_label = self.t("label.log_dir").to_owned();
        let export_label = self.t("logs.export").to_owned();
        let export_hint = self.t("logs.export_hint").to_owned();
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-about");
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(520.0, 350.0),
            self.about_viewport_placed,
        );
        let mut close_requested = false;
        let mut export_requested = false;
        ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, _class| {
            let viewport_ctx = viewport_ui.ctx().clone();
            if viewport_ctx.input(|i| i.viewport().close_requested()) {
                viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                close_requested = true;
                return;
            }
            egui::CentralPanel::default().show(viewport_ui, |ui| {
                ui.heading(format!("FileManager v{}", env!("CARGO_PKG_VERSION")));
                ui.label("Rust + egui/eframe");
                ui.add_space(style::SPACE_SM);
                ui.label(format!(
                    "{}: {}",
                    config_label,
                    self.dirs.config_dir.display()
                ));
                ui.horizontal(|ui| {
                    ui.label(format!("{}: {}", log_label, self.dirs.log_dir.display()));
                    if ui
                        .add_enabled(
                            !self.log_export_busy,
                            egui::Button::new(export_label.clone()),
                        )
                        .clicked()
                    {
                        export_requested = true;
                    }
                });
                ui.add(egui::Label::new(&export_hint).wrap());
                if let Some(status) = &self.log_export_status {
                    ui.label(status);
                }
                ui.add_space(style::SPACE_SM);
                let can_check = !self.update_busy
                    && self.pending_update.is_none()
                    && self.update_ready.is_none();
                if ui
                    .add_enabled(
                        can_check,
                        egui::Button::new(self.t("update.check").to_owned()),
                    )
                    .clicked()
                {
                    self.update_status = None;
                    self.update_err = None;
                    self.spawn_update_check(&viewport_ctx);
                }
                if let Some(manifest) = self.pending_update.clone() {
                    ui.add_space(style::SPACE_SM);
                    ui.label(self.t("update.found").replace("{v}", &manifest.version));
                    if self.update_ready.is_some() {
                        ui.horizontal(|ui| {
                            ui.label(self.t("update.ready").to_owned());
                            if ui.button(self.t("update.install").to_owned()).clicked() {
                                self.request_update_install(&viewport_ctx);
                            }
                            if self.update_restart_offer
                                && ui.button(self.t("update.restart_now").to_owned()).clicked()
                            {
                                self.arm_update_restart(&viewport_ctx);
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !self.update_busy,
                                    egui::Button::new(self.t("update.now").to_owned()),
                                )
                                .clicked()
                            {
                                self.start_update_download(&viewport_ctx);
                            }
                            if !self.update_busy
                                && ui.button(self.t("update.later").to_owned()).clicked()
                            {
                                self.pending_update = None;
                                self.update_downloaded = 0;
                                self.update_total = 0;
                                self.update_status = None;
                                self.update_err = None;
                            }
                        });
                    }
                }
                if let Some(status) = &self.update_status {
                    ui.label(status);
                }
                if self.update_busy
                    || (self.update_err.is_some() && self.update_total > 0)
                    || self.update_ready.is_some()
                {
                    update_progress_row(ui, self.update_downloaded, self.update_total);
                }
                if let Some(error) = &self.update_err {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        });
        self.about_viewport_placed = !close_requested;
        if close_requested {
            self.show_about = false;
        }
        if export_requested {
            self.export_logs_from_about(ctx);
            ctx.request_repaint();
        }
    }

    fn export_logs_from_about(&mut self, ctx: &egui::Context) {
        let picker_title = self.t("logs.export_folder").to_owned();
        let destination = match crate::platform::pick_folder(
            &picker_title,
            Some(&self.dirs.log_dir),
        ) {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => {
                self.log_export_status = Some(format!(
                    "{}: {}",
                    self.t("logs.export_failed"),
                    self.operation_error_message(error.kind())
                ));
                self.record_io_error(None, &error);
                return;
            }
        };

        let labels = crate::core::log_export::ExportLabels {
            title: self.t("logs.export_header").to_owned(),
            version: self.t("logs.export_version").to_owned(),
            exported_at: self.t("logs.export_time").to_owned(),
            scope: self
                .t("logs.export_scope")
                .replace("{days}", &crate::core::log_export::LOG_EXPORT_WINDOW_FILES.to_string()),
            journal_note: self.t("logs.export_journal_excluded").to_owned(),
            freshness_note: String::new(),
            skipped_logs_note: self.t("logs.export_skipped_header").to_owned(),
        };
        let freshness_confirmed_note = self.t("logs.export_freshness_confirmed").to_owned();
        let freshness_pending_note = self.t("logs.export_pending_note").to_owned();
        let exported_at = crate::platform::format_local_time(
            std::time::SystemTime::now(),
            self.i18n.language(),
        );
        let log_dir = self.dirs.log_dir.clone();
        let msg_tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        self.log_export_busy = true;
        self.log_export_status = Some(self.t("logs.exporting").to_owned());
        thread::spawn(move || {
            let freshness_confirmed = crate::core::log_export::wait_for_log_flush(
                &log_dir,
                crate::core::log_export::LOG_EXPORT_FLUSH_TIMEOUT,
            );
            let mut labels = labels;
            labels.freshness_note = if freshness_confirmed {
                freshness_confirmed_note
            } else {
                freshness_pending_note
            };
            let result = crate::core::log_export::export_logs(
                &log_dir,
                &destination,
                env!("CARGO_PKG_VERSION"),
                &exported_at,
                &labels,
            )
            .map(|exported| (exported, freshness_confirmed));
            let _ = msg_tx.send(WorkerMsg::LogExportFinished { result });
            ctx.request_repaint();
        });
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let title = self.t("menu.settings").to_owned();
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-settings");
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(640.0, 420.0),
            self.settings_viewport_placed,
        );
        let mut close_requested = false;
        let mut hidden_files_changed = false;
        let mut wsl_visibility_changed = false;
        let mut reset_choice = None;
        // Раньше слева была одна подпись «Общие», а справа лежали все параметры сразу.
        // Список категорий один: новая секция добавляется в SettingsCategory::ALL,
        // а не отдельной вёрсткой. Значения живут в Settings и пишутся сразу,
        // поэтому смена категории ничего не сбрасывает.
        let category_labels: Vec<String> = SettingsCategory::ALL
            .iter()
            .map(|category| self.t(category.label_key()).to_owned())
            .collect();
        let mut selected = self.settings_category;
        ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, _class| {
            let viewport_ctx = viewport_ui.ctx().clone();
            if viewport_ctx.input(|i| i.viewport().close_requested()) {
                viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                close_requested = true;
                return;
            }
            egui::CentralPanel::default().show(viewport_ui, |ui| {
                let content_height = ui.available_height();
                // horizontal() выравнивает колонки по центру, поэтому короткая
                // страница оказывалась посередине высокой панели. Здесь обе
                // колонки начинаются сверху.
                ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                    let sidebar_width = settings_category_column_width(ui, &category_labels);
                    ui.allocate_ui_with_layout(
                        egui::vec2(sidebar_width, content_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("settings-categories")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.add_space(style::SPACE_SM);
                                    for (category, label) in
                                        SettingsCategory::ALL.iter().zip(category_labels.iter())
                                    {
                                        if ui
                                            .selectable_label(selected == *category, label)
                                            .clicked()
                                        {
                                            selected = *category;
                                        }
                                    }
                                    ui.add_space(style::SPACE_MD);
                                });
                        },
                    );
                    let (divider, _) = ui
                        .allocate_exact_size(egui::vec2(1.0, content_height), egui::Sense::hover());
                    ui.painter().vline(
                        divider.center().x,
                        divider.y_range(),
                        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    );
                    ui.add_space(style::SPACE_SM);
                    let page_width = ui.available_width();
                    ui.allocate_ui_with_layout(
                        egui::vec2(page_width, content_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("settings-page")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(page_width);
                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                                    ui.add_space(style::SPACE_SM);
                                    match selected {
                                        SettingsCategory::General => self.settings_general(ui),
                                        SettingsCategory::Files => {
                                            self.settings_files(ui, &mut hidden_files_changed);
                                        }
                                        SettingsCategory::Tree => {
                                            self.settings_tree(ui, &mut wsl_visibility_changed);
                                        }
                                        SettingsCategory::Navigation => {
                                            self.settings_navigation(ui)
                                        }
                                        SettingsCategory::Trash => self.delete_mode_settings(ui),
                                        SettingsCategory::Updates => self.settings_updates(ui),
                                    }
                                    // Снизу тот же отступ, что и в начале страницы, чтобы поле не липло к рамке.
                                    ui.add_space(style::SPACE_MD);
                                });
                        },
                    );
                });
            });
            if self.settings_reset_confirm {
                let mut open = true;
                egui::Window::new(self.t("settings.reset_confirm_title").to_owned())
                    .open(&mut open)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(&viewport_ctx, |ui| {
                        if let Some(issue_key) = settings_reset_load_warning(self.store.load_issue) {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                self.t(issue_key),
                            );
                            ui.add_space(style::SPACE_SM);
                            ui.label(self.t("settings.reset_confirm_load_issue"));
                        } else {
                            ui.label(self.t("settings.reset_confirm_scope"));
                        }
                        ui.horizontal(|ui| {
                            if ui.button(self.t("settings.reset_confirm_action")).clicked() {
                                reset_choice = Some(true);
                            }
                            if ui.button(self.t("dlg.cancel")).clicked() {
                                reset_choice = Some(false);
                            }
                        });
                    });
                if !open && reset_choice.is_none() {
                    reset_choice = Some(false);
                }
            }
        });
        self.settings_category = selected;
        if hidden_files_changed {
            self.persist();
            for tab in 0..self.tabs.len() {
                self.navigate_fresh(tab, ctx);
            }
        }
        if wsl_visibility_changed {
            self.persist();
            if self.store.settings.show_wsl_in_tree {
                self.refresh_wsl_distributions();
            }
        }
        if let Some(confirmed) = reset_choice {
            self.settings_reset_confirm = false;
            if confirmed {
                self.apply_settings_reset(ctx);
            }
        }
        self.settings_viewport_placed = !close_requested;
        if close_requested {
            self.show_settings = false;
            self.settings_reset_confirm = false;
        }
    }

    fn apply_settings_reset(&mut self, ctx: &egui::Context) {
        let previous = self.store.settings.clone();
        let load_issue = self.store.load_issue;
        let settings_file_exists = self.dirs.config_dir.join("settings.json").is_file();
        let defaults = crate::core::settings::Settings::default();
        let hidden_files_changed =
            previous.show_hidden_files != defaults.show_hidden_files;
        let wsl_visibility_changed =
            previous.show_wsl_in_tree != defaults.show_wsl_in_tree;
        let restart_required = settings_reset_requires_restart(previous.single_instance);

        self.store.settings.reset_behavior_preserving_user_data();
        self.i18n.set_language(self.store.settings.language);
        crate::core::logging::set_panic_language(self.store.settings.language);
        self.style_configured = false;
        self.bg_ops_pool
            .set_limit(self.store.settings.max_concurrent_bg_ops);
        self.dir_cache
            .set_limit(self.store.settings.dir_cache_entries);

        if hidden_files_changed {
            for index in 0..self.tabs.len() {
                self.navigate_fresh(index, ctx);
            }
        }
        if wsl_visibility_changed && self.store.settings.show_wsl_in_tree {
            self.refresh_wsl_distributions();
        }
        self.sync_dir_watch(ctx);

        if self.persist_checked().is_ok() {
            self.settings_reset_status = Some(self.t(if restart_required {
                "settings.reset_done_restart"
            } else {
                "settings.reset_done"
            }).to_owned());
        } else {
            let message_key = if load_issue.is_some() {
                "settings.reset_session_only_load_issue"
            } else if settings_file_exists {
                "settings.reset_session_only"
            } else {
                "settings.reset_session_only_no_file"
            };
            let mut message = self.t(message_key).to_owned();
            if restart_required {
                message.push('\n');
                message.push_str(self.t("settings.reset_restart_note"));
            }
            self.settings_reset_status = Some(message);
        }
        ctx.request_repaint();
    }

    fn settings_general(&mut self, ui: &mut egui::Ui) {
        let single_instance_label = self.t("settings.single_instance").to_owned();
        if ui
            .checkbox(
                &mut self.store.settings.single_instance,
                single_instance_label,
            )
            .changed()
        {
            self.persist();
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.single_instance_hint")).weak());
        ui.add_space(style::SPACE_MD);
        ui.label(self.t("settings.max_concurrent_bg_ops"));
        let mut limit = crate::core::bg_ops::clamp_concurrent_bg_ops(
            self.store.settings.max_concurrent_bg_ops,
        );
        if ui
            .add(
                egui::DragValue::new(&mut limit)
                    .range(
                        crate::core::bg_ops::MIN_CONCURRENT_BG_OPS
                            ..=crate::core::bg_ops::MAX_CONCURRENT_BG_OPS,
                    ),
            )
            .changed()
        {
            self.store.settings.max_concurrent_bg_ops = limit;
            self.bg_ops_pool.set_limit(limit);
            self.persist();
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.max_concurrent_bg_ops_hint")).weak());
        ui.add_space(style::SPACE_MD);
        ui.separator();
        ui.add_space(style::SPACE_SM);
        if ui.button(self.t("settings.reset_button")).clicked() {
            self.settings_reset_status = None;
            self.settings_reset_confirm = true;
        }
        if let Some(status) = &self.settings_reset_status {
            ui.add_space(style::SPACE_SM);
            ui.label(status);
        }
    }

    fn settings_files(&mut self, ui: &mut egui::Ui, hidden_files_changed: &mut bool) {
        let show_hidden_label = self.t("settings.show_hidden_files").to_owned();
        if ui
            .checkbox(
                &mut self.store.settings.show_hidden_files,
                show_hidden_label,
            )
            .changed()
        {
            *hidden_files_changed = true;
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.show_hidden_files_hint")).weak());
    }

    fn settings_tree(&mut self, ui: &mut egui::Ui, wsl_visibility_changed: &mut bool) {
        let show_wsl_label = self.t("settings.show_wsl_in_tree").to_owned();
        if ui
            .checkbox(&mut self.store.settings.show_wsl_in_tree, show_wsl_label)
            .changed()
        {
            *wsl_visibility_changed = true;
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.show_wsl_in_tree_hint")).weak());
    }

    fn settings_navigation(&mut self, ui: &mut egui::Ui) {
        let side_navigation_label = self.t("settings.mouse_side_navigation").to_owned();
        if ui
            .checkbox(
                &mut self.store.settings.enable_mouse_side_navigation,
                side_navigation_label,
            )
            .changed()
        {
            self.persist();
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.mouse_side_navigation_hint")).weak());
    }

    fn settings_updates(&mut self, ui: &mut egui::Ui) {
        let auto_restart_label = self.t("settings.auto_restart_after_update").to_owned();
        if ui
            .checkbox(
                &mut self.store.settings.auto_restart_after_update,
                auto_restart_label,
            )
            .changed()
        {
            self.persist();
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.auto_restart_after_update_hint")).weak());
        ui.add_space(style::SPACE_MD);
        ui.label(self.t("settings.update_restart_delay"));
        let mut delay = self.store.settings.update_restart_delay_secs;
        if ui
            .add(
                egui::DragValue::new(&mut delay)
                    .range(0u32..=60)
                    .suffix(" s"),
            )
            .changed()
        {
            self.store.settings.update_restart_delay_secs = delay;
            self.persist();
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.update_restart_delay_hint")).weak());
    }

    fn delete_mode_settings(&mut self, ui: &mut egui::Ui) {
        use crate::core::settings::DeleteMode;

        ui.label(self.t("settings.delete_mode"));
        let mut mode = self.store.settings.delete_mode;
        let mut changed = false;
        changed |= ui
            .radio_value(
                &mut mode,
                DeleteMode::WindowsRecycle,
                self.t("settings.delete_mode.windows"),
            )
            .changed();
        changed |= ui
            .radio_value(
                &mut mode,
                DeleteMode::SecureTrash,
                self.t("settings.delete_mode.secure"),
            )
            .changed();
        changed |= ui
            .radio_value(
                &mut mode,
                DeleteMode::Ask,
                self.t("settings.delete_mode.ask"),
            )
            .changed();
        if changed {
            self.store.settings.delete_mode = mode;
            self.persist();
        }
        ui.add_space(style::SPACE_SM);
        ui.label(egui::RichText::new(self.t("settings.delete_mode.hint")).weak());
        ui.add_space(style::SPACE_SM);
        let mut hash_mb = (self.store.settings.secure_trash_hash_max_bytes / (1024 * 1024)) as u32;
        ui.label(self.t("settings.secure_trash_hash"));
        if ui
            .add(
                egui::DragValue::new(&mut hash_mb)
                    .range(0u32..=4096)
                    .suffix(" MB"),
            )
            .changed()
        {
            self.store.settings.secure_trash_hash_max_bytes = u64::from(hash_mb) * 1024 * 1024;
            self.persist();
        }
        ui.label(egui::RichText::new(self.t("settings.secure_trash_hash_hint")).weak());
        ui.add_space(style::SPACE_SM);
        let mut fallback = self.store.settings.secure_trash_allow_fallback;
        if ui
            .checkbox(&mut fallback, self.t("settings.secure_trash_fallback"))
            .changed()
        {
            self.store.settings.secure_trash_allow_fallback = fallback;
            self.persist();
        }
        ui.label(egui::RichText::new(self.t("settings.secure_trash_fallback_hint")).weak());
        ui.add_space(style::SPACE_SM);
        ui.label(self.t("settings.secure_trash_folders"));
        ui.label(egui::RichText::new(self.t("settings.secure_trash_folders_hint")).weak());
        let mut remove_at = None;
        for (index, folder) in self.store.settings.secure_trash_folders.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(folder.display().to_string());
                if ui.button(self.t("settings.secure_trash_remove")).clicked() {
                    remove_at = Some(index);
                }
            });
        }
        if let Some(index) = remove_at {
            self.store.settings.secure_trash_folders.remove(index);
            self.persist();
        }
        if ui
            .button(self.t("settings.secure_trash_add_folder"))
            .clicked()
            && let Ok(Some(path)) =
                crate::platform::pick_folder(&self.t("settings.secure_trash_add_folder"), None)
            && !self.store.settings.secure_trash_folders.contains(&path)
        {
            self.store.settings.secure_trash_folders.push(path);
            self.persist();
        }
        if ui.button(self.t("trash.open")).clicked() {
            self.show_secure_trash = true;
        }
    }

    fn spawn_update_check(&self, ctx: &egui::Context) {
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let msg = match crate::core::update::check_latest() {
                Ok(manifest)
                    if crate::core::update::version_newer(
                        env!("CARGO_PKG_VERSION"),
                        &manifest.version,
                    ) =>
                {
                    WorkerMsg::UpdateFound { manifest }
                }
                Ok(_) => WorkerMsg::UpdateLatest,
                Err(e) => WorkerMsg::UpdateFailed(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn start_update_download(&mut self, ctx: &egui::Context) {
        let Some(manifest) = self.pending_update.clone() else {
            return;
        };
        self.update_busy = true;
        self.update_ready = None;
        self.update_downloaded = 0;
        self.update_total = manifest.size;
        self.update_status = Some(self.t("update.downloading").to_owned());
        self.update_err = None;
        let tx = self.msg_tx.clone();
        let progress_tx = tx.clone();
        let progress_ctx = ctx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let mut last_percent = None;
            let msg = match crate::core::update::download_with_progress(
                &manifest,
                move |downloaded, total| {
                    let percent = update_percent(downloaded, total);
                    if last_percent != Some(percent) {
                        last_percent = Some(percent);
                        let _ = progress_tx.send(WorkerMsg::UpdateProgress { downloaded, total });
                        progress_ctx.request_repaint();
                    }
                },
            ) {
                Ok(archive) => {
                    let stage = crate::platform::prepare_update_archive(
                        &archive,
                        manifest.size,
                        &manifest.sha256,
                    );
                    if let Err(e) = std::fs::remove_file(&archive) {
                        tracing::warn!(path = %archive.display(), "failed to remove downloaded update archive: {e}");
                    }
                    match stage {
                        Ok(stage_dir) => WorkerMsg::UpdateReady(stage_dir),
                        Err(e) => WorkerMsg::UpdateFailed(e.to_string()),
                    }
                }
                Err(e) => WorkerMsg::UpdateFailed(e.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Установка ещё не началась: либо откладываем выход, либо спрашиваем пользователя.
    fn request_update_install(&mut self, ctx: &egui::Context) {
        match plan_update_install(
            self.store.settings.auto_restart_after_update,
            self.update_blocked_by_work(),
        ) {
            UpdateInstallAction::Blocked => {
                self.update_restart_at = None;
                self.update_restart_offer = false;
                self.update_err = Some(self.t("update.busy_ops").to_owned());
                ctx.request_repaint();
            }
            UpdateInstallAction::OfferRestart => {
                // Файлы всё равно нельзя заменить, пока этот процесс их держит.
                // Не закрываем окно, пока пользователь сам не подтвердит перезапуск.
                self.prepare_saved_state();
                self.update_restart_at = None;
                self.update_restart_offer = true;
                self.update_err = None;
                self.update_status = Some(self.t("update.restart_offer").to_owned());
                ctx.request_repaint();
            }
            UpdateInstallAction::ScheduleRestart => {
                self.arm_update_restart(ctx);
            }
        }
    }

    fn arm_update_restart(&mut self, ctx: &egui::Context) {
        if self.update_blocked_by_work() {
            self.update_restart_at = None;
            self.update_err = Some(self.t("update.busy_ops").to_owned());
            ctx.request_repaint();
            return;
        }
        self.prepare_saved_state();
        self.update_restart_offer = false;
        self.update_err = None;
        self.update_status = Some(self.t("update.restarting").to_owned());
        let delay = Duration::from_secs(self.store.settings.update_restart_delay_secs as u64);
        self.update_restart_at = Some(Instant::now() + delay);
        ctx.request_repaint();
    }

    /// Текущие вкладки и режим отображения уже умеет сохранять настройки. История и выделение живут только в памяти.
    fn prepare_saved_state(&mut self) {
        self.persist_tabs();
    }

    fn update_blocked_by_work(&self) -> bool {
        !self.ops_ui.jobs.is_empty()
            || self.ops_ui.conflict.is_some()
            || self.ops_ui.error_q.is_some()
            || self.zip_busy
            || self.zip_creating
    }

    pub(super) fn poll_update_restart(&mut self, ctx: &egui::Context) {
        let Some(at) = self.update_restart_at else {
            return;
        };
        if self.update_blocked_by_work() {
            self.update_restart_at = None;
            self.update_status = None;
            self.update_err = Some(self.t("update.busy_ops").to_owned());
            ctx.request_repaint();
            return;
        }
        let now = Instant::now();
        if now < at {
            ctx.request_repaint_after(at.saturating_duration_since(now));
            return;
        }
        self.finish_update_restart(ctx);
    }

    fn finish_update_restart(&mut self, ctx: &egui::Context) {
        self.update_restart_at = None;
        let Some(stage_dir) = self.update_ready.clone() else {
            return;
        };
        // К этому моменту пользователь либо включил автоперезапуск, либо нажал кнопку.
        let restart = true;
        let russian = self.store.settings.language == crate::core::i18n::Language::Ru;
        let launch_result = std::env::current_exe()
            .context("locate current FileManager executable")
            .and_then(|target| {
                crate::platform::launch_updater(
                    &stage_dir,
                    &target,
                    std::process::id(),
                    restart,
                    russian,
                )
            });
        match launch_result {
            Ok(()) => {
                // Раньше Close вызывался без id и попадал в окно «О программе»:
                // основной процесс оставался жив, updater ждал его pid и не доходил до запуска.
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            }
            Err(error) => {
                tracing::error!(path = %stage_dir.display(), error = %error, "failed to launch update installer");
                let _ = std::fs::remove_dir_all(&stage_dir);
                self.update_ready = None;
                self.update_status = None;
                self.update_restart_offer = false;
                self.update_err = Some(self.t("update.install_failed").to_owned());
                ctx.request_repaint();
            }
        }
    }

    /// Тяжёлые свойства читаются только для одного выбранного объекта или открытого окна.
    /// Повторный кадр с тем же путём новый поток не запускает.
    pub(super) fn sync_entry_details(&mut self, ctx: &egui::Context) {
        let subject = self.entry_details_subject();
        let step = self.details_watch.follow(subject.as_deref());
        let crate::core::entry_details::DetailsStep::Start { generation, path } = step else {
            return;
        };
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let details = crate::platform::read_entry_details(&path);
            let _ = tx.send(WorkerMsg::EntryDetails {
                generation,
                path,
                details,
            });
            ctx.request_repaint();
        });
    }

    fn entry_details_subject(&self) -> Option<PathBuf> {
        if self.props_multi_target.is_some() {
            return None;
        }
        let tab = self.tabs.get(self.active);
        let selected = tab.and_then(selected_entry_for_details);
        crate::core::entry_details::heavy_details_path(
            self.props_target.as_deref(),
            tab.is_some_and(|tab| tab.archive_prefix.is_some()),
            tab.is_some_and(|tab| tab.secure_trash),
            selected.map(|entry| entry.path.as_path()),
            selected.is_some_and(|entry| entry.archive_member.is_some()),
        )
        .map(Path::to_path_buf)
    }

    pub(super) fn act_props(&mut self) {
        let tab = &self.tabs[self.active];
        if tab.archive_prefix.is_some() {
            return;
        }
        let selected: Vec<_> = tab
            .selection
            .iter()
            .filter_map(|key| tab.entries.iter().find(|entry| key.matches(entry)).cloned())
            .collect();
        if tab.selection.len() > 1 && selected.len() > 1 {
            self.props_target = None;
            self.props_multi_target = Some(selected);
        } else {
            let selected = if tab.selection.len() == 1 {
                selected.into_iter().next()
            } else {
                tab.entries.get(tab.cursor).cloned()
            };
            if let Some(entry) = selected {
                self.props_target = Some(entry.path.clone());
                self.props_multi_target = None;
            }
        }
    }

    pub(super) fn props_window(&mut self, ctx: &egui::Context) {
        let target = self.props_target.clone();
        let multi_entries = self.props_multi_target.clone();
        if target.is_none() && multi_entries.is_none() {
            return;
        }
        let title = self.t("props.title").to_owned();
        let entry = target.as_ref().and_then(|target| {
            self.tabs[self.active]
                .entries
                .iter()
                .find(|entry| entry.path == *target)
                .cloned()
        });
        let allow_details = !self.tabs[self.active].secure_trash
            && self.tabs[self.active].archive_prefix.is_none()
            && entry
                .as_ref()
                .is_none_or(|entry| entry.archive_member.is_none());
        let rows = if let Some(entries) = &multi_entries {
            let entries: Vec<_> = entries.iter().collect();
            self.multi_property_lines(&entries, entries.len())
        } else if let Some(target) = &target {
            entry.as_ref().map_or_else(
                || self.missing_property_lines(target, allow_details),
                |entry| self.single_property_lines(entry, allow_details),
            )
        } else {
            Vec::new()
        };
        let mut dirty = false;
        let mut apply = None;
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-properties");
        // TODO: системное окно свойств Windows требует отдельной безопасной shell-интеграции.
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(
                crate::ui::PROPERTIES_WINDOW_DEFAULT_WIDTH,
                crate::ui::PROPERTIES_WINDOW_DEFAULT_HEIGHT,
            ),
            self.props_viewport_placed,
        );
        let mut close_requested = false;
        ctx.show_viewport_immediate(viewport_id, builder, |viewport_ui, _class| {
            let viewport_ctx = viewport_ui.ctx().clone();
            if viewport_ctx.input(|i| i.viewport().close_requested()) {
                viewport_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                close_requested = true;
                return;
            }
            egui::CentralPanel::default().show(viewport_ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.render_property_lines(ui, &rows);
                    if let Some(e) = &entry {
                        ui.separator();
                        let mut ro = e.readonly;
                        let mut hi = e.hidden;
                        if ui
                            .checkbox(&mut ro, self.t("props.readonly").to_owned())
                            .changed()
                        {
                            dirty = true;
                        }
                        if ui
                            .checkbox(&mut hi, self.t("props.hidden").to_owned())
                            .changed()
                        {
                            dirty = true;
                        }
                        if dirty {
                            apply = Some((ro, hi));
                        }
                    }
                });
            });
        });
        self.props_viewport_placed = !close_requested;
        if let (Some((ro, hi)), Some(target)) = (apply, target.as_ref()) {
            let result = crate::platform::set_attributes(target, ro, hi);
            if let Err(error) = &result {
                self.record_io_error(Some(target), error);
            }
            refresh_after_attribute_change(
                result,
                || self.refresh(ctx),
            );
        }
        if close_requested {
            self.props_target = None;
            self.props_multi_target = None;
        }
    }
}

fn selected_entry_for_details(tab: &Tab) -> Option<&crate::core::fs::FileEntry> {
    if tab.selection.len() != 1 {
        return None;
    }
    let key = tab.selection.iter().next()?;
    tab.entries.iter().find(|entry| key.matches(entry))
}

/// После успешной смены флага перечитываем список: hidden меняет состав видимых строк.
fn refresh_after_attribute_change(result: std::io::Result<()>, refresh: impl FnOnce()) {
    if result.is_ok() {
        refresh();
    }
}

/// Решение до запуска updater. Сам запуск нового exe делает updater после замены файлов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpdateInstallAction {
    Blocked,
    OfferRestart,
    ScheduleRestart,
}

pub(super) fn plan_update_install(
    auto_restart: bool,
    blocked_by_work: bool,
) -> UpdateInstallAction {
    if blocked_by_work {
        UpdateInstallAction::Blocked
    } else if auto_restart {
        UpdateInstallAction::ScheduleRestart
    } else {
        UpdateInstallAction::OfferRestart
    }
}

pub(super) fn update_percent(downloaded: u64, total: u64) -> u32 {
    if total == 0 {
        0
    } else {
        (downloaded.saturating_mul(100) / total).min(100) as u32
    }
}

fn update_progress_row(ui: &mut egui::Ui, downloaded: u64, total: u64) {
    let progress = if total == 0 {
        0.0
    } else {
        (downloaded as f32 / total as f32).clamp(0.0, 1.0)
    };
    let progress =
        ui.ctx()
            .animate_value_with_time(egui::Id::unique("about-update-progress"), progress, 0.15);
    let percent = update_percent(downloaded, total);
    let percent_width = 42.0;
    let gap = style::SPACE_SM;
    let bar_width = (ui.available_width() - percent_width - gap).max(40.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_width, 8.0), egui::Sense::hover());
        let visuals = ui.visuals();
        let radius = style::RADIUS_CONTROL;
        ui.painter()
            .rect_filled(rect, radius, visuals.widgets.inactive.bg_fill);
        let filled =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * progress, rect.height()));
        if progress > 0.0 {
            ui.painter()
                .rect_filled(filled, radius, visuals.selection.bg_fill);
        }
        ui.add_space(gap);
        ui.add_sized(
            egui::vec2(percent_width, 18.0),
            egui::Label::new(format!("{percent}%")),
        );
    });
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        SettingsCategory, log_export_error_key, refresh_after_attribute_change,
        selected_entry_for_details, settings_category_column_width,
        settings_reset_load_warning, settings_reset_requires_restart,
    };
    use crate::core::dir_cache::DirCache;
    use crate::core::fs::FileEntry;
    use crate::ui::tabs::{SelectionKey, Tab};

    fn entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.to_owned(),
            path: PathBuf::from(r"C:\cache-edit").join(name),
            is_dir: false,
            is_symlink: false,
            hidden: false,
            system: false,
            readonly: false,
            size: Some(1),
            modified: None,
            created: None,
            extension: Some("txt".to_owned()),
            archive_member: None,
        }
    }

    #[test]
    fn settings_categories_have_distinct_labels_and_start_with_general() {
        assert_eq!(SettingsCategory::ALL[0], SettingsCategory::General);
        assert_eq!(SettingsCategory::default(), SettingsCategory::General);
        let mut keys: Vec<_> = SettingsCategory::ALL
            .iter()
            .map(|category| category.label_key())
            .collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count);
    }

    #[test]
    fn category_column_follows_the_longest_label() {
        let ctx = egui::Context::default();
        let mut short = 0.0;
        let mut long = 0.0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                short = settings_category_column_width(ui, &["Общие".to_owned()]);
                long = settings_category_column_width(
                    ui,
                    &["Общие".to_owned(), "Дерево папок".to_owned()],
                );
            });
        });
        output.textures_delta.clear();
        assert!(long > short);
        assert!(short > crate::ui::style::SPACE_MD);
    }

    #[test]
    fn editing_attribute_invalidates_dir_cache() {
        let parent = std::env::temp_dir().join(format!(
            "fm-properties-cache-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&parent).unwrap();
        let key = DirCache::cache_key(&parent).unwrap();
        let mut cache = DirCache::new(4);
        let generation = cache.begin(&key);
        assert!(cache.store(&key, generation, Vec::new()));

        refresh_after_attribute_change(Ok(()), || cache.invalidate_dir(&parent));

        assert!(cache.lookup(&parent).is_none());
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn multiple_selection_does_not_issue_per_item_heavy_requests() {
        let mut tab = Tab::new(PathBuf::from(r"C:\cache-edit"));
        let first = entry("one.txt");
        let second = entry("two.txt");
        tab.selection.insert(SelectionKey::from_entry(&first));
        tab.selection.insert(SelectionKey::from_entry(&second));
        tab.entries = vec![first, second];

        assert!(selected_entry_for_details(&tab).is_none());
    }

    #[test]
    fn log_export_error_shows_understandable_message_no_stacktrace() {
        assert_eq!(
            log_export_error_key(&crate::core::log_export::LogExportError::NoLogFiles),
            "logs.export_no_logs"
        );
        let error = crate::core::log_export::LogExportError::Io {
            path: PathBuf::from(r"C:\export\file.txt"),
            source: std::io::Error::other("HRESULT 0x80070005\nstack trace: internal detail"),
            incomplete_path: None,
        };
        let key = log_export_error_key(&error);
        let text = crate::core::i18n::I18n::new(crate::core::i18n::Language::En)
            .t(key)
            .to_owned();
        assert_eq!(key, "error.operation.failed");
        assert!(!text.contains("HRESULT"));
        assert!(!text.contains("stack trace"));
        assert!(text.contains("operation could not be completed"));
    }

    #[test]
    fn export_hint_mentions_paths_localized() {
        for (json, path_word, journal_word, privacy_word) in [
            (
                include_str!("../../../i18n/en.json"),
                "file paths",
                "operations journal",
                "Nothing is sent automatically",
            ),
            (
                include_str!("../../../i18n/ru.json"),
                "пути к вашим файлам",
                "журнал операций",
                "никуда не отправляются автоматически",
            ),
        ] {
            let labels: serde_json::Value = serde_json::from_str(json).unwrap();
            let hint = labels["logs.export_hint"].as_str().unwrap();
            assert!(hint.contains(path_word));
            assert!(hint.contains(journal_word));
            assert!(hint.contains(privacy_word));
            for key in [
                "logs.exporting",
                "logs.export_freshness_confirmed",
                "logs.export_pending_note",
                "logs.export_skipped_header",
                "logs.export_skipped_note",
                "logs.export_temp_left",
            ] {
                assert!(labels[key].as_str().is_some(), "missing localized key {key}");
            }
        }
    }

    #[test]
    fn export_entry_in_about_window_not_second_window_menu_bar_untouched() {
        let dialogs = include_str!("dialogs.rs");
        let toolbar = include_str!("toolbar.rs");
        assert!(dialogs.contains("ViewportId::from_hash_of(\"filemanager-about\")"));
        assert!(dialogs.contains("self.export_logs_from_about()"));
        assert!(!toolbar.contains("logs.export"));
    }

    #[test]
    fn export_runs_off_ui_thread_or_returns_immediately_with_status() {
        let source = include_str!("dialogs.rs");
        let start = source.find("fn export_logs_from_about").unwrap();
        let tail = &source[start..];
        let end = tail.find("\n    pub(super) fn settings_window").unwrap();
        let handler = &tail[..end];
        assert!(handler.contains("self.log_export_status = Some(self.t(\"logs.exporting\")"));
        assert!(handler.contains("thread::spawn"));
        assert!(
            handler.find("thread::spawn").unwrap()
                < handler.find("crate::core::log_export::export_logs").unwrap()
        );
        assert!(handler.contains("WorkerMsg::LogExportFinished"));
    }

    #[test]
    fn settings_reset_requires_confirmation() {
        let source = include_str!("dialogs.rs");
        assert!(source.contains("self.settings_reset_confirm = true;"));
        assert!(source.contains("settings.reset_confirm_action"));
        assert!(source.contains("if let Some(confirmed) = reset_choice"));
        assert!(source.contains("if confirmed {\n                self.apply_settings_reset(ctx);"));
    }

    #[test]
    fn settings_reset_confirmation_describes_scope_honestly() {
        for (json, scope_words, issue_words) in [
            (
                include_str!("../../../i18n/en.json"),
                [
                    "Behavior preferences",
                    "bookmarks",
                    "tabs",
                    "protected-folder list",
                    "Files",
                    "Recycle Bin",
                    "system bookmarks",
                ],
                ["could not be loaded", "Only data currently in memory", "cannot recover"],
            ),
            (
                include_str!("../../../i18n/ru.json"),
                [
                    "Параметры поведения",
                    "закладки",
                    "вкладки",
                    "список папок защищённой корзины",
                    "Файлы",
                    "корзин",
                    "системные закладки",
                ],
                ["не удалось загрузить", "памяти сейчас", "восстановить"],
            ),
        ] {
            let labels: serde_json::Value = serde_json::from_str(json).unwrap();
            let scope = labels["settings.reset_confirm_scope"].as_str().unwrap();
            for word in scope_words {
                assert!(scope.contains(word), "missing scope text: {word}");
            }
            let load_issue = labels["settings.reset_confirm_load_issue"]
                .as_str()
                .unwrap();
            for word in issue_words {
                assert!(load_issue.contains(word), "missing load-issue text: {word}");
            }
        }
    }

    #[test]
    fn first_launch_missing_file_reset_writes_defaults_without_damage_warning() {
        let dir = std::env::temp_dir().join(format!(
            "fm-reset-first-launch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = crate::core::settings::SettingsStore::load(&dir);
        assert_eq!(store.load_issue, None);
        assert_eq!(settings_reset_load_warning(store.load_issue), None);

        store.settings.reset_behavior_preserving_user_data();
        store.save().unwrap();

        let loaded = crate::core::settings::SettingsStore::load(&dir);
        assert_eq!(loaded.load_issue, None);
        assert_eq!(
            serde_json::to_value(&loaded.settings).unwrap(),
            serde_json::to_value(crate::core::settings::Settings::default()).unwrap()
        );
        let scope = crate::core::i18n::I18n::new(crate::core::i18n::Language::En)
            .t("settings.reset_confirm_scope")
            .to_owned();
        assert!(!scope.contains("corrupt"));
        assert!(!scope.contains("damaged"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_reset_applies_without_restart_or_documents_need() {
        assert!(settings_reset_requires_restart(false));
        assert!(!settings_reset_requires_restart(true));
        let source = include_str!("dialogs.rs");
        let start = source.find("fn apply_settings_reset").unwrap();
        let end = source[start..]
            .find("\n    fn settings_general")
            .unwrap()
            + start;
        let reset = &source[start..end];
        assert!(reset.contains("self.bg_ops_pool"));
        assert!(reset.contains("self.dir_cache"));
        assert!(reset.contains("self.sync_dir_watch(ctx)"));
        assert!(reset.contains("self.navigate_fresh"));
        assert!(reset.contains("self.i18n.set_language"));
    }

    #[test]
    fn settings_reset_entry_in_settings_window_not_menu_bar_about_untouched() {
        let dialogs = include_str!("dialogs.rs");
        let toolbar = include_str!("toolbar.rs");
        let about_start = dialogs.find("pub(super) fn about_window").unwrap();
        let settings_start = dialogs.find("pub(super) fn settings_window").unwrap();
        assert!(!dialogs[about_start..settings_start].contains("settings.reset"));
        assert!(dialogs[settings_start..].contains("settings.reset_button"));
        assert!(!toolbar.contains("settings.reset"));
    }
}
