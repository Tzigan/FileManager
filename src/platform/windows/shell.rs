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

/// Opens Windows Terminal at the given directory.
pub fn open_terminal(path: &Path) -> io::Result<()> {
    std::process::Command::new(crate::system_tools::windows_terminal_executable()?)
        .arg("-d")
        .arg(path)
        .spawn()
        .map(|_| ())
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

/// Toggles FILE_ATTRIBUTE_READONLY / FILE_ATTRIBUTE_HIDDEN, preserving other flags.
pub fn set_attributes(path: &std::path::Path, readonly: bool, hidden: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_FLAGS_AND_ATTRIBUTES,
    };
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let cur = GetFileAttributesW(windows::core::PCWSTR(wide.as_ptr()));
        if cur == u32::MAX {
            return Err(std::io::Error::last_os_error());
        }
        let mut attrs = cur & !(FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_READONLY.0);
        if readonly {
            attrs |= FILE_ATTRIBUTE_READONLY.0;
        }
        if hidden {
            attrs |= FILE_ATTRIBUTE_HIDDEN.0;
        }
        SetFileAttributesW(
            windows::core::PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(attrs),
        )
        .map_err(|_| std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::MetadataExt;

    #[test]
    fn property_attribute_toggles_preserve_other_file_attributes() {
        let dir = std::env::temp_dir().join(format!("fm-attrs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.txt");
        std::fs::write(&path, b"attributes").unwrap();

        let result = (|| {
            let before = std::fs::metadata(&path).unwrap().file_attributes();
            set_attributes(&path, true, true).unwrap();
            let changed = std::fs::metadata(&path).unwrap().file_attributes();
            assert_ne!(changed & 0x1, 0);
            assert_ne!(changed & 0x2, 0);
            assert_eq!(changed & !0x3, before & !0x3);

            set_attributes(&path, false, false).unwrap();
            let restored = std::fs::metadata(&path).unwrap().file_attributes();
            assert_eq!(restored & 0x3, 0);
            assert_eq!(restored & !0x3, before & !0x3);
        })();
        // Always clear read-only/hidden before removing the test fixture.
        let _ = set_attributes(&path, false, false);
        std::fs::remove_dir_all(dir).unwrap();
        result
    }

    #[test]
    fn changing_attributes_for_missing_file_returns_an_error() {
        let path = std::env::temp_dir().join(format!("fm-no-such-{}", std::process::id()));
        assert!(set_attributes(&path, true, true).is_err());
    }
}
