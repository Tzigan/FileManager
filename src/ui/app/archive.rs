//! Действия пользователя для просмотра и извлечения ZIP.

use super::*;

impl FileManagerApp {
    pub(crate) fn can_start_zip_operation(&self) -> bool {
        !self.zip_busy
    }

    pub(crate) fn create_zip_from_selection(&mut self, sources: Vec<PathBuf>, ctx: &egui::Context) {
        if sources.is_empty() {
            return;
        }
        let tab_id = self.tabs[self.active].id;
        let destination = self.tabs[self.active].path.clone();
        self.spawn_zip_creation(sources, destination, tab_id, ctx);
    }

    pub(crate) fn extract_zip_file(&mut self, member: String, ctx: &egui::Context) {
        let archive = self.tabs[self.active].path.clone();
        let destination = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        self.spawn_zip_extraction(
            move || crate::platform::extract_zip_file(&archive, &member, &destination),
            ctx,
        );
    }

    pub(crate) fn extract_zip_folder(&mut self, folder: String, ctx: &egui::Context) {
        let archive = self.tabs[self.active].path.clone();
        let destination = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        self.spawn_zip_extraction(
            move || crate::platform::extract_zip_folder(&archive, &folder, &destination),
            ctx,
        );
    }

    pub(crate) fn extract_zip_all(&mut self, ctx: &egui::Context) {
        let archive = self.tabs[self.active].path.clone();
        self.extract_zip_all_from(archive, ctx);
    }

    pub(crate) fn extract_zip_all_from(&mut self, archive: PathBuf, ctx: &egui::Context) {
        let destination = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        self.spawn_zip_extraction(
            move || crate::platform::extract_zip_all(&archive, &destination),
            ctx,
        );
    }

    fn spawn_zip_extraction(
        &mut self,
        extraction: impl FnOnce() -> std::io::Result<PathBuf> + Send + 'static,
        ctx: &egui::Context,
    ) {
        if self.zip_busy {
            return;
        }
        self.zip_generation = self.zip_generation.wrapping_add(1);
        self.zip_busy = true;
        self.zip_creating = false;
        self.zip_message = None;
        let generation = self.zip_generation;
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = extraction();
            let _ = tx.send(WorkerMsg::ZipExtracted { generation, result });
            ctx.request_repaint();
        });
    }

    fn spawn_zip_creation(
        &mut self,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        tab_id: u64,
        ctx: &egui::Context,
    ) {
        if self.zip_busy {
            return;
        }
        self.zip_generation = self.zip_generation.wrapping_add(1);
        self.zip_busy = true;
        self.zip_creating = true;
        self.zip_message = None;
        let generation = self.zip_generation;
        let tx = self.msg_tx.clone();
        let message_destination = destination.clone();
        let repaint_ctx = ctx.clone();
        ctx.request_repaint();
        thread::spawn(move || {
            let result = crate::platform::create_zip_archive(&sources, &destination);
            let _ = tx.send(WorkerMsg::ZipCreated {
                generation,
                tab_id,
                destination: message_destination,
                result,
            });
            repaint_ctx.request_repaint();
        });
    }

    pub(super) fn zip_error_text(&self, error: &std::io::Error) -> String {
        if error
            .raw_os_error()
            .is_some_and(|code| code == 32 || code == 33)
        {
            return self.t("zip.error_busy").to_owned();
        }
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => self.t("error.access_denied").to_owned(),
            std::io::ErrorKind::NotFound => self.t("zip.error_invalid").to_owned(),
            std::io::ErrorKind::InvalidData => {
                if error.to_string().contains("unsafe path") {
                    self.t("zip.error_path").to_owned()
                } else {
                    self.t("zip.error_invalid").to_owned()
                }
            }
            std::io::ErrorKind::InvalidInput => {
                if error.to_string().contains("filename encoding") {
                    self.t("zip.error_names").to_owned()
                } else {
                    self.t("zip.error_path").to_owned()
                }
            }
            std::io::ErrorKind::AlreadyExists => self.t("zip.error_exists").to_owned(),
            std::io::ErrorKind::StorageFull => self.t("zip.error_space").to_owned(),
            std::io::ErrorKind::WouldBlock => self.t("zip.error_busy").to_owned(),
            _ => self.t("zip.error").to_owned(),
        }
    }

    pub(super) fn zip_create_error_text(&self, error: &std::io::Error) -> String {
        if error
            .raw_os_error()
            .is_some_and(|code| code == 32 || code == 33)
        {
            return self.t("zip.error_busy").to_owned();
        }
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => self.t("error.access_denied").to_owned(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
                self.t("zip.create_error_path").to_owned()
            }
            std::io::ErrorKind::InvalidData
                if error.to_string().contains("ZIP size limit")
                    || error.to_string().contains("ZIP member limit")
                    || error.to_string().contains("ZIP listing limit") =>
            {
                self.t("zip.create_error_limit").to_owned()
            }
            std::io::ErrorKind::InvalidData => {
                format!("{}: {error}", self.t("zip.create_error"))
            }
            std::io::ErrorKind::AlreadyExists => self.t("zip.error_exists").to_owned(),
            std::io::ErrorKind::StorageFull => self.t("zip.create_error_space").to_owned(),
            std::io::ErrorKind::WouldBlock => self.t("zip.error_busy").to_owned(),
            _ => format!("{}: {error}", self.t("zip.create_error")),
        }
    }

    pub(super) fn zip_status_window(&mut self, ctx: &egui::Context) {
        if !self.zip_busy && self.zip_message.is_none() {
            return;
        }
        let title = self
            .t(if self.zip_creating {
                "zip.create_window"
            } else {
                "zip.window"
            })
            .to_owned();
        let busy_label = self
            .t(if self.zip_creating {
                "zip.creating"
            } else {
                "zip.extracting"
            })
            .to_owned();
        let close_label = self.t("common.close").to_owned();
        let mut open = true;
        let mut close_requested = false;
        let mut window = egui::Window::new(title).collapsible(false).resizable(false);
        if self.zip_creating {
            let content_rect = ctx.content_rect();
            // Центрируем только окно создания в логических координатах viewport менеджера.
            window = window
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .constrain_to(content_rect)
                .max_size(content_rect.size());
        }
        window.open(&mut open).show(ctx, |ui| {
            if self.zip_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&busy_label);
                });
            } else if let Some(message) = &self.zip_message {
                ui.label(message);
                if ui.button(&close_label).clicked() {
                    close_requested = true;
                }
            }
        });
        if !open || close_requested {
            self.zip_message = None;
            if !self.zip_busy {
                self.zip_creating = false;
            }
        }
    }
}
