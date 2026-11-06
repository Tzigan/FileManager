use std::path::PathBuf;

use anyhow::Context;
use directories::BaseDirs;

const APP_DIR_NAME: &str = "FileManager";
const PORTABLE_MARKER: &str = "filemanager.portable";

fn is_portable(exe_dir: &std::path::Path) -> bool {
    exe_dir.join(PORTABLE_MARKER).exists() || exe_dir.join("settings.json").exists()
}

/// Resolved filesystem locations for settings, data and logs.
///
/// Portable mode is enabled when a `filemanager.portable` marker file or an
/// existing `settings.json` is found next to the executable. In that case all
/// data lives beside the `.exe`; otherwise `%LOCALAPPDATA%\FileManager` is used.
#[derive(Debug, Clone)]
pub struct AppDirs {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub portable: bool,
}

pub fn discover() -> anyhow::Result<AppDirs> {
    let exe_dir = std::env::current_exe()
        .context("failed to locate current exe")?
        .parent()
        .map(|p| p.to_path_buf())
        .context("exe has no parent directory")?;

    let portable = is_portable(&exe_dir);

    let base = if portable {
        exe_dir
    } else {
        BaseDirs::new()
            .context("failed to resolve %LOCALAPPDATA%")?
            .data_local_dir()
            .join(APP_DIR_NAME)
    };

    Ok(AppDirs {
        config_dir: base.clone(),
        data_dir: base.join("data"),
        log_dir: base.join("logs"),
        portable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("filemanager-paths-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discover_returns_dirs() {
        let dirs = discover().unwrap();
        assert!(!dirs.config_dir.as_os_str().is_empty());
        assert!(!dirs.log_dir.as_os_str().is_empty());
    }

    #[test]
    fn portable_mode_requires_marker_or_settings_file() {
        let dir = temp_dir();
        assert!(!is_portable(&dir));

        std::fs::write(dir.join(PORTABLE_MARKER), b"").unwrap();
        assert!(is_portable(&dir));
        std::fs::remove_file(dir.join(PORTABLE_MARKER)).unwrap();

        std::fs::write(dir.join("settings.json"), b"{}").unwrap();
        assert!(is_portable(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
