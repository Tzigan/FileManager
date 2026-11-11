#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipCompressionMethod {
    Store,
    Deflate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipCompressionLevel {
    Fast,
    Standard,
    Maximum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipExistingBehavior {
    AddSuffix,
    Replace,
    Ask,
}

#[derive(Clone, Debug)]
pub struct ZipCreateOptions {
    pub source_root: std::path::PathBuf,
    pub destination: std::path::PathBuf,
    pub archive_name: String,
    pub compression_method: ZipCompressionMethod,
    pub compression_level: ZipCompressionLevel,
    pub preserve_folder_structure: bool,
    pub existing_behavior: ZipExistingBehavior,
}

pub fn normalize_zip_archive_name(name: &str) -> Result<String, std::io::Error> {
    let name = name.trim();
    let has_zip_extension = name
        .get(name.len().saturating_sub(4)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".zip"));
    let stem = if has_zip_extension {
        &name[..name.len() - 4]
    } else {
        name
    };
    if !valid_zip_archive_stem(stem) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid ZIP archive filename",
        ));
    }
    Ok(format!("{stem}.zip"))
}

fn valid_zip_archive_stem(stem: &str) -> bool {
    if stem.is_empty()
        || stem == "."
        || stem == ".."
        || stem.ends_with('.')
        || stem.ends_with(' ')
        || stem.chars().any(|ch| {
            ch.is_control() || matches!(ch, '<' | '>' | '"' | '|' | '?' | '*' | '/' | '\\' | ':')
        })
    {
        return false;
    }

    let device = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(|ch| ch == '.' || ch == ' ')
        .to_ascii_uppercase();
    !matches!(
        device.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) && !reserved_zip_numbered_device(&device, "COM")
        && !reserved_zip_numbered_device(&device, "LPT")
}

