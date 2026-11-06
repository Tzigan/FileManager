mod args_pipe;
mod clipboard;
mod context_menu;
mod drives;
mod recycle;
mod shell;
mod single_instance;
mod space;
mod time_fmt;

pub use args_pipe::{listen as pipe_listen, send as pipe_send};
pub use clipboard::{get_clipboard_files, get_clipboard_text, set_clipboard_files};
pub use context_menu::{install_context_menu, uninstall_context_menu};
pub use drives::drives;
pub use recycle::recycle_bin;
pub use shell::{open_recycle_bin, open_terminal, set_attributes, shell_open};
pub use space::free_space;
pub use single_instance::{acquire_single_instance, notify_already_running, AcquireResult, SingleInstanceGuard};
pub use time_fmt::format_local_time;
