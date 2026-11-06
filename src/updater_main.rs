#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
#[allow(dead_code)]
#[path = "platform/windows/update_support.rs"]
mod update_support;
#[cfg(windows)]
#[path = "platform/windows/updater.rs"]
mod updater;

#[cfg(windows)]
fn main() {
    if let Err(error) = updater::run() {
        updater::show_error(&format!("{error:#}"));
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}
