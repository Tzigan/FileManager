mod args_pipe;
mod clipboard;
mod context_menu;
mod dir_watch;
mod drives;
mod entry_details;
mod file_ops;
mod image_decode;
mod recycle;
mod shell;
mod single_instance;
mod space;
mod tab_drag;
mod time_fmt;
mod update_support;
mod wsl;
mod zip;

pub use args_pipe::{listen as pipe_listen, send as pipe_send};
pub use clipboard::{get_clipboard_files, get_clipboard_text, set_clipboard_files};
pub use context_menu::{install_context_menu, uninstall_context_menu};
pub use dir_watch::spawn_dir_watch;
pub use drives::{drives, is_local_drive};
pub use entry_details::read_entry_details;
pub use file_ops::replace_file;
pub use image_decode::decode_image;
pub use recycle::recycle_bin;
pub use shell::{
    apply_stored_attributes, open_recycle_bin, open_terminal, pick_folder, set_attributes,
    shell_open,
};
pub use single_instance::{
    AcquireResult, SingleInstanceGuard, acquire_single_instance, notify_already_running,
};
#[cfg(not(debug_assertions))]
pub use single_instance::notify_panic;
pub use space::{disk_usage, free_space, volume_id};
pub(crate) use tab_drag::{
    TabDragPreviewWindow, cursor_screen_position, escape_key_down, left_mouse_button_down,
    monitor_work_area_at_screen_position,
};
pub use time_fmt::format_local_time;
pub use update_support::{launch_updater, prepare_update_archive, signal_update_started};
pub use wsl::{
    distribution_root as wsl_distribution_root, distributions as wsl_distributions, is_wsl_path,
    read_dir as read_wsl_dir, read_subdirs as read_wsl_subdirs,
};
pub use zip::{
    create_zip_archive_cancellable, extract_zip_all_cancellable, extract_zip_file_cancellable,
    extract_zip_folder_cancellable, read_zip_dir_cancellable, search_zip,
};
#[allow(unused_imports)]
pub use zip::{create_zip_archive, extract_zip_all, extract_zip_file, extract_zip_folder};
