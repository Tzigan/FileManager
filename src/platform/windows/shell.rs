use std::io;
use std::path::Path;

use ::windows::core::{HSTRING, PCWSTR};
use ::windows::Win32::UI::Shell::ShellExecuteW;
use ::windows::Win32::UI::WindowsAndMessaging::SW_NORMAL;

/// Opens `path` with its default Windows application (ShellExecute "open").
pub fn shell_open(path: &Path) -> io::Result<()> {
    unsafe {
        let file = HSTRING::from(path.as_os_str());
        let r = ShellExecuteW(None, &HSTRING::from("open"), &file, PCWSTR::null(), PCWSTR::null(), SW_NORMAL);
        if r.0 as isize > 32 {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::Other, format!("ShellExecute failed ({})", r.0 as isize)))
        }
    }
}

/// Opens the Windows Recycle Bin in Explorer.
pub fn open_recycle_bin() -> io::Result<()> {
    unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from("explorer.exe"),
            &HSTRING::from("shell:RecycleBinFolder"),
            PCWSTR::null(),
            SW_NORMAL,
        );
    }
    Ok(())
}
