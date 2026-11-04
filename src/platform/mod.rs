#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{acquire_single_instance, notify_already_running, AcquireResult, SingleInstanceGuard};

/// Stubs for non-Windows hosts so the crate still compiles for docs/tests.
#[cfg(not(windows))]
mod stubs {
    pub enum AcquireResult {
        Acquired(SingleInstanceGuard),
        AlreadyRunning,
    }

    pub struct SingleInstanceGuard;

    impl SingleInstanceGuard {
        pub fn noop() -> Self {
            Self
        }
    }

    pub fn acquire_single_instance() -> anyhow::Result<AcquireResult> {
        Ok(AcquireResult::Acquired(SingleInstanceGuard))
    }

    pub fn notify_already_running() {}
}

#[cfg(not(windows))]
pub use stubs::{acquire_single_instance, notify_already_running, AcquireResult, SingleInstanceGuard};
