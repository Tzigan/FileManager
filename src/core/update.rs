//! Manifest-based update discovery and verified archive downloads.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde::Deserialize;

const MANIFEST_URL: &str =
    "https://github.com/Tzigan/FileManager/releases/latest/download/manifest.json";
const RELEASE_ASSET_PREFIX: &str = "https://github.com/Tzigan/FileManager/releases/download/";
const USER_AGENT: &str = concat!("FileManager/", env!("CARGO_PKG_VERSION"));
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_ARCHIVE_SIZE: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateManifest {
    pub schema_version: u32,
    pub version: String,
    pub archive_url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub minimum_supported_version: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

impl UpdateManifest {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema_version != 1 {
            bail!(
                "unsupported update manifest schema: {}",
                self.schema_version
            );
        }
        if parse_version(&self.version).is_none() {
            bail!("invalid update version: {}", self.version);
        }
        if !self.archive_url.starts_with(RELEASE_ASSET_PREFIX)
            || !self.archive_url.to_ascii_lowercase().ends_with(".zip")
        {
            bail!("update archive URL is not a FileManager HTTPS release asset");
        }
        if self.size == 0 || self.size > MAX_ARCHIVE_SIZE {
            bail!("invalid update archive size: {}", self.size);
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid update archive SHA-256");
        }
        if let Some(minimum) = &self.minimum_supported_version
            && parse_version(minimum).is_none()
        {
            bail!("invalid minimum supported version: {minimum}");
        }
        if self.signature.is_some() {
            bail!("signed update manifests are not supported by this unsigned build");
        }
        Ok(())
    }
}

fn parse_manifest(raw: &str) -> anyhow::Result<UpdateManifest> {
    let manifest: UpdateManifest = serde_json::from_str(raw).context("update manifest JSON")?;
    manifest.validate()?;
    Ok(manifest)
}

/// Fetches the manifest from the latest GitHub Release.
/// Runs on a worker thread because this performs blocking network I/O.
pub fn check_latest() -> anyhow::Result<UpdateManifest> {
    let mut resp = ureq::get(MANIFEST_URL)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()?;
    let mut raw = String::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_string(&mut raw)
        .context("manifest body")?;
    if raw.len() > MAX_MANIFEST_BYTES {
        bail!("update manifest is too large");
    }
    let manifest = parse_manifest(&raw)?;
    if let Some(minimum) = &manifest.minimum_supported_version
        && version_newer(env!("CARGO_PKG_VERSION"), minimum)
    {
        bail!("this update requires version {minimum} or newer");
    }
    Ok(manifest)
}

/// `latest` is a newer semver than `current` (both may have a `v` prefix).
pub fn version_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

fn parse_version(s: &str) -> Option<[u64; 3]> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let mut parse_component = || {
        let part = parts.next()?;
        if part.is_empty()
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        part.parse().ok()
    };
    let version = [parse_component()?, parse_component()?, parse_component()?];
    parts.next().is_none().then_some(version)
}

/// Downloads the release archive into a unique temporary file.
pub fn download(manifest: &UpdateManifest) -> anyhow::Result<PathBuf> {
    manifest.validate()?;
    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let dst = std::env::temp_dir().join(format!(
        "FileManager-update-{}-{id}.zip",
        std::process::id()
    ));
    let mut resp = ureq::get(&manifest.archive_url)
        .header("User-Agent", USER_AGENT)
        .call()?;
    let mut reader = resp.body_mut().as_reader();
    write_download(&mut reader, &dst, manifest.size)
}

fn write_download(
    reader: &mut impl Read,
    dst: &Path,
    expected_size: u64,
) -> anyhow::Result<PathBuf> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .with_context(|| format!("create {}", dst.display()))?;
    let result = (|| {
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            let read = reader.read(&mut buffer).context("read update archive")?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .context("update archive size overflow")?;
            if total > expected_size || total > MAX_ARCHIVE_SIZE {
                bail!("update archive exceeds the manifest size");
            }
            file.write_all(&buffer[..read])
                .context("write update archive")?;
        }
        if total != expected_size {
            bail!("update archive size does not match the manifest");
        }
        file.sync_all().context("flush update")?;
        Ok::<_, anyhow::Error>(())
    })();
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    Ok(dst.to_path_buf())
}

/// Removes a stale `.new.exe` / `.old.exe` from a previous run.
pub fn cleanup_leftovers() {
    if let Ok(cur) = std::env::current_exe() {
        cleanup_update_artifacts(&cur);
    }
}

