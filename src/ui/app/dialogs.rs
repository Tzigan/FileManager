//! Диалоги настроек, свойств и обновления.

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
            egui::vec2(500.0, 240.0),
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
                if ui.button(self.t("update.check").to_owned()).clicked() {
                    self.spawn_update_check(&viewport_ctx);
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
        self.update_err = None;
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let msg = match crate::core::update::download(&manifest) {
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

    pub(super) fn update_window(&mut self, ctx: &egui::Context) {
        if self.pending_update.is_none() && self.update_err.is_none() && !self.update_busy {
            return;
        }
        let title = self.t("update.title").to_owned();
        let found = self.pending_update.clone();
        let mut open = true;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                if self.update_busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(self.t("update.downloading").to_owned());
                    });
                }
                if let Some(manifest) = &found {
                    ui.label(self.t("update.found").replace("{v}", &manifest.version));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.update_busy,
                                egui::Button::new(self.t("update.now").to_owned()),
                            )
                            .clicked()
                        {
                            self.start_update_download(ctx);
                        }
                        if ui.button(self.t("update.later").to_owned()).clicked() {
                            self.pending_update = None;
                        }
                    });
                }
                if let Some(e) = &self.update_err {
                    ui.label(e.as_str());
                }
            });
        if !open {
            self.pending_update = None;
            self.update_err = None;
        }
    }
}
