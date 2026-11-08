//! Действия пользователя для просмотра и извлечения ZIP.

use super::*;

impl FileManagerApp {
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

    pub(super) fn zip_status_window(&mut self, ctx: &egui::Context) {
        if !self.zip_busy && self.zip_message.is_none() {
            return;
        }
        let title = self.t("zip.window").to_owned();
        let extracting = self.t("zip.extracting").to_owned();
        let close_label = self.t("common.close").to_owned();
        let mut open = true;
        let mut close_requested = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                if self.zip_busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&extracting);
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
        }
    }
}