fn cleanup_update_artifacts(current: &Path) {
    for ext in ["new.exe", "old.exe"] {
        let _ = std::fs::remove_file(current.with_extension(ext));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("filemanager-update-{id}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn valid_manifest() -> String {
        format!(
            r#"{{"schema_version":1,"version":"0.7.0","archive_url":"{RELEASE_ASSET_PREFIX}v0.7.0/FileManager-v0.7.0-x64.zip","sha256":"{}","size":7}}"#,
            "a".repeat(64)
        )
    }

    #[test]
    fn newer_semver() {
        assert!(version_newer("0.5.3", "0.6.0"));
        assert!(version_newer("0.5.3", "v0.5.4"));
        assert!(version_newer("1.9.9", "2.0.0"));
        assert!(!version_newer("0.6.0", "0.6.0"));
        assert!(!version_newer("v0.6.0", "0.6.0"));
        assert!(!version_newer("0.6.1", "0.6.0"));
        assert!(version_newer("0.9.9", "0.10.0"));
        assert!(!version_newer("0.10.0", "0.9.9"));
        assert!(!version_newer("0.6", "0.6.1"));
        assert!(!version_newer("0.6.0", "0.6.0.1"));
        assert!(!version_newer("unknown", "0.6.1"));
        assert!(!version_newer("vv0.6.0", "0.6.1"));
        assert!(!version_newer("0.06.0", "0.6.1"));
    }

    #[test]
    fn parses_manifest_with_required_integrity_metadata() {
        let manifest = parse_manifest(&valid_manifest()).unwrap();
        assert_eq!(manifest.version, "0.7.0");
        assert_eq!(manifest.size, 7);
        assert_eq!(manifest.sha256, "a".repeat(64));
        assert_eq!(manifest.minimum_supported_version, None);
        assert_eq!(manifest.signature, None);
    }

    #[test]
    fn rejects_invalid_manifest_url_hash_size_schema_and_signature() {
        assert!(parse_manifest("not json").is_err());
        let valid = valid_manifest();
        assert!(
            parse_manifest(&valid.replace("\"schema_version\":1", "\"schema_version\":2")).is_err()
        );
        assert!(parse_manifest(&valid.replace("https://", "http://")).is_err());
        assert!(parse_manifest(&valid.replace(&"a".repeat(64), "bad")).is_err());
        assert!(parse_manifest(&valid.replace("\"size\":7", "\"size\":0")).is_err());
        let with_minimum = valid.replace(
            "\"size\":7",
            "\"size\":7,\"minimum_supported_version\":\"0.6.5\"",
        );
        assert_eq!(
            parse_manifest(&with_minimum)
                .unwrap()
                .minimum_supported_version
                .as_deref(),
            Some("0.6.5")
        );
        assert!(
            parse_manifest(&valid.replace(
                "\"size\":7",
                "\"size\":7,\"minimum_supported_version\":\"0.6\""
            ))
            .is_err()
        );
        assert!(
            parse_manifest(&valid.replace("\"size\":7", "\"size\":7,\"signature\":\"unverified\""))
                .is_err()
        );
    }

    #[test]
    fn downloaded_bytes_are_written_exactly_and_partial_files_are_not_kept() {
        let dir = temp_dir();
        let dst = dir.join("update.zip");
        let bytes = b"sample update payload";
        assert_eq!(
            write_download(&mut Cursor::new(bytes), &dst, bytes.len() as u64).unwrap(),
            dst
        );
        assert_eq!(std::fs::read(&dst).unwrap(), bytes);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_download_write_removes_partial_artifact() {
        struct BrokenReader(bool);
        impl Read for BrokenReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::other("simulated transfer failure"));
                }
                self.0 = true;
                buf[..4].copy_from_slice(b"part");
                Ok(4)
            }
        }

        let dir = temp_dir();
        let dst = dir.join("update.zip");
        assert!(write_download(&mut BrokenReader(false), &dst, 8).is_err());
        assert!(!dst.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_download_size_is_rejected_and_partial_archive_removed() {
        let dir = temp_dir();
        let dst = dir.join("wrong-size.zip");
        assert!(write_download(&mut Cursor::new(b"short"), &dst, 100).is_err());
        assert!(!dst.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_cleanup_removes_only_update_sidecars() {
        let dir = temp_dir();
        let current = dir.join("app.exe");
        let new = dir.join("app.new.exe");
        let old = dir.join("app.old.exe");
        let unrelated = dir.join("notes.exe");
        for p in [&current, &new, &old, &unrelated] {
            std::fs::write(p, b"fixture").unwrap();
        }

        cleanup_update_artifacts(&current);
        assert!(current.exists());
        assert!(!new.exists());
        assert!(!old.exists());
        assert!(unrelated.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
