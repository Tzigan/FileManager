//! Диалоги настроек, свойств и обновления.

use anyhow::Context;

use super::*;

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
                                self.install_ready_update(ctx, &viewport_ctx);
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
            egui::vec2(560.0, 205.0),
            self.settings_viewport_placed,
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
                let content_height = ui.available_height();
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(150.0, content_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.add_space(style::SPACE_SM);
                            let _ = ui.selectable_label(true, self.t("settings.category.general"));
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
                    ui.vertical(|ui| {
                        ui.add_space(style::SPACE_MD);
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
                        ui.label(
                            egui::RichText::new(self.t("settings.single_instance_hint")).weak(),
                        );
                    });
                });
            });
        });
        self.settings_viewport_placed = !close_requested;
        if close_requested {
            self.show_settings = false;
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

    fn install_ready_update(&mut self, main_ctx: &egui::Context, about_ctx: &egui::Context) {
        let Some(stage_dir) = self.update_ready.clone() else {
            return;
        };
        let launch_result = std::env::current_exe()
            .context("locate current FileManager executable")
            .and_then(|target| {
                crate::platform::launch_updater(&stage_dir, &target, std::process::id())
            });
        match launch_result {
            Ok(()) => {
                about_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                main_ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(error) => {
                tracing::error!(path = %stage_dir.display(), error = %error, "failed to launch update installer");
                let _ = std::fs::remove_dir_all(&stage_dir);
                self.update_ready = None;
                self.update_status = None;
                self.update_err = Some(self.t("update.install_failed").to_owned());
                main_ctx.request_repaint();
                about_ctx.request_repaint();
            }
        }
    }

    pub(super) fn act_props(&mut self) {
        let tab = &self.tabs[self.active];
        if tab.archive_prefix.is_some() {
            return;
        }
        let idx = tab
            .selection
            .iter()
            .next()
            .copied()
            .or(if tab.entries.is_empty() {
                None
            } else {
                Some(tab.cursor)
            });
        if let Some(e) = idx.and_then(|i| tab.entries.get(i)) {
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
