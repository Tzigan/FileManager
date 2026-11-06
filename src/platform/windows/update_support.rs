use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, HANDLE, HWND, NTSTATUS, WAIT_OBJECT_0,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_OBJECT_LENGTH,
    BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider,
    BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptGetProperty, BCryptHashData,
    BCryptOpenAlgorithmProvider,
};
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
    WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
    WTD_UI_NONE, WTD_UICONTEXT_EXECUTE, WinVerifyTrust,
};
use windows::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

pub const UPDATE_STAGE_ENV: &str = "FILEMANAGER_UPDATE_STAGE";
pub const UPDATE_UPDATER_PID_ENV: &str = "FILEMANAGER_UPDATE_UPDATER_PID";
const STAGE_PREFIX: &str = "FileManager-update-stage-";
pub(crate) const UPDATE_STARTED_MARKER: &str = ".update-started";
pub(crate) const PACKAGE_FILES: [&str; 2] = ["FileManager.exe", "FileManagerUpdater.exe"];

pub fn prepare_update_archive(
    archive: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> anyhow::Result<PathBuf> {
    if !archive.is_file() {
        bail!("downloaded update archive is missing");
    }
    let actual_size = fs::metadata(archive)?.len();
    if actual_size != expected_size {
        bail!("update archive size mismatch: expected {expected_size}, got {actual_size}");
    }
    let actual_sha256 = sha256_file(archive)?;
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        bail!("update archive SHA-256 mismatch");
    }
    validate_archive_members(archive)?;

    let stage = create_stage_directory()?;
    let result = (|| {
        let output = Command::new("tar.exe")
            .args(["-xf"])
            .arg(archive)
            .arg("-C")
            .arg(&stage)
            .output()
            .context("launch Windows archive extractor (tar.exe)")?;
        if !output.status.success() {
            bail!(
                "failed to extract update archive: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        validate_stage_files(&stage)?;
        verify_stage_signatures(&stage)?;
        Ok::<_, anyhow::Error>(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    Ok(stage)
}

pub fn launch_updater(stage: &Path, target_exe: &Path, process_id: u32) -> anyhow::Result<()> {
    validate_stage_files(stage)?;
    let updater = stage.join("FileManagerUpdater.exe");
    Command::new(&updater)
        .arg("--pid")
        .arg(process_id.to_string())
        .arg("--target")
        .arg(target_exe)
        .arg("--stage")
        .arg(stage)
        .spawn()
        .with_context(|| format!("launch updater {}", updater.display()))?;
    Ok(())
}

/// Signals a successful first frame to the updater, then removes the temporary
/// package after the updater process exits.
pub fn signal_update_started() -> anyhow::Result<()> {
    static SIGNALLED: AtomicBool = AtomicBool::new(false);
    if SIGNALLED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let Some(stage) = env::var_os(UPDATE_STAGE_ENV).map(PathBuf::from) else {
        return Ok(());
    };
    let pid = env::var(UPDATE_UPDATER_PID_ENV)
        .context("missing updater process id for staging cleanup")?
        .parse::<u32>()
        .context("invalid updater process id")?;

    let temp = env::temp_dir();
    let stage_name = stage.file_name().and_then(|name| name.to_str());
    if stage.parent() != Some(temp.as_path())
        || !stage_name.is_some_and(|name| name.starts_with(STAGE_PREFIX))
    {
        bail!("refusing to clean an unexpected update staging path");
    }

    let marker = stage.join(UPDATE_STARTED_MARKER);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .with_context(|| format!("create update startup marker {}", marker.display()))?
        .sync_all()
        .context("flush update startup marker")?;

    std::thread::spawn(move || {
        if wait_for_process_exit(pid).is_ok() {
            let _ = remove_dir_if_present(&stage);
        }
    });
    Ok(())
}

fn remove_dir_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove update stage {}", path.display())),
    }
}

pub(crate) fn wait_for_process_exit(pid: u32) -> anyhow::Result<()> {
    let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(process) => process,
        Err(error) if is_missing_process_error(&error) => return Ok(()),
        Err(error) => return Err(error).context("open process handle for update wait"),
    };

    let wait = unsafe { WaitForSingleObject(process, INFINITE) };
    unsafe {
        let _ = CloseHandle(process);
    }
    if wait == WAIT_OBJECT_0 {
        Ok(())
    } else {
        bail!("waiting for process {pid} failed ({})", wait.0);
    }
}

fn is_missing_process_error(error: &windows::core::Error) -> bool {
    (error.code().0 as u32 & 0xffff) == ERROR_INVALID_PARAMETER.0
}

fn validate_archive_members(archive: &Path) -> anyhow::Result<()> {
    let output = Command::new("tar.exe")
        .args(["-tf"])
        .arg(archive)
        .output()
        .context("inspect update archive with tar.exe")?;
    if !output.status.success() {
        bail!(
            "failed to inspect update archive: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let members: Vec<_> = String::from_utf8(output.stdout)?
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    validate_member_names(members)
}

fn validate_member_names(mut members: Vec<String>) -> anyhow::Result<()> {
    members.sort();
    let mut expected = PACKAGE_FILES.map(str::to_owned).to_vec();
    expected.sort();
    if members != expected {
        bail!("update archive must contain only FileManager.exe and FileManagerUpdater.exe");
    }
    Ok(())
}

pub(crate) fn validate_stage_files(stage: &Path) -> anyhow::Result<()> {
    let temp = env::temp_dir();
    let stage_name = stage.file_name().and_then(|name| name.to_str());
    if stage.parent() != Some(temp.as_path())
        || !stage_name.is_some_and(|name| name.starts_with(STAGE_PREFIX))
    {
        bail!("refusing to use an unexpected update staging path");
    }
    let mut members = Vec::new();
    for entry in
        fs::read_dir(stage).with_context(|| format!("read update stage {}", stage.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            bail!("update stage contains a non-file entry");
        }
        members.push(entry.file_name().to_string_lossy().into_owned());
    }
    members.sort();
    let mut expected = PACKAGE_FILES.map(str::to_owned).to_vec();
    expected.sort();
    if members != expected {
        bail!("update stage does not contain the expected application and updater executables");
    }
    Ok(())
}

pub(crate) fn verify_stage_signatures(stage: &Path) -> anyhow::Result<()> {
    for name in PACKAGE_FILES {
        verify_authenticode(&stage.join(name))?;
    }
    Ok(())
}

fn verify_authenticode(path: &Path) -> anyhow::Result<()> {
    if !path.is_file() {
        bail!("signed update executable is missing: {}", path.display());
    }
    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut file_info = WINTRUST_FILE_INFO::default();
    file_info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
    file_info.pcwszFilePath = windows::core::PCWSTR(path_wide.as_ptr());
    file_info.hFile = HANDLE::default();
    file_info.pgKnownSubject = std::ptr::null_mut();

    let mut trust_data = WINTRUST_DATA::default();
    trust_data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    trust_data.dwUIChoice = WTD_UI_NONE;
    trust_data.fdwRevocationChecks = WTD_REVOKE_NONE;
    trust_data.dwUnionChoice = WTD_CHOICE_FILE;
    trust_data.Anonymous.pFile = &mut file_info;
    trust_data.dwStateAction = WTD_STATEACTION_VERIFY;
    trust_data.dwProvFlags = WTD_REVOCATION_CHECK_NONE;
    trust_data.dwUIContext = WTD_UICONTEXT_EXECUTE;

    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe {
        WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut trust_data as *mut WINTRUST_DATA).cast(),
        )
    };
    trust_data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        let _ = WinVerifyTrust(
            HWND::default(),
            &mut action,
            (&mut trust_data as *mut WINTRUST_DATA).cast(),
        );
    }
    if status != 0 {
        bail!(
            "Authenticode verification failed for {} (0x{:08x})",
            path.display(),
            status as u32
        );
    }
    Ok(())
}

fn create_stage_directory() -> anyhow::Result<PathBuf> {
    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = env::temp_dir().join(format!("{STAGE_PREFIX}{}{id}", std::process::id()));
    fs::create_dir(&path).with_context(|| format!("create update stage {}", path.display()))?;
    Ok(path)
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut algorithm = BCRYPT_ALG_HANDLE::default();
    check_ntstatus(
        unsafe {
            BCryptOpenAlgorithmProvider(
                &mut algorithm,
                BCRYPT_SHA256_ALGORITHM,
                windows::core::PCWSTR::null(),
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
            )
        },
        "open SHA-256 provider",
    )?;
    let algorithm_guard = AlgorithmGuard(algorithm);
    let result = hash_file_with_algorithm(path, algorithm_guard.0);
    drop(algorithm_guard);
    result
}

fn hash_file_with_algorithm(path: &Path, algorithm: BCRYPT_ALG_HANDLE) -> anyhow::Result<String> {
    let mut object_len_bytes = [0_u8; 4];
    let mut bytes_returned = 0_u32;
    check_ntstatus(
        unsafe {
            BCryptGetProperty(
                BCRYPT_HANDLE::from(algorithm),
                BCRYPT_OBJECT_LENGTH,
                Some(&mut object_len_bytes),
                &mut bytes_returned,
                0,
            )
        },
        "read SHA-256 object size",
    )?;
    if bytes_returned != object_len_bytes.len() as u32 {
        bail!("unexpected SHA-256 object-size response");
    }
    let object_len = u32::from_le_bytes(object_len_bytes) as usize;
    if object_len == 0 || object_len > 16 * 1024 * 1024 {
        bail!("invalid SHA-256 object size");
    }
    let mut object = vec![0_u8; object_len];
    let mut hash = BCRYPT_HASH_HANDLE::default();
    check_ntstatus(
        unsafe { BCryptCreateHash(algorithm, &mut hash, Some(&mut object), None, 0) },
        "create SHA-256 hash",
    )?;
    let hash_guard = HashGuard(hash);
    let result = (|| {
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .context("read update archive for hashing")?;
            if read == 0 {
                break;
            }
            check_ntstatus(
                unsafe { BCryptHashData(hash_guard.0, &buffer[..read], 0) },
                "hash update archive",
            )?;
        }
        let mut digest = [0_u8; 32];
        check_ntstatus(
            unsafe { BCryptFinishHash(hash_guard.0, &mut digest, 0) },
            "finish SHA-256 hash",
        )?;
        let mut output = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write as _;
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
        }
        Ok(output)
    })();
    drop(hash_guard);
    result
}