fn reserved_zip_numbered_device(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix).is_some_and(|suffix| {
        matches!(
            suffix,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

#[derive(Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskUsage {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

impl DiskUsage {
    pub fn used_percent(self) -> Option<u8> {
        let used = self.total_bytes.checked_sub(self.free_bytes)?;
        if self.total_bytes == 0 {
            return None;
        }
        let percent =
            (used as u128 * 100 + self.total_bytes as u128 / 2) / self.total_bytes as u128;
        Some(percent.min(100) as u8)
    }

    /// Доля занятости без округления. `None` — ёмкость неизвестна, это не ноль.
    pub fn used_ratio(self) -> Option<f32> {
        let used = self.total_bytes.checked_sub(self.free_bytes)?;
        if self.total_bytes == 0 {
            return None;
        }
        Some((used as f32 / self.total_bytes as f32).clamp(0.0, 1.0))
    }
}

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{
    AcquireResult, SingleInstanceGuard, acquire_single_instance, apply_stored_attributes,
    create_zip_archive, decode_image, disk_usage, drives, extract_zip_all, extract_zip_file,
    extract_zip_folder, format_local_time, free_space, get_clipboard_files, get_clipboard_text,
    install_context_menu, launch_updater, notify_already_running, open_recycle_bin, open_terminal,
    pick_folder, pipe_listen, pipe_send, prepare_update_archive, read_entry_details,
    read_zip_dir_cancellable, recycle_bin, replace_file, search_zip, set_attributes,
    set_clipboard_files, shell_open, signal_update_started, uninstall_context_menu, volume_id,
};

#[cfg(windows)]
pub use self::windows::{
    is_local_drive, is_wsl_path, read_wsl_dir, read_wsl_subdirs, wsl_distribution_root,
    wsl_distributions,
};

#[cfg(windows)]
pub(crate) use self::windows::{
    TabDragPreviewWindow, cursor_screen_position, escape_key_down, left_mouse_button_down,
    monitor_work_area_at_screen_position,
};

/// Таргет приложения — только Windows.
/// Новые ветки `cfg(unix)` и `cfg(not(windows))` в модулях не добавлять:
/// мёртвые заглушки «для другой ОС» уже убраны из корзины и не должны вернуться.
/// Блок ниже не описывает вторую поддерживаемую систему, он лишь не даёт
/// типам разъехаться, если файл открыли не на Windows. WSL в дереве — функция Windows.
///
/// Stubs for non-Windows hosts so the crate still compiles for docs/tests.
#[cfg(not(windows))]
mod stubs {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct TabDragPreviewWindow;

    impl TabDragPreviewWindow {
        pub(crate) fn find(_title: &str) -> Option<Self> {
            None
        }

        pub(crate) fn is_valid(&self) -> bool {
            false
        }

        pub(crate) fn native_pixels_per_point(&self) -> Option<f32> {
            None
        }

        pub(crate) fn set_position(&self, _x: i32, _y: i32) -> bool {
            false
        }

        pub(crate) fn capture_mouse(&self) -> bool {
            false
        }

        pub(crate) fn owns_mouse_capture(&self) -> bool {
            false
        }

        pub(crate) fn release_mouse_capture(&self) {}
    }

    pub(crate) fn cursor_screen_position() -> Option<(i32, i32)> {
        None
    }

    pub(crate) fn left_mouse_button_down() -> bool {
        false
    }

    pub(crate) fn escape_key_down() -> bool {
        false
    }

    pub(crate) fn monitor_work_area_at_screen_position(
        _screen_position: (i32, i32),
    ) -> Option<(i32, i32, i32, i32)> {
        None
    }

    pub enum AcquireResult {
        Acquired(SingleInstanceGuard),
        AlreadyRunning,
    }

    pub struct SingleInstanceGuard;

    impl SingleInstanceGuard {
        pub fn noop() -> Self {
            Self
        }
    }

    pub fn acquire_single_instance() -> anyhow::Result<AcquireResult> {
        Ok(AcquireResult::Acquired(SingleInstanceGuard))
    }

    pub fn notify_already_running() {}

    pub fn format_local_time(
        t: std::time::SystemTime,
        language: crate::core::i18n::Language,
    ) -> String {
        match t.duration_since(std::time::SystemTime::UNIX_EPOCH) {
            Ok(d) => crate::core::format::format_unix_utc_for_language(d.as_secs(), language),
            Err(_) => "-".to_string(),
        }
    }

    pub fn drives() -> Vec<std::path::PathBuf> {
        vec![std::path::PathBuf::from("/")]
    }

    pub fn is_local_drive(_path: &std::path::Path) -> bool {
        false
    }

    pub fn wsl_distributions() -> Vec<String> {
        Vec::new()
    }

    pub fn wsl_distribution_root(_name: &str) -> Option<std::path::PathBuf> {
        None
    }

    pub fn is_wsl_path(_path: &std::path::Path) -> bool {
        false
    }

    pub fn read_wsl_dir(
        _path: &std::path::Path,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Vec<crate::core::fs::FileEntry>, crate::core::fs::ReadDirError> {
        Err(crate::core::fs::ReadDirError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "WSL browsing is supported only on Windows",
        )))
    }

    pub fn read_wsl_subdirs(_path: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "WSL browsing is supported only on Windows",
        ))
    }

    pub fn free_space(_path: &std::path::Path) -> Option<u64> {
        None
    }
    pub fn disk_usage(_path: &std::path::Path) -> Option<super::DiskUsage> {
        None
    }
    pub fn decode_image(
        _path: &std::path::Path,
        _max_dimension: u32,
    ) -> std::io::Result<super::DecodedImage> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "image previews are supported only on Windows",
        ))
    }
    pub fn read_zip_dir_cancellable(
        _archive: &std::path::Path,
        _prefix: &str,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Vec<crate::core::fs::FileEntry>, crate::core::fs::ReadDirError> {
        Err(crate::core::fs::ReadDirError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ZIP browsing is supported only on Windows",
        )))
    }
    pub fn search_zip(
        _archive: &std::path::Path,
        _prefix: &str,
        _options: &crate::core::search::SearchOptions,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Vec<crate::core::fs::FileEntry>, crate::core::fs::ReadDirError> {
        Err(crate::core::fs::ReadDirError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ZIP search is supported only on Windows",
        )))
    }
    pub fn extract_zip_file(
        _archive: &std::path::Path,
        _member: &str,
        _destination: &std::path::Path,
    ) -> std::io::Result<std::path::PathBuf> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ZIP extraction is supported only on Windows",
        ))
    }
    pub fn extract_zip_folder(
        _archive: &std::path::Path,
        _folder: &str,
        _destination: &std::path::Path,
    ) -> std::io::Result<std::path::PathBuf> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ZIP extraction is supported only on Windows",
        ))
    }
    pub fn extract_zip_all(
        _archive: &std::path::Path,
        _destination: &std::path::Path,
    ) -> std::io::Result<std::path::PathBuf> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ZIP extraction is supported only on Windows",
        ))
    }
    pub fn create_zip_archive(
        _sources: &[std::path::PathBuf],
        _options: &super::ZipCreateOptions,
    ) -> std::io::Result<std::path::PathBuf> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ZIP creation is supported only on Windows",
        ))
    }
    pub fn recycle_bin(paths: &[std::path::PathBuf]) -> std::io::Result<()> {
        for p in paths {
            let m = std::fs::metadata(p)?;
            if m.is_dir() {
                std::fs::remove_dir_all(p)?;
            } else {
                std::fs::remove_file(p)?;
            }
        }
        Ok(())
    }
    pub fn replace_file(
        source: &std::path::Path,
        destination: &std::path::Path,
    ) -> std::io::Result<()> {
        std::fs::rename(source, destination)
    }
    pub fn set_clipboard_files(_p: &[std::path::PathBuf]) -> std::io::Result<()> {
        Ok(())
    }
    pub fn get_clipboard_files() -> Vec<std::path::PathBuf> {
        Vec::new()
    }
    pub fn get_clipboard_text() -> String {
        String::new()
    }
    pub fn pipe_listen(_f: impl FnMut(std::path::PathBuf) + Send + 'static) {}
    pub fn pipe_send(_p: &std::path::Path) -> std::io::Result<()> {
        Ok(())
    }
    pub fn install_context_menu(_e: &std::path::Path, _t: &str) -> std::io::Result<()> {
        Ok(())
    }
    pub fn uninstall_context_menu() -> std::io::Result<()> {
        Ok(())
    }
    pub fn shell_open(_p: &std::path::Path) -> std::io::Result<()> {
        Ok(())
    }
    pub fn pick_folder(
        _title: &str,
        _initial_dir: Option<&std::path::Path>,
    ) -> std::io::Result<Option<std::path::PathBuf>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "folder selection is supported only on Windows",
        ))
    }
    pub fn open_terminal(_p: &std::path::Path) -> std::io::Result<()> {
        Ok(())
    }
    pub fn open_recycle_bin() -> std::io::Result<()> {
        Ok(())
    }
    pub fn prepare_update_archive(
        _archive: &std::path::Path,
        _size: u64,
        _sha256: &str,
    ) -> anyhow::Result<std::path::PathBuf> {
        anyhow::bail!("automatic updates are supported only on Windows")
    }
    pub fn launch_updater(
        _stage: &std::path::Path,
        _target: &std::path::Path,
        _pid: u32,
        _auto_restart: bool,
        _russian: bool,
    ) -> anyhow::Result<()> {
        anyhow::bail!("automatic updates are supported only on Windows")
    }
    pub fn signal_update_started() -> anyhow::Result<()> {
        Ok(())
    }
    pub fn set_attributes(_p: &std::path::Path, _r: bool, _h: bool) -> std::io::Result<()> {
        Ok(())
    }
    pub fn read_entry_details(_path: &std::path::Path) -> crate::core::entry_details::EntryDetails {
        crate::core::entry_details::EntryDetails::unavailable()
    }
    pub fn volume_id(_path: &std::path::Path) -> Option<String> {
        None
    }
}

