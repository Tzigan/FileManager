//! Тяжёлые сведения об одном уже выбранном объекте.
//!
//! Список папки их не запрашивает. Содержимое файла не читается:
//! размер на диске берётся у файловой системы, владелец и список доступа —
//! из дескриптора безопасности.

use std::path::Path;

use ::windows::Win32::Foundation::{ERROR_SUCCESS, GetLastError, HLOCAL, LocalFree};
use ::windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use ::windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, LookupAccountSidW, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SID_NAME_USE,
};
use ::windows::Win32::Storage::FileSystem::GetCompressedFileSizeW;
use ::windows::core::{HSTRING, PCWSTR, PWSTR};

use crate::core::entry_details::{AccessList, Detail, EntryDetails};

/// Имя и домен, которые ещё помещаются в обычную учётную запись.
/// Более длинный ответ не обрезаем, а помечаем недоступным.
const ACCOUNT_NAME_CHARS: usize = 256;

const INVALID_FILE_SIZE: u32 = 0xFFFF_FFFF;

pub fn read_entry_details(path: &Path) -> EntryDetails {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return EntryDetails::unavailable(),
    };
    // symlink_metadata не идёт по ссылке. Если это сама ссылка, дальнейшие
    // вызовы открыли бы цель и приписали бы её числа этой ссылке.
    if metadata.file_type().is_symlink() {
        return EntryDetails::for_symlink();
    }
    let (owner, access) = security_summary(path);
    EntryDetails {
        size_on_disk: allocated_size(path),
        folder_contents_not_counted: metadata.is_dir(),
        accessed: metadata
            .accessed()
            .map(Detail::Known)
            .unwrap_or(Detail::Unavailable),
        owner,
        access,
    }
}

fn allocated_size(path: &Path) -> Detail<u64> {
    let name = HSTRING::from(path.as_os_str());
    let mut high = 0u32;
    let low = unsafe { GetCompressedFileSizeW(&name, Some(&mut high)) };
    if low == INVALID_FILE_SIZE {
        let error = unsafe { GetLastError() };
        if error != ERROR_SUCCESS {
            return Detail::Unavailable;
        }
    }
    Detail::Known(((high as u64) << 32) | u64::from(low))
}

fn security_summary(path: &Path) -> (Detail<String>, Detail<AccessList>) {
    let name = HSTRING::from(path.as_os_str());
    let mut owner = PSID(std::ptr::null_mut());
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
    let status = unsafe {
        GetNamedSecurityInfoW(
            &name,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return (Detail::Unavailable, Detail::Unavailable);
    }
    let owner_name = account_name(owner);
    let access = if dacl.is_null() {
        Detail::Known(AccessList::Null)
    } else {
        let entries = unsafe { (*dacl).AceCount };
        Detail::Known(if entries == 0 {
            AccessList::Empty
        } else {
            AccessList::Present { entries }
        })
    };
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    (owner_name, access)
}

fn account_name(sid: PSID) -> Detail<String> {
    if sid.0.is_null() {
        return Detail::Unavailable;
    }
    let mut name = [0u16; ACCOUNT_NAME_CHARS];
    let mut domain = [0u16; ACCOUNT_NAME_CHARS];
    let mut name_len = name.len() as u32;
    let mut domain_len = domain.len() as u32;
    let mut name_use = SID_NAME_USE::default();
    let looked_up = unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            sid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut name_use,
        )
    };
    if looked_up.is_err() || name_len == 0 {
        return Detail::Unavailable;
    }
    let name = wide_prefix(&name, name_len);
    let domain = wide_prefix(&domain, domain_len);
    if name.is_empty() {
        return Detail::Unavailable;
    }
    Detail::Known(if domain.is_empty() {
        name
    } else {
        format!("{domain}\\{name}")
    })
}

fn wide_prefix(buffer: &[u16], reported_len: u32) -> String {
    let end = (reported_len as usize).min(buffer.len());
    let end = buffer[..end]
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(end);
    String::from_utf16_lossy(&buffer[..end])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::core::entry_details::Detail;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fm-entry-details-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn allocated_sizes_of_different_files_stay_separate_from_logical_length() {
        let dir = scratch("sizes");
        let small = dir.join("small.txt");
        let larger = dir.join("larger.txt");
        fs::write(&small, b"hi").unwrap();
        fs::write(&larger, vec![0u8; 5000]).unwrap();

        let small_details = read_entry_details(&small);
        let larger_details = read_entry_details(&larger);
        assert_eq!(fs::metadata(&small).unwrap().len(), 2);
        assert_eq!(fs::metadata(&larger).unwrap().len(), 5000);
        match (small_details.size_on_disk, larger_details.size_on_disk) {
            (Detail::Known(small_size), Detail::Known(larger_size)) => {
                // На этом томе маленький файл может занять ровно свою длину.
                // Различаются запросы двух файлов, а не подставленная константа.
                assert!(small_size >= 2);
                assert!(larger_size >= 5000);
                assert_ne!(small_size, larger_size);
            }
            other => panic!("local files should report allocated size, got {other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn folder_allocated_size_is_not_the_child_file() {
        let dir = scratch("folder");
        fs::write(dir.join("child.bin"), vec![0u8; 100_000]).unwrap();
        let details = read_entry_details(&dir);
        assert!(details.folder_contents_not_counted);
        if let Detail::Known(size) = details.size_on_disk {
            assert!(size < 100_000);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn details_request_does_not_resolve_symlink_target() {
        let dir = scratch("link");
        let target = dir.join("target.bin");
        fs::write(&target, vec![0u8; 100_000]).unwrap();
        let link = dir.join("Alias");
        let linked = std::os::windows::fs::symlink_dir(&dir, &link).is_ok()
            || std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &dir.to_string_lossy(),
                ])
                .status()
                .is_ok_and(|status| status.success());
        assert!(linked, "нужна точка соединения или ссылка");
        let details = read_entry_details(&link);
        assert_eq!(details.size_on_disk, Detail::Unavailable);
        assert_eq!(details.owner, Detail::Unavailable);
        assert!(!details.folder_contents_not_counted);
        let _ = fs::remove_dir(link);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_path_is_unavailable() {
        let missing =
            std::env::temp_dir().join(format!("fm-entry-details-missing-{}", std::process::id()));
        let details = read_entry_details(&missing);
        assert_eq!(details.size_on_disk, Detail::Unavailable);
        assert_eq!(details.owner, Detail::Unavailable);
        assert_eq!(details.access, Detail::Unavailable);
        assert!(!details.folder_contents_not_counted);
    }

    #[test]
    fn owned_file_names_the_owner_and_reports_an_access_list() {
        let dir = scratch("owner");
        let file = dir.join("owned.txt");
        fs::write(&file, b"x").unwrap();
        let details = read_entry_details(&file);
        match details.owner {
            Detail::Known(name) => assert!(!name.is_empty()),
            Detail::Unavailable => panic!("the created file should name its owner"),
        }
        assert!(matches!(details.access, Detail::Known(_)));
        let _ = fs::remove_dir_all(dir);
    }
}
