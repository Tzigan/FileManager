//! Действия пользователя для просмотра и извлечения ZIP.

use super::*;
use crate::platform::{
    ZipCompressionLevel, ZipCompressionMethod, ZipCreateOptions, ZipExistingBehavior,
};

#[derive(Clone)]
pub(super) struct ZipCreateDialog {
    sources: Vec<PathBuf>,
    source_root: PathBuf,
    archive_name: String,
    destination: String,
    compression_method: ZipCompressionMethod,
    compression_level: ZipCompressionLevel,
    preserve_folder_structure: bool,
    existing_behavior: ZipExistingBehavior,
    awaiting_conflict: bool,
    conflict_path: Option<PathBuf>,
    validation_error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ZipDialogAction {
    None,
    Create,
    Cancel,
    Replace,
    AddSuffix,
    Back,
}

#[derive(Debug)]
enum ZipDialogValidationError {
    Message(&'static str),
    Io(std::io::Error),
}

impl FileManagerApp {
    pub(crate) fn can_start_zip_operation(&self) -> bool {
        !self.zip_busy && self.zip_create_dialog.is_none()
    }

    pub(crate) fn create_zip_from_selection(&mut self, sources: Vec<PathBuf>, ctx: &egui::Context) {
        if sources.is_empty() || self.zip_busy {
            return;
        }
        let tab = &self.tabs[self.active];
        let default_name = suggested_zip_name(tab, &sources, self.t("zip.default_name"));
        self.zip_message = None;
        self.zip_creating = false;
        self.zip_create_dialog = Some(ZipCreateDialog {
            sources,
            source_root: tab.path.clone(),
            archive_name: default_name,
            destination: tab.path.display().to_string(),
            compression_method: ZipCompressionMethod::Deflate,
            compression_level: ZipCompressionLevel::Standard,
            preserve_folder_structure: true,
            existing_behavior: ZipExistingBehavior::AddSuffix,
            awaiting_conflict: false,
            conflict_path: None,
            validation_error: None,
        });
        ctx.request_repaint();
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
        options: ZipCreateOptions,
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
        let message_destination = options.destination.clone();
        let repaint_ctx = ctx.clone();
        ctx.request_repaint();
        thread::spawn(move || {
            let result = crate::platform::create_zip_archive(&sources, &options);
            let _ = tx.send(WorkerMsg::ZipCreated {
                generation,
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
            std::io::ErrorKind::FileTooLarge => self.t("zip.error_too_large").to_owned(),
            std::io::ErrorKind::Unsupported => self.t("zip.error_zip64_unsupported").to_owned(),
            std::io::ErrorKind::NotFound => self.t("zip.error_invalid").to_owned(),
            std::io::ErrorKind::InvalidData => {
                if error.to_string().contains("unsafe path") {
                    self.t("zip.error_path").to_owned()
                } else if error.to_string().contains("extraction limit") {
                    self.t("zip.error_limit").to_owned()
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
            std::io::ErrorKind::FileTooLarge => self.t("zip.error_too_large").to_owned(),
            std::io::ErrorKind::Unsupported => self.t("zip.create_error").to_owned(),
            std::io::ErrorKind::InvalidInput
                if error.to_string().contains("no files were selected")
                    || error.to_string().contains("selection contains no files") =>
            {
                self.t("zip.create_error_no_files").to_owned()
            }
            std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
                if error
                    .to_string()
                    .contains("archive destination is inside a selected folder")
                {
                    self.t("zip.options.error_destination_inside_source")
                        .to_owned()
                } else {
                    self.t("zip.create_error_path").to_owned()
                }
            }
            std::io::ErrorKind::InvalidData
                if error.to_string().contains("ZIP size limit")
                    || error.to_string().contains("ZIP member limit")
                    || error.to_string().contains("ZIP listing limit") =>
            {
                self.t("zip.create_error_limit").to_owned()
            }
            std::io::ErrorKind::InvalidData
                if error.to_string().contains("duplicate file names") =>
            {
                self.t("zip.create_error_duplicate").to_owned()
            }
            std::io::ErrorKind::InvalidData => self.t("zip.create_error").to_owned(),
            std::io::ErrorKind::AlreadyExists => self.t("zip.error_exists").to_owned(),
            std::io::ErrorKind::StorageFull => self.t("zip.create_error_space").to_owned(),
            std::io::ErrorKind::WouldBlock => self.t("zip.error_busy").to_owned(),
            _ => self.t("zip.create_error").to_owned(),
        }
    }

    fn start_zip_creation_from_dialog(
        &mut self,
        mut dialog: ZipCreateDialog,
        behavior: Option<ZipExistingBehavior>,
        ctx: &egui::Context,
    ) {
        if let Some(behavior) = behavior {
            dialog.existing_behavior = behavior;
        }
        dialog.validation_error = None;
        dialog.awaiting_conflict = false;
        dialog.conflict_path = None;

        let options = match validate_zip_create_dialog(&dialog) {
            Ok(options) => options,
            Err(ZipDialogValidationError::Message(key)) => {
                dialog.validation_error = Some(self.t(key).to_owned());
                self.zip_create_dialog = Some(dialog);
                return;
            }
            Err(ZipDialogValidationError::Io(error)) => {
                dialog.validation_error = Some(self.zip_create_error_text(&error));
                self.zip_create_dialog = Some(dialog);
                return;
            }
        };

        if options.existing_behavior == ZipExistingBehavior::Ask {
            let target = options.destination.join(&options.archive_name);
            match std::fs::symlink_metadata(&target) {
                Ok(_) => {
                    dialog.awaiting_conflict = true;
                    dialog.conflict_path = Some(target);
                    self.zip_create_dialog = Some(dialog);
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    dialog.validation_error = Some(self.zip_create_error_text(&error));
                    self.zip_create_dialog = Some(dialog);
                    return;
                }
            }
        }

        self.zip_create_dialog = None;
        self.spawn_zip_creation(dialog.sources, options, ctx);
    }

    pub(super) fn zip_status_window(&mut self, ctx: &egui::Context) {
        let mut dialog = self.zip_create_dialog.take();
        let options_open = dialog.is_some();
        if !options_open && !self.zip_busy && self.zip_message.is_none() {
            return;
        }

        let title = self
            .t(if options_open {
                "zip.options.title"
            } else if self.zip_creating {
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
        let name_label = self.t("zip.options.name").to_owned();
        let name_hint = self.t("zip.options.name_hint").to_owned();
        let folder_label = self.t("zip.options.folder").to_owned();
        let folder_hint = self.t("zip.options.folder_hint").to_owned();
        let method_label = self.t("zip.options.method").to_owned();
        let store_label = self.t("zip.method.store").to_owned();
        let deflate_label = self.t("zip.method.deflate").to_owned();
        let level_label = self.t("zip.options.level").to_owned();
        let fast_label = self.t("zip.level.fast").to_owned();
        let standard_label = self.t("zip.level.standard").to_owned();
        let maximum_label = self.t("zip.level.maximum").to_owned();
        let preserve_label = self.t("zip.options.preserve_structure").to_owned();
        let conflict_label = self.t("zip.options.conflict").to_owned();
        let suffix_label = self.t("zip.conflict.suffix").to_owned();
        let replace_label = self.t("zip.conflict.replace").to_owned();
        let ask_label = self.t("zip.conflict.ask").to_owned();
        let create_label = self.t("zip.options.create").to_owned();
        let cancel_label = self.t("zip.options.cancel").to_owned();
        let create_copy_label = self.t("zip.options.create_copy").to_owned();
        let back_label = self.t("zip.options.back").to_owned();
        let conflict_warning = self.t("zip.options.conflict_warning").to_owned();
        let is_busy = self.zip_busy;
        let message = self.zip_message.clone();
        let mut open = true;
        let mut close_requested = false;
        let mut action = ZipDialogAction::None;
        let mut window = egui::Window::new(title)
            .collapsible(false)
            .resizable(options_open);
        if options_open || self.zip_creating {
            let content_rect = ctx.content_rect();
            // Окно параметров и статус создания остаются в пределах viewport менеджера.
            window = window
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .constrain_to(content_rect)
                .max_size(content_rect.size());
        }
        window.open(&mut open).show(ctx, |ui| {
            if let Some(dialog) = dialog.as_mut() {
                let mut edited = false;
                ui.add_enabled_ui(!dialog.awaiting_conflict, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&name_label);
                        edited |= ui
                            .add(
                                egui::TextEdit::singleline(&mut dialog.archive_name)
                                    .desired_width(260.0)
                                    .hint_text(&name_hint),
                            )
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label(&folder_label);
                        edited |= ui
                            .add(
                                egui::TextEdit::singleline(&mut dialog.destination)
                                    .desired_width(360.0)
                                    .hint_text(&folder_hint),
                            )
                            .changed();
                    });
                    ui.separator();
                    ui.label(&method_label);
                    ui.horizontal(|ui| {
                        edited |= ui
                            .radio_value(
                                &mut dialog.compression_method,
                                ZipCompressionMethod::Store,
                                &store_label,
                            )
                            .changed();
                        edited |= ui
                            .radio_value(
                                &mut dialog.compression_method,
                                ZipCompressionMethod::Deflate,
                                &deflate_label,
                            )
                            .changed();
                    });
                    ui.add_enabled_ui(
                        dialog.compression_method == ZipCompressionMethod::Deflate,
                        |ui| {
                            ui.label(&level_label);
                            ui.horizontal(|ui| {
                                edited |= ui
                                    .radio_value(
                                        &mut dialog.compression_level,
                                        ZipCompressionLevel::Fast,
                                        &fast_label,
                                    )
                                    .changed();
                                edited |= ui
                                    .radio_value(
                                        &mut dialog.compression_level,
                                        ZipCompressionLevel::Standard,
                                        &standard_label,
                                    )
                                    .changed();
                                edited |= ui
                                    .radio_value(
                                        &mut dialog.compression_level,
                                        ZipCompressionLevel::Maximum,
                                        &maximum_label,
                                    )
                                    .changed();
                            });
                        },
                    );
                    edited |= ui
                        .checkbox(&mut dialog.preserve_folder_structure, &preserve_label)
                        .changed();
                    ui.separator();
                    ui.label(&conflict_label);
                    ui.vertical(|ui| {
                        edited |= ui
                            .radio_value(
                                &mut dialog.existing_behavior,
                                ZipExistingBehavior::AddSuffix,
                                &suffix_label,
                            )
                            .changed();
                        edited |= ui
                            .radio_value(
                                &mut dialog.existing_behavior,
                                ZipExistingBehavior::Replace,
                                &replace_label,
                            )
                            .changed();
                        edited |= ui
                            .radio_value(
                                &mut dialog.existing_behavior,
                                ZipExistingBehavior::Ask,
                                &ask_label,
                            )
                            .changed();
                    });
                });
                if edited {
                    dialog.validation_error = None;
                }
                if let Some(error) = &dialog.validation_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.separator();
                if dialog.awaiting_conflict {
                    let path = dialog
                        .conflict_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default();
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("{conflict_warning}\n{path}"),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(&replace_label).clicked() {
                            action = ZipDialogAction::Replace;
                        }
                        if ui.button(&create_copy_label).clicked() {
                            action = ZipDialogAction::AddSuffix;
                        }
                        if ui.button(&back_label).clicked() {
                            action = ZipDialogAction::Back;
                        }
                        if ui.button(&cancel_label).clicked() {
                            action = ZipDialogAction::Cancel;
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.button(&create_label).clicked() {
                            action = ZipDialogAction::Create;
                        }
                        if ui.button(&cancel_label).clicked() {
                            action = ZipDialogAction::Cancel;
                        }
                    });
                }
            } else if is_busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&busy_label);
                });
            } else if let Some(message) = &message {
                ui.label(message);
                if ui.button(&close_label).clicked() {
                    close_requested = true;
                }
            }
        });

