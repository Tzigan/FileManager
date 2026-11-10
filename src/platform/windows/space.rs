use std::path::{Path, PathBuf};

use ::windows::core::HSTRING;
use ::windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW};

use crate::platform::DiskUsage;

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;

/// Free bytes available to the user on the volume containing `path`,
/// or `None` when it can't be determined (e.g. exotic UNC paths).
pub fn free_space(path: &Path) -> Option<u64> {
    let root = existing_volume_path(path)?;
    let mut free: u64 = 0;
    unsafe {
        GetDiskFreeSpaceExW(&HSTRING::from(root.as_os_str()), None, Some(&mut free), None).ok()?;
    }
    Some(free)
}

pub fn disk_usage(path: &Path) -> Option<DiskUsage> {
    let root = existing_volume_path(path)?;
    let drive_type = unsafe { GetDriveTypeW(&HSTRING::from(root.as_os_str())) };
    // CD-ROM и неизвестные тома не имеют полезной для пользователя ёмкости.
    if !drive_type_has_usage(drive_type) {
        return None;
    }

    let mut total = 0u64;
    let mut total_free = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(
            &HSTRING::from(root.as_os_str()),
            None,
            Some(&mut total),
            Some(&mut total_free),
        )
        .ok()?;
    }
    if total == 0 || total_free > total {
        return None;
    }
    Some(DiskUsage {
        total_bytes: total,
        free_bytes: total_free,
    })
}

fn drive_type_has_usage(drive_type: u32) -> bool {
    match drive_type {
        DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_REMOTE | DRIVE_RAMDISK => true,
        DRIVE_CDROM => false,
        _ => false,
    }
}

fn existing_volume_path(path: &Path) -> Option<PathBuf> {
    let mut candidate = Some(path.to_path_buf());
    loop {
        match candidate {
            Some(path) if path.exists() => return Some(path),
            Some(path) => candidate = path.parent().map(Path::to_path_buf),
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn only_storage_volumes_with_capacity_can_show_usage() {
        assert!(drive_type_has_usage(DRIVE_FIXED));
        assert!(drive_type_has_usage(DRIVE_REMOVABLE));
        assert!(drive_type_has_usage(DRIVE_REMOTE));
        assert!(!drive_type_has_usage(DRIVE_CDROM));
        assert!(!drive_type_has_usage(0));
        assert!(!drive_type_has_usage(1));
    }

    #[test]
    fn free_space_positive() {
        let f = free_space(Path::new("C:\\"));
        assert!(f.map(|v| v > 0).unwrap_or(false));
    }

    #[test]
    fn free_space_uses_an_existing_parent_for_a_new_destination() {
        let base = std::env::temp_dir().join(format!("fm-space-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let future = base.join("not-created").join("destination");
        assert!(free_space(&future).is_some_and(|bytes| bytes > 0));
        std::fs::remove_dir_all(base).unwrap();
    }
}
