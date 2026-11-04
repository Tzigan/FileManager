mod single_instance;

pub use single_instance::{acquire_single_instance, notify_already_running, AcquireResult, SingleInstanceGuard};
