use std::io;
use std::path::{Path, PathBuf};

use ::windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use ::windows::Win32::Storage::FileSystem::MoveFileW;
use ::windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
    IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH, ShellExecuteW,
};
use ::windows::Win32::UI::WindowsAndMessaging::SW_NORMAL;
use ::windows::core::{HSTRING, PCWSTR};

/// Системный выбор папки. `Ok(None)` — пользователь закрыл окно, это не ошибка.
/// Само извлечение после выбора выполняется отдельно, чтобы не держать интерфейс.
pub fn pick_folder(title: &str, initial_dir: Option<&Path>) -> io::Result<Option<PathBuf>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).map_err(com_error)?;
        let options = dialog.GetOptions().map_err(com_error)?;
        // Штатное окно Windows само соблюдает масштаб экрана и умеет создать новую папку.
        dialog
            .SetOptions(::windows::Win32::UI::Shell::FILEOPENDIALOGOPTIONS(
                options.0 | FOS_PICKFOLDERS.0 | FOS_FORCEFILESYSTEM.0 | FOS_PATHMUSTEXIST.0,
            ))
            .map_err(com_error)?;
        dialog.SetTitle(&HSTRING::from(title)).map_err(com_error)?;
        if let Some(initial_dir) = initial_dir.filter(|path| path.is_dir()) {
            let name = HSTRING::from(initial_dir.as_os_str());
            if let Ok(folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(&name, None) {
                let _ = dialog.SetFolder(&folder);
            }
        }
        match dialog.Show(None) {
            Ok(()) => {}
            Err(error) if is_dialog_cancel(&error) => return Ok(None),
            Err(error) => return Err(com_error(error)),
        }
        let item = dialog.GetResult().map_err(com_error)?;
        let display_name = item.GetDisplayName(SIGDN_FILESYSPATH).map_err(com_error)?;
        let path = display_name.to_string().map_err(|error| {
            CoTaskMemFree(Some(display_name.0.cast()));
            io::Error::other(error.to_string())
        })?;
        CoTaskMemFree(Some(display_name.0.cast()));
        let path = PathBuf::from(path);
        if !path.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "destination folder is unavailable",
            ));
        }
        Ok(Some(path))
    }
}

/// Перемещает temp-файл без замены уже существующего результата.
pub fn publish_file_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source = HSTRING::from(source.as_os_str());
    let destination = HSTRING::from(destination.as_os_str());
    unsafe { MoveFileW(&source, &destination).map_err(com_error) }
}

fn is_dialog_cancel(error: &windows::core::Error) -> bool {
    // ERROR_CANCELLED, обёрнутый в HRESULT.
    error.code().0 == 0x8007_04C7u32 as i32
}

fn com_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0;
    if code & 0xffff_0000u32 as i32 == 0x8007_0000u32 as i32 {
        io::Error::from_raw_os_error((code & 0xffff) as i32)
    } else {
        io::Error::other(error.message())
    }
}

/// Opens `path` with its default Windows application (ShellExecute "open").
pub fn shell_open(path: &Path) -> io::Result<()> {
    unsafe {
        let file = HSTRING::from(path.as_os_str());
        let r = ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &file,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_NORMAL,
        );
        if r.0 as isize > 32 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("ShellExecute failed ({})", r.0 as isize),
            ))
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
    let result = unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from("explorer.exe"),
            &HSTRING::from("shell:RecycleBinFolder"),
            PCWSTR::null(),
            SW_NORMAL,
        )
    };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "could not open the Recycle Bin shell item",
        ))
    }
}

/// Toggles FILE_ATTRIBUTE_READONLY / FILE_ATTRIBUTE_HIDDEN, preserving other flags.
pub fn set_attributes(path: &std::path::Path, readonly: bool, hidden: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_FLAGS_AND_ATTRIBUTES,
        GetFileAttributesW, SetFileAttributesW,
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

/// Возвращает сохранённые readonly/hidden/system, не затирая остальные флаги файла.
/// Ноль означает, что карточка атрибутов не содержала — вызывающий код тогда не зовёт эту функцию.
pub fn apply_stored_attributes(path: &std::path::Path, stored: u32) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
        FILE_FLAGS_AND_ATTRIBUTES, GetFileAttributesW, SetFileAttributesW,
    };
    let mask = FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let current = GetFileAttributesW(windows::core::PCWSTR(wide.as_ptr()));
        if current == u32::MAX {
            return Err(std::io::Error::last_os_error());
        }
        let attrs = (current & !mask) | (stored & mask);
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
