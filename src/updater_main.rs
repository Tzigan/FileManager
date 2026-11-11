#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
#[allow(dead_code)]
#[path = "platform/windows/system_tools.rs"]
mod system_tools;

#[cfg(windows)]
#[allow(dead_code)]
#[path = "platform/windows/update_support.rs"]
mod update_support;
#[cfg(windows)]
#[path = "platform/windows/updater.rs"]
mod updater;

#[cfg(windows)]
fn main() {
    let mut args = std::env::args_os().skip(1);
    if args
        .next()
        .is_some_and(|arg| arg.to_string_lossy() == "--verify-file")
    {
        let Some(path) = args.next().map(std::path::PathBuf::from) else {
            eprintln!("--verify-file requires a path");
            std::process::exit(2);
        };
        if args.next().is_some() {
            eprintln!("unexpected argument after --verify-file path");
            std::process::exit(2);
        }
        if let Err(error) = update_support::verify_release_executable(&path) {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(error) = updater::run() {
        // Пользователь видит короткую причину. Цепочка anyhow остаётся только в отладочном выводе.
        eprintln!("{error:#}");
        updater::show_error(&updater::user_facing_message(&error));
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {}
