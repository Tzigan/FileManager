#[derive(Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{acquire_single_instance, create_zip_archive, decode_image, drives, extract_zip_all, extract_zip_file, extract_zip_folder, format_local_time, free_space, get_clipboard_files, get_clipboard_text, install_context_menu, launch_updater, notify_already_running, open_recycle_bin, open_terminal, pipe_listen, pipe_send, prepare_update_archive, read_zip_dir_cancellable, recycle_bin, replace_file, search_zip, set_attributes, set_clipboard_files, shell_open, signal_update_started, uninstall_context_menu, AcquireResult, SingleInstanceGuard};

#[cfg(windows)]
pub(crate) use self::windows::{
    TabDragPreviewWindow, cursor_screen_position, escape_key_down, left_mouse_button_down,
    monitor_work_area_at_screen_position,
};

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

    pub fn free_space(_path: &std::path::Path) -> Option<u64> {
        None
    }
    pub fn decode_image(_path: &std::path::Path, _max_dimension: u32) -> std::io::Result<super::DecodedImage> {
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
        _destination: &std::path::Path,
    ) -> std::io::Result<std::path::PathBuf> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "ZIP creation is supported only on Windows",
        ))
    }
    pub fn recycle_bin(paths: &[std::path::PathBuf]) -> std::io::Result<()> {
        for p in paths {
            let m = std::fs::metadata(p)?;
            if m.is_dir() { std::fs::remove_dir_all(p)?; } else { std::fs::remove_file(p)?; }
        }
        Ok(())
    }
    pub fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
        std::fs::rename(source, destination)
    }
    pub fn set_clipboard_files(_p: &[std::path::PathBuf]) -> std::io::Result<()> { Ok(()) }
    pub fn get_clipboard_files() -> Vec<std::path::PathBuf> { Vec::new() }
    pub fn get_clipboard_text() -> String { String::new() }
    pub fn pipe_listen(_f: impl FnMut(std::path::PathBuf) + Send + 'static) {}
    pub fn pipe_send(_p: &std::path::Path) -> std::io::Result<()> { Ok(()) }
    pub fn install_context_menu(_e: &std::path::Path, _t: &str) -> std::io::Result<()> { Ok(()) }
    pub fn uninstall_context_menu() -> std::io::Result<()> { Ok(()) }
    pub fn shell_open(_p: &std::path::Path) -> std::io::Result<()> { Ok(()) }
    pub fn open_terminal(_p: &std::path::Path) -> std::io::Result<()> { Ok(()) }
    pub fn open_recycle_bin() -> std::io::Result<()> { Ok(()) }
    pub fn prepare_update_archive(_archive: &std::path::Path, _size: u64, _sha256: &str) -> anyhow::Result<std::path::PathBuf> {
        anyhow::bail!("automatic updates are supported only on Windows")
    }
    pub fn launch_updater(_stage: &std::path::Path, _target: &std::path::Path, _pid: u32) -> anyhow::Result<()> {
        anyhow::bail!("automatic updates are supported only on Windows")
    }
    pub fn signal_update_started() -> anyhow::Result<()> { Ok(()) }
    pub fn set_attributes(_p: &std::path::Path, _r: bool, _h: bool) -> std::io::Result<()> { Ok(()) }
}

#[cfg(not(windows))]
pub use stubs::{acquire_single_instance, create_zip_archive, decode_image, drives, extract_zip_all, extract_zip_file, extract_zip_folder, format_local_time, free_space, get_clipboard_files, get_clipboard_text, install_context_menu, launch_updater, notify_already_running, open_recycle_bin, open_terminal, pipe_listen, pipe_send, prepare_update_archive, read_zip_dir_cancellable, recycle_bin, replace_file, search_zip, set_attributes, set_clipboard_files, shell_open, signal_update_started, uninstall_context_menu, AcquireResult, SingleInstanceGuard};

#[cfg(not(windows))]
pub(crate) use stubs::{
    TabDragPreviewWindow, cursor_screen_position, escape_key_down, left_mouse_button_down,
    monitor_work_area_at_screen_position,
};

pub(crate) fn ops_platform() -> crate::core::ops::OpsPlatform {
    crate::core::ops::OpsPlatform::new(free_space, recycle_bin, replace_file)
}
