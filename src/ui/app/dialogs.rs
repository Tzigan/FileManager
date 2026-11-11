//! Диалоги настроек, свойств и обновления.

use anyhow::Context;

use super::*;

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
    text_width + style::SPACE_MD * 2.0
}

impl FileManagerApp {
    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let title = self.t("about.title").to_owned();
        let config_label = self.t("label.config_dir").to_owned();
        let log_label = self.t("label.log_dir").to_owned();
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-about");
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(520.0, 350.0),
            self.about_viewport_placed,
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
                ui.heading(format!("FileManager v{}", env!("CARGO_PKG_VERSION")));
                ui.label("Rust + egui/eframe");
                ui.add_space(style::SPACE_SM);
                ui.label(format!(
                    "{}: {}",
                    config_label,
                    self.dirs.config_dir.display()
                ));
                ui.label(format!("{}: {}", log_label, self.dirs.log_dir.display()));
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
            egui::vec2(560.0, 760.0),
            self.settings_viewport_placed,
        );
        let mut close_requested = false;
        let mut hidden_files_changed = false;
        let mut wsl_visibility_changed = false;
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
                ui.horizontal(|ui| {
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
                    let page_size = ui.available_size();
                    ui.allocate_ui(page_size, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("settings-page")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.add_space(style::SPACE_MD);
                                match selected {
                                    SettingsCategory::General => self.settings_general(ui),
                                    SettingsCategory::Files => {
                                        self.settings_files(ui, &mut hidden_files_changed);
                                    }
                                    SettingsCategory::Tree => {
                                        self.settings_tree(ui, &mut wsl_visibility_changed);
                                    }
                                    SettingsCategory::Navigation => self.settings_navigation(ui),
                                    SettingsCategory::Trash => self.delete_mode_settings(ui),
                                    SettingsCategory::Updates => self.settings_updates(ui),
                                }
                                ui.add_space(style::SPACE_MD);
                            });
                    });
                });
            });
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
        self.settings_viewport_placed = !close_requested;
        if close_requested {
            self.show_settings = false;
        }
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

    pub(super) fn act_props(&mut self) {
        let tab = &self.tabs[self.active];
        if tab.archive_prefix.is_some() {
            return;
        }
        let selected = if tab.selection.len() == 1 {
            tab.selection
                .iter()
                .next()
                .and_then(|key| tab.entries.iter().find(|entry| key.matches(entry)))
        } else {
            tab.entries.get(tab.cursor)
        };
        if let Some(e) = selected {
            self.props_target = Some(e.path.clone());
        }
    }

    pub(super) fn props_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.props_target.clone() else {
            return;
        };
        let title = self.t("props.title").to_owned();
        let mut dirty = false;
        let mut apply = None;
        let viewport_id = egui::ViewportId::from_hash_of("filemanager-properties");
        let builder = floating_dialog_viewport(
            ctx,
            title,
            egui::vec2(560.0, 360.0),
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
                let entry = self.tabs[self.active]
                    .entries
                    .iter()
                    .find(|e| e.path == target)
                    .cloned();
                let name = entry.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| {
                    target
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| target.display().to_string())
                });
                ui.heading(&name);
                ui.label(format!("{}: {}", self.t("props.path"), target.display()));
                if let Some(e) = &entry {
                    let ty = if e.is_dir {
                        self.t("type.folder").to_owned()
                    } else {
                        match &e.extension {
                            Some(x) => format!("{} {}", x.to_uppercase(), self.t("type.file")),
                            None => self.t("type.file").to_owned(),
                        }
                    };
                    ui.label(format!("{}: {ty}", self.t("col.type")));
                    if let Some(s) = e.size {
                        ui.label(format!(
                            "{}: {}",
                            self.t("col.size"),
                            crate::core::format::format_size(s)
                        ));
                    }
                    if let Some(m) = e.modified {
                        ui.label(format!(
                            "{}: {}",
                            self.t("col.modified"),
                            crate::platform::format_local_time(m, self.i18n.language())
                        ));
                    }
                    if let Some(c) = e.created {
                        ui.label(format!(
                            "{}: {}",
                            self.t("props.created"),
                            crate::platform::format_local_time(c, self.i18n.language())
                        ));
                    }
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
        self.props_viewport_placed = !close_requested;
        if let Some((ro, hi)) = apply {
            if crate::platform::set_attributes(&target, ro, hi).is_ok() {
                self.refresh(ctx);
            }
        }
        if close_requested {
            self.props_target = None;
        }
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
    use super::{SettingsCategory, settings_category_column_width};

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
}
