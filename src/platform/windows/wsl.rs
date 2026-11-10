use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use ::windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use ::windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey,
    RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};
use ::windows::core::{HSTRING, PWSTR};

use crate::core::fs::{FileEntry, ReadDirError};

const LXSS_REGISTRY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Lxss";
const LOCALHOST_PREFIX: &str = "\\\\wsl.localhost\\";
const LEGACY_PREFIX: &str = "\\\\wsl$\\";
const MAX_REGISTRY_STRING_BYTES: u32 = 4096;
const MAX_WSL_DISTRIBUTIONS: u32 = 256;

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// Читает имена зарегистрированных дистрибутивов из HKCU, не запуская WSL.
pub fn distributions() -> Vec<String> {
    let mut root = HKEY::default();
    let root_name = HSTRING::from(LXSS_REGISTRY_KEY);
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &root_name, None, KEY_READ, &mut root) };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    let root = RegistryKey(root);
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    append_distribution_names(root.0, true, &mut names, &mut seen);
    names.sort_by_cached_key(|name| name.to_lowercase());
    names
}

fn append_distribution_names(
    key: HKEY,
    allow_distros_container: bool,
    names: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for index in 0..MAX_WSL_DISTRIBUTIONS {
        let mut key_name = [0u16; 512];
        let mut key_name_len = key_name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(key_name.as_mut_ptr())),
                &mut key_name_len,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            continue;
        }

        let key_name = String::from_utf16_lossy(&key_name[..key_name_len as usize]);
        let is_distros_container =
            allow_distros_container && key_name.eq_ignore_ascii_case("Distros");
        let key_name = HSTRING::from(key_name);
        let mut subkey = HKEY::default();
        let status = unsafe { RegOpenKeyExW(key, &key_name, None, KEY_READ, &mut subkey) };
        if status != ERROR_SUCCESS {
            continue;
        }
        let subkey = RegistryKey(subkey);
        if let Some(name) = read_distribution_name(subkey.0) {
            let Some(name) = validate_distribution_name(&name) else {
                continue;
            };
            if seen.insert(name.to_lowercase()) {
                names.push(name);
            }
        } else if is_distros_container {
            append_distribution_names(subkey.0, false, names, seen);
        }
    }
}

pub fn distribution_root(name: &str) -> Option<PathBuf> {
    let name = validate_distribution_name(name)?;
    Some(PathBuf::from(format!("{LOCALHOST_PREFIX}{name}\\")))
}

pub fn is_wsl_path(path: &Path) -> bool {
    path_candidates(path).is_some()
}

/// Читает корень или подпапку дистрибутива, пробуя оба UNC-алиаса WSL.
pub fn read_dir(path: &Path, cancel: &AtomicBool) -> Result<Vec<FileEntry>, ReadDirError> {
    let candidates = path_candidates(path).ok_or_else(|| {
        ReadDirError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a WSL path",
        ))
    })?;
    let mut last_error = None;
    for candidate in candidates {
        match crate::core::fs::read_dir(&candidate, cancel) {
            Ok(entries) => return Ok(entries),
            Err(error @ ReadDirError::Cancelled) => return Err(error),
            Err(error @ ReadDirError::Io(_)) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        ReadDirError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "WSL directory is unavailable",
        ))
    }))
}

pub fn read_subdirs(path: &Path) -> io::Result<Vec<PathBuf>> {
    let candidates = path_candidates(path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a WSL path"))?;
    let mut last_error = None;
    for candidate in candidates {
        match crate::core::fs::read_subdirs(&candidate) {
            Ok(directories) => return Ok(directories),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "WSL directory is unavailable")))
}

fn read_distribution_name(key: HKEY) -> Option<String> {
    let value_name = HSTRING::from("DistributionName");
    let mut value_type = REG_VALUE_TYPE(0);
    let mut byte_len = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            &value_name,
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS
        || (value_type != REG_SZ && value_type != REG_EXPAND_SZ)
        || byte_len == 0
        || byte_len > MAX_REGISTRY_STRING_BYTES
        || byte_len % 2 != 0
    {
        return None;
    }

    let mut bytes = vec![0u8; byte_len as usize];
    let status = unsafe {
        RegQueryValueExW(
            key,
            &value_name,
            None,
            Some(&mut value_type),
            Some(bytes.as_mut_ptr()),
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS || byte_len as usize > bytes.len() {
        return None;
    }
    let wide: Vec<u16> = bytes[..byte_len as usize]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let end = wide
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(wide.len());
    String::from_utf16(&wide[..end]).ok()
}

fn validate_distribution_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '<' | '>' | '"' | '|' | '?' | '*' | '/' | '\\' | ':'
                )
        })
    {
        return None;
    }
    Some(name.to_owned())
}

fn path_candidates(path: &Path) -> Option<[PathBuf; 2]> {
    let path_text = path.to_string_lossy();
    let lowercase_path = path_text.to_ascii_lowercase();
    if lowercase_path.starts_with(&LOCALHOST_PREFIX.to_ascii_lowercase()) {
        let suffix = &path_text[LOCALHOST_PREFIX.len()..];
        Some([
            path.to_path_buf(),
            PathBuf::from(format!("{LEGACY_PREFIX}{suffix}")),
        ])
    } else if lowercase_path.starts_with(&LEGACY_PREFIX.to_ascii_lowercase()) {
        let suffix = &path_text[LEGACY_PREFIX.len()..];
        Some([
            path.to_path_buf(),
            PathBuf::from(format!("{LOCALHOST_PREFIX}{suffix}")),
        ])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_root_and_unc_fallbacks_are_validated() {
        let root = distribution_root("Ubuntu").unwrap();
        assert_eq!(root.to_string_lossy(), r"\\wsl.localhost\Ubuntu\");
        let candidates = path_candidates(&root).unwrap();
        assert_eq!(candidates[1].to_string_lossy(), r"\\wsl$\Ubuntu\");
        assert!(distribution_root("../outside").is_none());
        assert!(distribution_root("bad\\name").is_none());
    }
}
