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

    // Single instance guard (stub): second launch shows a message and exits.
    // Path forwarding to the running instance is implemented in v0.5.
    let _instance = match platform::acquire_single_instance() {
        Ok(AcquireResult::Acquired(guard)) => guard,
        Ok(AcquireResult::AlreadyRunning) => {
            tracing::info!("another instance is already running, exiting");
            platform::notify_already_running();
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
        Box::new(move |cc| Ok(Box::new(ui::FileManagerApp::new(cc, store, i18n, dirs)))),
    );

    if let Err(e) = &result {
        tracing::error!("eframe exited with error: {e}");
    }
    tracing::info!("FileManager exiting");
    result.map_err(|e| anyhow::anyhow!("{e}"))
}
