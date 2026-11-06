use std::path::{Path, PathBuf};

use ::windows::core::HSTRING;
use ::windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// Free bytes available to the user on the volume containing `path`,
/// or `None` when it can't be determined (e.g. exotic UNC paths).
pub fn free_space(path: &Path) -> Option<u64> {
    // Walk up to an existing ancestor — GetDiskFreeSpaceEx needs a real dir.
    let mut p: Option<PathBuf> = Some(path.to_path_buf());
    let root = loop {
        match p {
            Some(cand) if cand.exists() => break cand,
            Some(cand) => p = cand.parent().map(|x| x.to_path_buf()),
            None => return None,
        }
    };
    let mut free: u64 = 0;
    unsafe {
        GetDiskFreeSpaceExW(&HSTRING::from(root.as_os_str()), None, Some(&mut free), None).ok()?;
    }
    Some(free)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
