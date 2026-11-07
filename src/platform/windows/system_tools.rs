use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

pub fn system_executable(name: &str) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length >= buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows system directory path is too long",
        ));
    }

    buffer.truncate(length);
    let path = PathBuf::from(OsString::from_wide(&buffer)).join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("system executable not found: {}", path.display()),
        ))
    }
}

pub fn windows_terminal_executable() -> io::Result<PathBuf> {
    let local_data = directories::BaseDirs::new()
        .map(|dirs| dirs.data_local_dir().to_path_buf())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "local app data is unavailable"))?;
    let path = local_data
        .join("Microsoft")
        .join("WindowsApps")
        .join("wt.exe");
    if path.is_file() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Windows Terminal alias not found: {}", path.display()),
        ))
    }
}
