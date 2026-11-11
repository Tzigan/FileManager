//! Общие типы и низкоуровневые операции ZIP.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ::windows::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW, MoveFileW};
use ::windows::core::{Error as WindowsError, PCWSTR};

use crate::core::fs::ReadDirError;
use crate::system_tools::system_executable;

pub(super) const MAX_UNCOMPRESSED_BYTES: u64 = 16 * 1024 * 1024 * 1024 * 1024;
pub(super) const MAX_ARCHIVE_LISTING_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_ARCHIVE_MEMBERS: usize = 100_000;
pub(super) const MAX_ARCHIVE_NAME_ATTEMPTS: u32 = 10_000;
pub(super) const ZIP64_FILE_SIZE_THRESHOLD: u64 = u32::MAX as u64;
pub(super) const ZIP64_FILE_SIZE_MARGIN: u64 = 16 * 1024 * 1024;
pub(super) const COPY_BUFFER_SIZE: usize = 64 * 1024;

static NEXT_STAGE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArchiveMember {
    pub(super) path: String,
    pub(super) is_dir: bool,
}

pub(super) fn list_members(archive: &Path) -> io::Result<Vec<ArchiveMember>> {
    list_members_with_cancel(archive, None)
}

pub(super) fn list_members_with_cancel(
    archive: &Path,
    cancel: Option<&AtomicBool>,
) -> io::Result<Vec<ArchiveMember>> {
    let tar = system_executable("tar.exe")?;
    let mut child = Command::new(tar)
        .args(["-tf"])
        .arg(archive)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| io::Error::other("could not start the archive reader"))?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("archive reader did not return a listing"));
    };
    let mut listing = Vec::new();
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "archive listing cancelled",
            ));
        }
        let count = match stdout.read(&mut buffer) {
            Ok(count) => count,
            Err(error) => {
                drop(stdout);
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if count == 0 {
            break;
        }
        if listing.len().saturating_add(count) as u64 > MAX_ARCHIVE_LISTING_BYTES {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive listing exceeds the supported limit",
            ));
        }
        listing.extend_from_slice(&buffer[..count]);
    }
    drop(stdout);
    if !child.wait()?.success() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "archive is damaged or unsupported",
        ));
    }
    let listing = String::from_utf8(listing).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive contains unsupported filename encoding",
        )
    })?;
    let mut seen = HashSet::new();
    let mut members = Vec::new();
    for raw in listing.lines() {
        if let Some(member) = normalize_member(raw)? {
            if !seen.insert(member.path.clone()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "archive contains duplicate paths",
                ));
            }
            members.push(member);
            if members.len() > MAX_ARCHIVE_MEMBERS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "archive contains too many entries",
                ));
            }
        }
    }
    Ok(members)
}

pub(super) fn read_dir_error(error: io::Error) -> ReadDirError {
    if error.kind() == io::ErrorKind::Interrupted {
        ReadDirError::Cancelled
    } else {
        ReadDirError::Io(error)
    }
}

pub(super) fn normalize_prefix(prefix: &str) -> io::Result<String> {
    if prefix.is_empty() {
        Ok(String::new())
    } else {
        normalize_member(prefix)?
            .map(|member| member.path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid archive folder"))
    }
}

pub(super) fn normalize_member(raw: &str) -> io::Result<Option<ArchiveMember>> {
    let mut path = raw;
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    let is_dir = path.ends_with('/');
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "." {
        return Ok(None);
    }
    if path.starts_with('/')
        || path.chars().any(|ch| matches!(ch, '\\' | ':' | '\0'))
        || path.split('/').any(|part| !valid_component(part))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive contains an unsafe path",
        ));
    }
    Ok(Some(ArchiveMember {
        path: path.to_owned(),
        is_dir,
    }))
}

pub(super) fn valid_component(part: &str) -> bool {
    if part.is_empty()
        || part == "."
        || part == ".."
        || part.ends_with('.')
        || part.ends_with(' ')
        || part
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return false;
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(|ch| ch == '.' || ch == ' ')
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) && !reserved_numbered_device(&stem, "COM")
        && !reserved_numbered_device(&stem, "LPT")
}

pub(super) fn reserved_numbered_device(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix).is_some_and(|suffix| {
        matches!(
            suffix,
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
        )
    })
}

pub(super) fn move_directory_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
        )
    }
    .map_err(|error| move_error_to_io(error, destination, true))
}

pub(super) fn move_file_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
        )
    }
    .map_err(|error| move_error_to_io(error, destination, true))
}

pub(super) fn move_file_replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING,
        )
    }
    .map_err(|error| move_error_to_io(error, destination, false))
}

pub(super) fn move_error_to_io(
    error: WindowsError,
    destination: &Path,
    collision_is_error: bool,
) -> io::Error {
    if collision_is_error && fs::symlink_metadata(destination).is_ok() {
        return io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists");
    }

    // Win32 API возвращает HRESULT_FROM_WIN32; сохраняем код, чтобы UI распознал тип ошибки.
    let hresult = error.code().0 as u32;
    if hresult & 0xffff_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((hresult & 0xffff) as i32)
    } else {
        io::Error::other(error.to_string())
    }
}

pub(super) fn create_stage_dir(parent: &Path) -> io::Result<PathBuf> {
    for _ in 0..16 {
        let id = NEXT_STAGE_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".filemanager-zip-stage-{}-{id}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a temporary extraction folder",
    ))
}

pub(super) fn join_member(root: &Path, member: &str) -> io::Result<PathBuf> {
    let member = normalize_member(member)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty archive path"))?;
    let mut path = root.to_path_buf();
    for component in member.path.split('/') {
        path.push(component);
    }
    Ok(path)
}

pub(super) fn extract_member_to(
    archive: &Path,
    member: &str,
    destination: &Path,
    total: &mut u64,
) -> io::Result<()> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = (|| {
        let tar = system_executable("tar.exe")?;
        let mut child = Command::new(tar)
            .args(["-xOf"])
            .arg(archive)
            .arg("--")
            .arg(member)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| io::Error::other("could not start the archive extractor"))?;
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(
                "archive extractor did not return file data",
            ));
        };
        if let Err(error) = copy_with_limit(
            &mut stdout,
            &mut output,
            total,
            "archive expands beyond the extraction limit",
        ) {
            drop(stdout);
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        drop(stdout);
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive member could not be extracted",
            ));
        }
        output.sync_all()?;
        Ok(())
    })();
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

pub(super) fn copy_with_limit(
    reader: &mut impl Read,
    writer: &mut impl Write,
    total: &mut u64,
    limit_error: &'static str,
) -> io::Result<()> {
    let mut buffer = [0u8; COPY_BUFFER_SIZE];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        let next = total
            .checked_add(count as u64)
            .filter(|size| *size <= MAX_UNCOMPRESSED_BYTES)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, limit_error))?;
        writer.write_all(&buffer[..count])?;
        *total = next;
    }
}