        if options_open {
            if !open {
                return;
            }
            let Some(dialog) = dialog else {
                return;
            };
            match action {
                ZipDialogAction::Cancel => {}
                ZipDialogAction::Create => self.start_zip_creation_from_dialog(dialog, None, ctx),
                ZipDialogAction::Replace => self.start_zip_creation_from_dialog(
                    dialog,
                    Some(ZipExistingBehavior::Replace),
                    ctx,
                ),
                ZipDialogAction::AddSuffix => self.start_zip_creation_from_dialog(
                    dialog,
                    Some(ZipExistingBehavior::AddSuffix),
                    ctx,
                ),
                ZipDialogAction::Back => {
                    let mut dialog = dialog;
                    dialog.awaiting_conflict = false;
                    dialog.conflict_path = None;
                    self.zip_create_dialog = Some(dialog);
                }
                ZipDialogAction::None => self.zip_create_dialog = Some(dialog),
            }
            return;
        }

        if !open || close_requested {
            self.zip_message = None;
            if !self.zip_busy {
                self.zip_creating = false;
            }
        }
    }
}

fn suggested_zip_name(tab: &crate::ui::tabs::Tab, sources: &[PathBuf], fallback: &str) -> String {
    if sources.len() != 1 {
        return fallback.to_owned();
    }
    let Some(entry) = tab.entries.iter().find(|entry| entry.path == sources[0]) else {
        return fallback.to_owned();
    };
    let name = if entry.is_dir {
        entry.name.as_str()
    } else {
        Path::new(&entry.name)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(&entry.name)
    };
    if name.is_empty() {
        fallback.to_owned()
    } else {
        name.to_owned()
    }
}

