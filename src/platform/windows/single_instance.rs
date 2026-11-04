use ::windows::core::w;
use ::windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WIN32_ERROR,
};
use ::windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use ::windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, MB_ICONINFORMATION, MB_OK,
};

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
    let handle = unsafe { CreateMutexW(None, true, MUTEX_NAME)? };
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
