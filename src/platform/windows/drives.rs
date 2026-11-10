use std::path::{Path, PathBuf};

use ::windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
use ::windows::core::HSTRING;

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;

/// Lists available drive roots (`C:\`, `D:\`, …) via `GetLogicalDrives`.
pub fn drives() -> Vec<PathBuf> {
    let mask = unsafe { GetLogicalDrives() };
    (0u32..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        .collect()
}

/// Локальными считаются фиксированные, съёмные, оптические и RAM-тома.
pub fn is_local_drive(path: &Path) -> bool {
    let drive_type = unsafe { GetDriveTypeW(&HSTRING::from(path.as_os_str())) };
    matches!(
        drive_type,
        DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_CDROM | DRIVE_RAMDISK
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_drives() {
        let d = drives();
        assert!(!d.is_empty());
        assert!(d.iter().all(|p| p.to_string_lossy().ends_with(":\\")));
    }
}