fn validate_zip_create_dialog(
    dialog: &ZipCreateDialog,
) -> Result<ZipCreateOptions, ZipDialogValidationError> {
    if dialog.sources.is_empty() {
        return Err(ZipDialogValidationError::Message(
            "zip.options.error_selection",
        ));
    }
    let archive_name = crate::platform::normalize_zip_archive_name(&dialog.archive_name)
        .map_err(|_| ZipDialogValidationError::Message("zip.options.error_name"))?;
    let source_root =
        std::fs::canonicalize(&dialog.source_root).map_err(ZipDialogValidationError::Io)?;
    if !source_root.is_dir() {
        return Err(ZipDialogValidationError::Message(
            "zip.options.error_selection",
        ));
    }
    let destination_text = dialog.destination.trim();
    if destination_text.is_empty() {
        return Err(ZipDialogValidationError::Message(
            "zip.options.error_folder",
        ));
    }
    let destination = PathBuf::from(destination_text);
    let destination_metadata = match std::fs::metadata(&destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ZipDialogValidationError::Message(
                "zip.options.error_folder",
            ));
        }
        Err(error) => return Err(ZipDialogValidationError::Io(error)),
    };
    if !destination_metadata.is_dir() {
        return Err(ZipDialogValidationError::Message(
            "zip.options.error_folder",
        ));
    }

    Ok(ZipCreateOptions {
        source_root,
        destination,
        archive_name,
        compression_method: dialog.compression_method,
        compression_level: dialog.compression_level,
        preserve_folder_structure: dialog.preserve_folder_structure,
        existing_behavior: dialog.existing_behavior,
    })
}