fn check_ntstatus(status: NTSTATUS, operation: &str) -> anyhow::Result<()> {
    if status.0 < 0 {
        bail!("{operation} failed with NTSTATUS 0x{:08x}", status.0 as u32);
    }
    Ok(())
}

struct AlgorithmGuard(BCRYPT_ALG_HANDLE);

impl Drop for AlgorithmGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptCloseAlgorithmProvider(self.0, 0);
        }
    }
}

struct HashGuard(BCRYPT_HASH_HANDLE);

impl Drop for HashGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptDestroyHash(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cng_sha256_matches_the_standard_known_vector() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("filemanager-sha256-{}-{id}", std::process::id()));
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn update_archive_member_allowlist_rejects_extra_and_traversal_paths() {
        assert!(
            validate_member_names(vec![
                "FileManager.exe".into(),
                "FileManagerUpdater.exe".into()
            ])
            .is_ok()
        );
        assert!(
            validate_member_names(vec![
                "FileManager.exe".into(),
                "FileManagerUpdater.exe".into(),
                "../outside.txt".into()
            ])
            .is_err()
        );
    }

    #[test]
    fn authenticode_verification_rejects_an_unsigned_file() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("filemanager-unsigned-{id}.exe"));
        fs::write(&path, b"not a signed executable").unwrap();
        assert!(verify_authenticode(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}
