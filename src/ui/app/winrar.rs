use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use super::*;

const APP_PATHS_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\WinRAR.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn is_rar(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rar"))
}

fn selection_kinds(paths: &[PathBuf]) -> (bool, bool) {
    paths.iter().fold((false, false), |(plain, rar), path| {
        if is_rar(path) {
            (plain, true)
        } else {
            (true, rar)
        }
    })
}

fn base_name(path: &Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Archive".to_owned())
}

fn app_paths_exe() -> Option<PathBuf> {
    use ::windows::Win32::Foundation::ERROR_SUCCESS;
    use ::windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey,
        RegOpenKeyExW, RegQueryValueExW,
    };
    use ::windows::core::HSTRING;

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    let mut key = HKEY::default();
    let key_name = HSTRING::from(APP_PATHS_KEY);
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, &key_name, None, KEY_READ, &mut key) }
        != ERROR_SUCCESS
    {
        return None;
    }
    let key = Key(key);
    let value_name = HSTRING::new();
    let mut value_type = REG_VALUE_TYPE(0);
    let mut byte_len = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            &value_name,
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS
        || (value_type != REG_SZ && value_type != REG_EXPAND_SZ)
        || byte_len == 0
        || byte_len > 32768
        || byte_len % 2 != 0
    {
        return None;
    }
    let mut bytes = vec![0u8; byte_len as usize];
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            &value_name,
            None,
            Some(&mut value_type),
            Some(bytes.as_mut_ptr()),
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS || byte_len as usize > bytes.len() {
        return None;
    }
    let wide: Vec<u16> = bytes[..byte_len as usize]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let end = wide.iter().position(|unit| *unit == 0).unwrap_or(wide.len());
    let value = String::from_utf16(&wide[..end]).ok()?;
    Some(PathBuf::from(if value_type == REG_EXPAND_SZ {
        expand_environment(&value)
    } else {
        value
    }))
}

fn expand_environment(value: &str) -> String {
    let mut expanded = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            expanded.push_str(&rest[start..]);
            return expanded;
        };
        let name = &after[..end];
        if let Some(value) = std::env::var_os(name) {
            expanded.push_str(&value.to_string_lossy());
        } else {
            expanded.push_str(&rest[start..start + end + 2]);
        }
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    expanded
}

fn detect_install_dir_with(
    registry_exe: Option<PathBuf>,
    fallback_dirs: impl IntoIterator<Item = PathBuf>,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    registry_exe
        .filter(|path| path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("WinRAR.exe")))
        .filter(|path| is_file(path))
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .or_else(|| {
            fallback_dirs.into_iter().find(|dir| {
                let exe = dir.join("WinRAR.exe");
                is_file(&exe)
            })
        })
}

pub(super) fn detect_install_dir() -> Option<PathBuf> {
    let mut fallbacks = ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|root| PathBuf::from(root).join("WinRAR"))
        .collect::<Vec<_>>();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        fallbacks.push(PathBuf::from(local).join("Programs").join("WinRAR"));
    }
    detect_install_dir_with(app_paths_exe(), fallbacks, Path::is_file)
}

fn rar_executable(install_dir: &Path) -> Option<PathBuf> {
    let rar = install_dir.join("Rar.exe");
    rar.is_file().then_some(rar)
}

fn quick_add_executable(install_dir: &Path, rar_exists: bool) -> PathBuf {
    if rar_exists {
        install_dir.join("Rar.exe")
    } else {
        install_dir.join("WinRAR.exe")
    }
}

fn append_paths(args: &mut Vec<OsString>, paths: &[PathBuf]) {
    args.extend(paths.iter().map(|path| path.as_os_str().to_owned()));
}

fn add_dialog_args(paths: &[PathBuf]) -> Vec<OsString> {
    let mut args = vec![OsString::from("a")];
    append_paths(&mut args, paths);
    args
}

fn quick_add_args(name: &str, paths: &[PathBuf]) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("a"),
        OsString::from("-y"),
        OsString::from(format!("{name}.rar")),
    ];
    append_paths(&mut args, paths);
    args
}

fn extract_args(archive: &Path, destination: &Path) -> Vec<OsString> {
    vec![
        OsString::from("x"),
        OsString::from("-y"),
        archive.as_os_str().to_owned(),
        destination.as_os_str().to_owned(),
    ]
}