#[cfg(test)]
mod zip_dialog_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("filemanager-zip-dialog-{id}"));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn dialog_validates_name_folder_and_selected_items_before_building_options() {
        let root = temp_dir();
        let source = root.join("notes.txt");
        std::fs::write(&source, b"selected").unwrap();
        let mut dialog = ZipCreateDialog {
            sources: vec![source],
            source_root: root.clone(),
            archive_name: "notes".to_owned(),
            destination: root.display().to_string(),
            compression_method: ZipCompressionMethod::Store,
            compression_level: ZipCompressionLevel::Maximum,
            preserve_folder_structure: false,
            existing_behavior: ZipExistingBehavior::Ask,
            awaiting_conflict: false,
            conflict_path: None,
            validation_error: None,
        };

        let options = validate_zip_create_dialog(&dialog).unwrap();
        assert_eq!(options.archive_name, "notes.zip");
        assert_eq!(options.compression_method, ZipCompressionMethod::Store);
        assert_eq!(options.compression_level, ZipCompressionLevel::Maximum);
        assert!(!options.preserve_folder_structure);
        assert_eq!(options.existing_behavior, ZipExistingBehavior::Ask);

        dialog.archive_name = "../invalid".to_owned();
        assert!(matches!(
            validate_zip_create_dialog(&dialog),
            Err(ZipDialogValidationError::Message("zip.options.error_name"))
        ));
        dialog.archive_name = "notes".to_owned();
        dialog.destination = root.join("missing").display().to_string();
        assert!(matches!(
            validate_zip_create_dialog(&dialog),
            Err(ZipDialogValidationError::Message(
                "zip.options.error_folder"
            ))
        ));
        dialog.destination = root.display().to_string();
        dialog.sources.clear();
        assert!(matches!(
            validate_zip_create_dialog(&dialog),
            Err(ZipDialogValidationError::Message(
                "zip.options.error_selection"
            ))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
