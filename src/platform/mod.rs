#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{acquire_single_instance, drives, format_local_time, free_space, get_clipboard_files, get_clipboard_text, install_context_menu, notify_already_running, open_recycle_bin, open_terminal, pipe_listen, pipe_send, recycle_bin, set_attributes, set_clipboard_files, shell_open, uninstall_context_menu, AcquireResult, SingleInstanceGuard};

/// Stubs for non-Windows hosts so the crate still compiles for docs/tests.
#[cfg(not(windows))]
mod stubs {
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
    pub fn recycle_bin(paths: &[std::path::PathBuf]) -> std::io::Result<()> {
        for p in paths {
            let m = std::fs::metadata(p)?;
            if m.is_dir() { std::fs::remove_dir_all(p)?; } else { std::fs::remove_file(p)?; }
        }
        Ok(())
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
    pub fn set_attributes(_p: &std::path::Path, _r: bool, _h: bool) -> std::io::Result<()> { Ok(()) }
}

#[cfg(not(windows))]
pub use stubs::{acquire_single_instance, drives, format_local_time, free_space, get_clipboard_files, get_clipboard_text, install_context_menu, notify_already_running, open_recycle_bin, open_terminal, pipe_listen, pipe_send, recycle_bin, set_attributes, set_clipboard_files, shell_open, uninstall_context_menu, AcquireResult, SingleInstanceGuard};