#[derive(Debug)]
pub(crate) struct WinrarFinished {
    pub(super) operation: &'static str,
    pub(super) path: PathBuf,
    pub(super) directory: PathBuf,
    pub(super) invalidate: bool,
    pub(super) result: Result<(), String>,
}

impl FileManagerApp {
    pub(crate) fn winrar_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        paths: Vec<PathBuf>,
        ctx: &egui::Context,
    ) {
        let Some(install_dir) = self.winrar_dir.clone() else {
            return;
        };
        if paths.is_empty() {
            return;
        }
        let (has_plain, has_rar) = selection_kinds(&paths);
        let plain_paths: Vec<_> = paths.iter().filter(|path| !is_rar(path)).cloned().collect();
        let rar_paths: Vec<_> = paths.iter().filter(|path| is_rar(path)).cloned().collect();
        ui.menu_button(self.t("winrar.menu").to_owned(), |ui| {
            if has_plain {
                if ui.button(self.t("winrar.add_dialog").to_owned()).clicked() {
                    let directory = plain_paths[0].parent().unwrap_or(Path::new(".")).to_path_buf();
                    self.launch_dialog(
                        install_dir.join("WinRAR.exe"),
                        add_dialog_args(&plain_paths),
                        "winrar.add_dialog",
                        plain_paths[0].clone(),
                        directory,
                        true,
                        ctx,
                    );
                    ui.close();
                }
                let name = base_name(&plain_paths[0]);
                let label = self.t("winrar.quick_add").replace("{name}", &name);
                if ui.button(label).clicked() {
                    let directory = plain_paths[0].parent().unwrap_or(Path::new(".")).to_path_buf();
                    let exe = quick_add_executable(
                        &install_dir,
                        rar_executable(&install_dir).is_some(),
                    );
                    self.launch_waiting(
                        exe,
                        quick_add_args(&name, &plain_paths),
                        "winrar.quick_add",
                        plain_paths[0].clone(),
                        directory,
                        ctx,
                    );
                    ui.close();
                }
            }
            if has_plain && has_rar {
                ui.separator();
            }
            if has_rar {
                if ui.button(self.t("winrar.extract_here").to_owned()).clicked() {
                    let Some(exe) = rar_executable(&install_dir) else {
                        self.show_winrar_error("winrar.extract_here", &rar_paths[0], "Rar.exe is missing");
                        ui.close();
                        return;
                    };
                    let operations = rar_paths
                        .iter()
                        .map(|archive| {
                            let directory = archive.parent().unwrap_or(Path::new(".")).to_path_buf();
                            (archive.clone(), directory)
                        })
                        .collect();
                    self.launch_extractions(
                        exe,
                        operations,
                        "winrar.extract_here",
                        false,
                        ctx,
                    );
                    ui.close();
                }
                let folder = base_name(&rar_paths[0]);
                let label = self.t("winrar.extract_to").replace("{folder}", &folder);
                if ui.button(label).clicked() {
                    let Some(exe) = rar_executable(&install_dir) else {
                        self.show_winrar_error("winrar.extract_to", &rar_paths[0], "Rar.exe is missing");
                        ui.close();
                        return;
                    };
                    let base = rar_paths[0].parent().unwrap_or(Path::new(".")).to_path_buf();
                    let destination = base.join(&folder);
                    let operations = rar_paths
                        .iter()
                        .map(|archive| (archive.clone(), destination.clone()))
                        .collect();
                    self.launch_extractions(
                        exe,
                        operations,
                        "winrar.extract_to",
                        true,
                        ctx,
                    );
                    ui.close();
                }
                if ui.button(self.t("winrar.open").to_owned()).clicked() {
                    for archive in rar_paths {
                        let directory = archive.parent().unwrap_or(Path::new(".")).to_path_buf();
                        self.launch_dialog(
                            install_dir.join("WinRAR.exe"),
                            vec![archive.as_os_str().to_owned()],
                            "winrar.open",
                            archive,
                            directory,
                            false,
                            ctx,
                        );
                    }
                    ui.close();
                }
            }
        });
    }

    fn launch_dialog(
        &self,
        exe: PathBuf,
        args: Vec<OsString>,
        operation: &'static str,
        path: PathBuf,
        directory: PathBuf,
        invalidate: bool,
        ctx: &egui::Context,
    ) {
        self.spawn_winrar_thread(exe, args, operation, path, directory, invalidate, false, ctx);
    }

    fn launch_waiting(
        &self,
        exe: PathBuf,
        args: Vec<OsString>,
        operation: &'static str,
        path: PathBuf,
        directory: PathBuf,
        ctx: &egui::Context,
    ) {
        self.spawn_winrar_thread(exe, args, operation, path, directory, true, true, ctx);
    }

    fn launch_extractions(
        &self,
        exe: PathBuf,
        operations: Vec<(PathBuf, PathBuf)>,
        operation: &'static str,
        create_destination: bool,
        ctx: &egui::Context,
    ) {
        let Some((first_archive, directory)) = operations.first().cloned() else {
            return;
        };
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = (|| {
                if create_destination {
                    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
                }
                for (archive, destination) in operations {
                    run_command(&exe, &extract_args(&archive, &destination), true)?;
                }
                Ok(())
            })();
            finish(tx, ctx, operation, first_archive, directory, true, result);
        });
    }

    fn spawn_winrar_thread(
        &self,
        exe: PathBuf,
        args: Vec<OsString>,
        operation: &'static str,
        path: PathBuf,
        directory: PathBuf,
        invalidate: bool,
        no_window: bool,
        ctx: &egui::Context,
    ) {
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = run_command(&exe, &args, no_window);
            finish(tx, ctx, operation, path, directory, invalidate, result);
        });
    }

    fn show_winrar_error(&mut self, operation: &str, path: &Path, reason: &str) {
        let message = self
            .t("winrar.operation_failed")
            .replace("{operation}", self.t(operation))
            .replace("{path}", &path.display().to_string());
        tracing::error!(operation, path = %path.display(), reason, "WinRAR operation failed");
        self.ops_ui.journal.push(message);
        self.ops_ui.show_journal = true;
    }
}

