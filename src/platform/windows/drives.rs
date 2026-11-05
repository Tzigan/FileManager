use std::path::PathBuf;

use ::windows::Win32::Storage::FileSystem::GetLogicalDrives;

/// Lists available drive roots (`C:\`, `D:\`, …) via `GetLogicalDrives`.
pub fn drives() -> Vec<PathBuf> {
    let mask = unsafe { GetLogicalDrives() };
    (0u32..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        .collect()
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
