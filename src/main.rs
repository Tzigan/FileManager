#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod platform;
mod ui;

use anyhow::Context;
use platform::AcquireResult;

fn main() -> anyhow::Result<()> {
    let dirs = core::paths::discover().context("failed to resolve app directories")?;
    std::fs::create_dir_all(&dirs.log_dir)?;
    let _log_guard = core::logging::init(&dirs.log_dir);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        portable = dirs.portable,
        config_dir = %dirs.config_dir.display(),
        "FileManager starting"
    );

    // CLI args: positional path, `--folder <dir>`, `--select <file>`.
    // Explorer's context menu passes "%1" positionally.
    let (initial_path, initial_select) =
        core::cli::parse_args(&std::env::args().skip(1).collect::<Vec<_>>());
    {
    }

    // Single instance: a second launch forwards its path over a named pipe
    // instead of showing a dialog.
    let _instance = match platform::acquire_single_instance() {
        Ok(AcquireResult::Acquired(guard)) => guard,
        Ok(AcquireResult::AlreadyRunning) => {
            tracing::info!("another instance is already running");
            if let Some(p) = &initial_path {
                match platform::pipe_send(p) {
                    Ok(()) => tracing::info!(path = %p.display(), "path forwarded to running instance"),
                    Err(e) => tracing::warn!("pipe_send failed: {e}"),
                }
            } else {
                platform::notify_already_running();
            }
            return Ok(());
        }
        Err(e) => {
            tracing::warn!("single-instance check failed, continuing anyway: {e}");
            platform::SingleInstanceGuard::noop()
        }
    };

    let store = core::settings::SettingsStore::load(&dirs.config_dir);
    let i18n = core::i18n::I18n::new(store.settings.language);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("FileManager")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([640.0, 420.0]);
    match eframe::icon_data::from_png_bytes(include_bytes!("../resources/app.png")) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => tracing::warn!("failed to decode window icon: {e}"),
    }

    let options = eframe::NativeOptions {
        viewport,
        persist_window: true,
        ..Default::default()
    };

    let result = eframe::run_native(
        "FileManager",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::FileManagerApp::new(cc, store, i18n, dirs, initial_path, initial_select)))
        }),
    );

    if let Err(e) = &result {
        tracing::error!("eframe exited with error: {e}");
    }
    tracing::info!("FileManager exiting");
    result.map_err(|e| anyhow::anyhow!("{e}"))
}


#[cfg(test)]
mod tests {
    #[test]
    fn windows_gui_subsystem() {
        let src = include_str!("main.rs");
        assert!(src.contains("windows_subsystem = \"windows\""));
    }

    #[test]
    fn manifest_declares_awareness() {
        let m = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "\\app.manifest")).unwrap();
        assert!(m.contains("longPathAware"));
        assert!(m.contains("PerMonitorV2") || m.contains("dpiAwareness"));
    }

    #[test]
    fn icon_resource_exists() {
        let rc = concat!(env!("CARGO_MANIFEST_DIR"), "\\app.rc");
        assert!(std::path::Path::new(rc).exists());
        let c = std::fs::read_to_string(rc).unwrap();
        assert!(c.to_lowercase().contains("icon") || c.contains(".ico"));
    }
}