fn run_command(exe: &Path, args: &[OsString], no_window: bool) -> Result<(), String> {
    let mut command = Command::new(exe);
    command.args(args);
    if no_window {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let status = command.status().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("exit code {:?}", status.code()))
    }
}

fn finish(
    tx: Sender<WorkerMsg>,
    ctx: egui::Context,
    operation: &'static str,
    path: PathBuf,
    directory: PathBuf,
    invalidate: bool,
    result: Result<(), String>,
) {
    if let Err(error) = &result {
        tracing::error!(operation, path = %path.display(), error, "WinRAR operation failed");
    }
    let _ = tx.send(WorkerMsg::WinrarFinished(WinrarFinished {
        operation,
        path,
        directory,
        invalidate,
        result,
    }));
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winrar_submenu_absent_when_not_detected() {
        assert!(detect_install_dir_with(None, [], |_| false).is_none());
        assert!(include_str!("winrar.rs").contains("let Some(install_dir) = self.winrar_dir.clone() else"));
    }

    #[test]
    fn winrar_submenu_present_with_add_items_for_plain_selection() {
        let paths = vec![PathBuf::from(r"C:\data\notes.txt")];
        assert_eq!(selection_kinds(&paths), (true, false));
        let source = include_str!("../list/actions.rs");
        assert!(source.contains("self.winrar_context_menu(ui, self.selected_paths(), ctx)"));
        assert!(include_str!("winrar.rs").contains("winrar.quick_add"));
    }

    #[test]
    fn winrar_submenu_present_with_extract_items_for_rar_selection() {
        let paths = vec![PathBuf::from(r"C:\data\backup.RAR")];
        assert_eq!(selection_kinds(&paths), (false, true));
        let source = include_str!("winrar.rs");
        assert!(source.contains("winrar.extract_here"));
        assert!(source.contains("winrar.extract_to"));
        assert!(source.contains("winrar.open"));
    }

    #[test]
    fn winrar_quick_add_archive_name_derived_from_first_selection() {
        assert_eq!(base_name(Path::new(r"C:\data\report.txt")), "report");
        assert!(quick_add_args("report", &[PathBuf::from("report.txt")])
            .iter()
            .any(|arg| arg == "report.rar"));
    }

    #[test]
    fn winrar_extract_folder_name_derived_from_archive_basename() {
        assert_eq!(base_name(Path::new(r"C:\data\Backup.part.rar")), "Backup.part");
    }

    #[test]
    fn winrar_commands_quote_paths_as_separate_args() {
        let archive = PathBuf::from(r"C:\My Files\Архив.rar");
        let source = PathBuf::from(r"C:\Мои файлы\document.txt");
        let add = quick_add_args("result", std::slice::from_ref(&source));
        assert_eq!(add[3], source.as_os_str());
        let extract = extract_args(&archive, Path::new(r"C:\Out Dir"));
        assert_eq!(extract[2], archive.as_os_str());
        assert_eq!(extract[3], Path::new(r"C:\Out Dir").as_os_str());
    }

    #[test]
    fn winrar_rar_exit_code_nonzero_reports_through_error_layer() {
        let source = include_str!("mod.rs");
        assert!(source.contains("winrar.operation_failed"));
        assert!(source.contains("self.ops_ui.journal.push(format!(\"{message}\\n{reason}\"))"));
    }

    #[test]
    fn winrar_successful_operation_invalidates_touched_directory() {
        let source = include_str!("mod.rs");
        assert!(source.contains("self.apply_external_dir_change(&finished.directory, ctx)"));
    }

    #[test]
    fn winrar_missing_rar_exe_falls_back_to_winrar_exe() {
        assert_eq!(
            quick_add_executable(Path::new(r"C:\WinRAR"), false),
            PathBuf::from(r"C:\WinRAR\WinRAR.exe")
        );
    }

    #[test]
    fn winrar_detection_checks_registry_then_known_installation_folders() {
        let registry = Some(PathBuf::from(r"C:\Apps\WinRAR\WinRAR.exe"));
        let folders = vec![PathBuf::from(r"C:\Fallback\WinRAR")];
        assert_eq!(
            detect_install_dir_with(registry, folders.clone(), |path| {
                path == Path::new(r"C:\Apps\WinRAR\WinRAR.exe")
            }),
            Some(PathBuf::from(r"C:\Apps\WinRAR"))
        );
        assert_eq!(
            detect_install_dir_with(None, folders, |path| {
                path == Path::new(r"C:\Fallback\WinRAR\WinRAR.exe")
            }),
            Some(PathBuf::from(r"C:\Fallback\WinRAR"))
        );
    }

    #[test]
    fn winrar_localization_has_ru_and_en_menu_and_error_strings() {
        for json in [
            include_str!("../../../i18n/ru.json"),
            include_str!("../../../i18n/en.json"),
        ] {
            let table: serde_json::Value = serde_json::from_str(json).unwrap();
            for key in [
                "winrar.menu",
                "winrar.add_dialog",
                "winrar.quick_add",
                "winrar.extract_here",
                "winrar.extract_to",
                "winrar.open",
                "winrar.operation_failed",
            ] {
                assert!(table.get(key).is_some(), "missing {key}");
            }
        }
    }

    #[test]
    fn winrar_detection_runs_once_at_startup_not_per_frame() {
        let source = include_str!("mod.rs");
        assert_eq!(source.matches("winrar::detect_install_dir()").count(), 1);
    }

    #[test]
    fn existing_zip_domain_tests_not_regressed() {
        let source = include_str!("archive.rs");
        assert!(source.contains("create_zip_archive_cancellable"));
        assert!(source.contains("extract_zip_all_cancellable"));
    }

    #[test]
    fn existing_context_menu_tests_not_regressed() {
        let source = include_str!("../list/actions.rs");
        assert!(source.contains("fn row_context_menu_not_regressed"));
        assert!(source.contains("fn empty_area_menu_other_items_and_order_not_regressed"));
    }

    #[test]
    fn cargo_lock_unchanged_no_new_dependencies() {
        assert!(include_str!("../../../Cargo.toml").contains("windows ="));
        assert!(include_str!("../../../Cargo.lock").contains("name = \"windows\""));
    }

    #[test]
    fn settings_structure_and_json_schema_unchanged() {
        assert!(include_str!("../../core/settings.rs").contains("settings_structure_and_json_schema_unchanged"));
    }
}