#[cfg(not(windows))]
pub use stubs::{
    AcquireResult, SingleInstanceGuard, acquire_single_instance, create_zip_archive, decode_image,
    disk_usage, drives, extract_zip_all, extract_zip_file, extract_zip_folder, format_local_time,
    free_space, get_clipboard_files, get_clipboard_text, install_context_menu, launch_updater,
    notify_already_running, open_recycle_bin, open_terminal, pick_folder, pipe_listen, pipe_send,
    prepare_update_archive, read_entry_details, read_zip_dir_cancellable, recycle_bin,
    replace_file, search_zip, set_attributes, set_clipboard_files, shell_open,
    signal_update_started, uninstall_context_menu, volume_id,
};

#[cfg(not(windows))]
pub use stubs::{
    is_local_drive, is_wsl_path, read_wsl_dir, read_wsl_subdirs, wsl_distribution_root,
    wsl_distributions,
};

#[cfg(not(windows))]
pub(crate) use stubs::{
    TabDragPreviewWindow, cursor_screen_position, escape_key_down, left_mouse_button_down,
    monitor_work_area_at_screen_position,
};

pub(crate) fn ops_platform() -> crate::core::ops::OpsPlatform {
    crate::core::ops::OpsPlatform::new(free_space, recycle_bin, replace_file)
}

#[cfg(test)]
mod zip_name_tests {
    use super::normalize_zip_archive_name;

    #[test]
    fn normalizes_extension_and_rejects_invalid_archive_names() {
        assert_eq!(normalize_zip_archive_name("report").unwrap(), "report.zip");
        assert_eq!(
            normalize_zip_archive_name("report.ZIP").unwrap(),
            "report.zip"
        );
        for name in ["", " ", "../report", "C:report", "CON", "bad?name"] {
            assert!(normalize_zip_archive_name(name).is_err(), "{name}");
        }
    }
}

#[cfg(test)]
mod disk_usage_tests {
    use super::DiskUsage;

    #[test]
    fn usage_percent_rounds_and_rejects_unknown_or_inconsistent_capacity() {
        assert_eq!(
            DiskUsage {
                total_bytes: 100,
                free_bytes: 58,
            }
            .used_percent(),
            Some(42)
        );
        assert_eq!(
            DiskUsage {
                total_bytes: 3,
                free_bytes: 2,
            }
            .used_percent(),
            Some(33)
        );
        assert_eq!(
            DiskUsage {
                total_bytes: 0,
                free_bytes: 0,
            }
            .used_percent(),
            None
        );
        assert_eq!(
            DiskUsage {
                total_bytes: 10,
                free_bytes: 11,
            }
            .used_percent(),
            None
        );
        assert_eq!(
            DiskUsage {
                total_bytes: 0,
                free_bytes: 0,
            }
            .used_ratio(),
            None
        );
        let ratio = DiskUsage {
            total_bytes: 1000,
            free_bytes: 566,
        }
        .used_ratio()
        .unwrap();
        assert!((ratio - 0.434).abs() < 0.0001);
        assert_eq!(
            DiskUsage {
                total_bytes: 1000,
                free_bytes: 566,
            }
            .used_percent(),
            Some(43)
        );
    }
}
