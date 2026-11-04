use std::path::PathBuf;

use anyhow::Context;
use directories::BaseDirs;

const APP_DIR_NAME: &str = "FileManager";
const PORTABLE_MARKER: &str = "filemanager.portable";

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

    let portable = exe_dir.join(PORTABLE_MARKER).exists() || exe_dir.join("settings.json").exists();

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
