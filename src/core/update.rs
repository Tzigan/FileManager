//! Self-update from GitHub Releases: check → download → replace → restart.

use std::path::{Path, PathBuf};
use std::io::Read;

use anyhow::{Context, bail};

const LATEST_URL: &str = "https://api.github.com/repos/Tzigan/FileManager/releases/latest";
const USER_AGENT: &str = concat!("FileManager/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

fn parse_release(raw: &str) -> anyhow::Result<Release> {
    let resp: serde_json::Value = serde_json::from_str(raw).context("release json")?;
    let tag = resp["tag_name"].as_str().context("tag_name")?;
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let url = resp["assets"]
        .as_array()
        .context("assets")?
        .iter()
        .filter_map(|a| a["browser_download_url"].as_str())
        .find(|u| u.to_ascii_lowercase().ends_with(".exe"))
        .context("exe asset")?;
    Ok(Release {
        version: tag.to_string(),
        url: url.to_string(),
    })
}

/// Latest release tag and its `.exe` download URL.
/// Runs on a worker thread - blocking network I/O.
pub fn check_latest() -> anyhow::Result<Release> {
    let mut resp = ureq::get(LATEST_URL)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()?;
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut resp.body_mut().as_reader(), &mut raw).context("release body")?;
    parse_release(&raw)
}

/// `latest` is a newer semver than `current` (both may have a `v` prefix).
pub fn version_newer(current: &str, latest: &str) -> bool {
    fn parse(s: &str) -> Option<[u64; 3]> {
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
    match (parse(current), parse(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// Downloads the new exe next to the current one (`<name>.new.exe`).
pub fn download(url: &str) -> anyhow::Result<PathBuf> {
    let dst = std::env::current_exe()?.with_extension("new.exe");
    let mut resp = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()?;
    let mut reader = resp.body_mut().as_reader();
    write_download(&mut reader, &dst)
}

fn write_download(reader: &mut impl Read, dst: &Path) -> anyhow::Result<PathBuf> {
    let result = (|| {
        let mut file = std::fs::File::create(dst).with_context(|| format!("create {}", dst.display()))?;
        std::io::copy(reader, &mut file).context("write update")?;
        file.sync_all().context("flush update")?;
        Ok::<_, anyhow::Error>(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    Ok(dst.to_path_buf())
}

/// Replaces `current` with a downloaded file and restores the backup if the
/// second rename fails. This part is separated from process restart for tests.
fn replace_executable(current: &Path, new_path: &Path, backup: &Path) -> anyhow::Result<()> {
    if !new_path.is_file() {
        bail!("downloaded update does not exist: {}", new_path.display());
    }
    if backup.exists() {
        std::fs::remove_file(backup).with_context(|| format!("remove old backup {}", backup.display()))?;
    }
    std::fs::rename(current, backup).context("rename current executable to backup")?;
    if let Err(e) = std::fs::rename(new_path, current) {
        if let Err(rollback) = std::fs::rename(backup, current) {
            return Err(anyhow::anyhow!(
                "install update failed ({e}); restoring previous executable also failed ({rollback})"
            ));
        }
        return Err(e).context("install downloaded executable");
    }
    Ok(())
}

/// Windows allows renaming a running exe: current → `.old.exe`, new → current path, relaunch.
pub fn apply_and_restart(new_path: &Path) -> anyhow::Result<()> {
    let cur = std::env::current_exe()?;
    let old = cur.with_extension("old.exe");
    replace_executable(&cur, new_path, &old)?;
    std::process::Command::new(&cur).spawn().context("relaunch")?;
    std::process::exit(0);
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
    fn latest_release_selects_executable_asset_and_normalizes_tag() {
        let release = parse_release(
            r#"{"tag_name":"v0.7.0","assets":[
                {"browser_download_url":"https://example.invalid/source.zip"},
                {"browser_download_url":"https://example.invalid/FileManager.EXE"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            release,
            Release {
                version: "0.7.0".into(),
                url: "https://example.invalid/FileManager.EXE".into(),
            }
        );
    }

    #[test]
    fn malformed_release_or_missing_exe_is_rejected() {
        assert!(parse_release("not json").is_err());
        assert!(parse_release(r#"{"assets":[]}"#).is_err());
        assert!(parse_release(r#"{"tag_name":"v1.0.0","assets":[]}"#).is_err());
    }

    #[test]
    fn downloaded_bytes_are_written_exactly_and_partial_files_are_not_kept() {
        let dir = temp_dir();
        let dst = dir.join("update.new.exe");
        let bytes = b"sample update payload";
        assert_eq!(
            write_download(&mut Cursor::new(bytes), &dst).unwrap(),
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
        let dst = dir.join("update.new.exe");
        assert!(write_download(&mut BrokenReader(false), &dst).is_err());
        assert!(!dst.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn executable_swap_preserves_old_binary_as_backup() {
        let dir = temp_dir();
        let current = dir.join("app.exe");
        let new_path = dir.join("app.new.exe");
        let backup = dir.join("app.old.exe");
        std::fs::write(&current, b"old").unwrap();
        std::fs::write(&new_path, b"new").unwrap();

        replace_executable(&current, &new_path, &backup).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        assert!(!new_path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn executable_swap_replaces_a_stale_backup_with_the_current_binary() {
        let dir = temp_dir();
        let current = dir.join("app.exe");
        let new_path = dir.join("app.new.exe");
        let backup = dir.join("app.old.exe");
        std::fs::write(&current, b"current").unwrap();
        std::fs::write(&new_path, b"download").unwrap();
        std::fs::write(&backup, b"stale backup").unwrap();

        replace_executable(&current, &new_path, &backup).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), b"download");
        assert_eq!(std::fs::read(&backup).unwrap(), b"current");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_download_does_not_move_current_executable() {
        let dir = temp_dir();
        let current = dir.join("app.exe");
        let missing = dir.join("missing.new.exe");
        let backup = dir.join("app.old.exe");
        std::fs::write(&current, b"working version").unwrap();

        assert!(replace_executable(&current, &missing, &backup).is_err());
        assert_eq!(std::fs::read(&current).unwrap(), b"working version");
        assert!(!backup.exists());
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
