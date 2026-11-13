use ::windows::core::{w, PCWSTR};
#[cfg(any(test, not(debug_assertions)))]
use ::windows::core::HSTRING;
use ::windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WIN32_ERROR,
};
use ::windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use ::windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, MB_ICONINFORMATION, MB_OK,
};
#[cfg(not(debug_assertions))]
use ::windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR;

const MUTEX_NAME: ::windows::core::PCWSTR = w!("Local\\FileManager.SingleInstance");

/// Keeps the single-instance mutex alive until the process exits.
pub struct SingleInstanceGuard {
    handle: HANDLE,
}

impl SingleInstanceGuard {
    /// Placeholder used when the OS check fails; dropping it is a no-op.
    pub fn noop() -> Self {
        Self {
            handle: HANDLE::default(),
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = ReleaseMutex(self.handle);
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

pub enum AcquireResult {
    Acquired(SingleInstanceGuard),
    AlreadyRunning,
}

/// Attempts to create the named single-instance mutex.
///
/// `AlreadyRunning` means another process owns the mutex. For v0.1 the caller
/// just shows a message and exits; forwarding the requested path to the running
/// instance over an IPC channel is planned for v0.5.
pub fn acquire_single_instance() -> anyhow::Result<AcquireResult> {
    acquire_named_instance(MUTEX_NAME)
}

fn acquire_named_instance(name: PCWSTR) -> anyhow::Result<AcquireResult> {
    let handle = unsafe { CreateMutexW(None, true, name)? };
    if unsafe { GetLastError() } == WIN32_ERROR(ERROR_ALREADY_EXISTS.0) {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Ok(AcquireResult::AlreadyRunning);
    }
    Ok(AcquireResult::Acquired(SingleInstanceGuard { handle }))
}

/// Minimal stub UI for the "already running" case.
pub fn notify_already_running() {
    unsafe {
        let _: MESSAGEBOX_RESULT = MessageBoxW(
            None,
            w!("FileManager is already running."),
            w!("FileManager"),
            MESSAGEBOX_STYLE(MB_OK.0 | MB_ICONINFORMATION.0),
        );
    }
}

#[cfg(not(debug_assertions))]
pub fn notify_panic(title: &str, message: &str) {
    let title = HSTRING::from(title);
    let message = HSTRING::from(message);
    unsafe {
        let _: MESSAGEBOX_RESULT = MessageBoxW(
            None,
            &message,
            &title,
            MESSAGEBOX_STYLE(MB_OK.0 | MB_ICONERROR.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_instance_guard_blocks_a_second_owner_and_releases_on_drop() {
        let name = HSTRING::from(format!(
            "Local\\FileManager.Test.{}.{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let first = acquire_named_instance(PCWSTR(name.as_ptr())).unwrap();
        assert!(matches!(&first, AcquireResult::Acquired(_)));

        let second = acquire_named_instance(PCWSTR(name.as_ptr())).unwrap();
        assert!(matches!(second, AcquireResult::AlreadyRunning));
        drop(first);

        let third = acquire_named_instance(PCWSTR(name.as_ptr())).unwrap();
        assert!(matches!(&third, AcquireResult::Acquired(_)));
    }
}
