mod args_pipe;
mod clipboard;
mod context_menu;
mod drives;
mod file_ops;
mod image_decode;
mod recycle;
mod shell;
mod single_instance;
mod space;
mod tab_drag;
mod time_fmt;
mod update_support;
mod zip;

pub use args_pipe::{listen as pipe_listen, send as pipe_send};
pub use clipboard::{get_clipboard_files, get_clipboard_text, set_clipboard_files};
pub use context_menu::{install_context_menu, uninstall_context_menu};
pub use drives::drives;
pub use file_ops::replace_file;
pub use image_decode::decode_image;
pub use recycle::recycle_bin;
pub use shell::{open_recycle_bin, open_terminal, set_attributes, shell_open};
pub use space::{disk_usage, free_space};
pub use single_instance::{acquire_single_instance, notify_already_running, AcquireResult, SingleInstanceGuard};
pub(crate) use tab_drag::{
    TabDragPreviewWindow, cursor_screen_position, escape_key_down, left_mouse_button_down,
    monitor_work_area_at_screen_position,
};
pub use time_fmt::format_local_time;
pub use update_support::{launch_updater, prepare_update_archive, signal_update_started};
pub use zip::{
    create_zip_archive, extract_zip_all, extract_zip_file, extract_zip_folder,
    read_zip_dir_cancellable, search_zip,
};
