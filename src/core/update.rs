//! Self-update from GitHub Releases: check → download → replace → restart.

use std::path::{Path, PathBuf};

use anyhow::Context;

const LATEST_URL: &str = "https://api.github.com/repos/Tzigan/FileManager/releases/latest";
const USER_AGENT: &str = concat!("FileManager/", env!("CARGO_PKG_VERSION"));

pub struct Release {
    pub version: String,
    pub url: String,
}

/// Latest release tag and its `.exe` download URL.
/// Runs on a worker thread — blocking network I/O.
pub fn check_latest() -> anyhow::Result<Release> {
    let mut resp = ureq::get(LATEST_URL)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()?;
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut resp.body_mut().as_reader(), &mut raw).context("release body")?;
    let resp: serde_json::Value = serde_json::from_str(&raw).context("release json")?;
    let tag = resp["tag_name"].as_str().context("tag_name")?.trim_start_matches('v');
    let url = resp["assets"]
        .as_array()
        .context("assets")?
        .iter()
        .filter_map(|a| a["browser_download_url"].as_str())
        .find(|u| u.ends_with(".exe"))
        .context("exe asset")?;
    Ok(Release {
        version: tag.to_string(),
        url: url.to_string(),
    })
}

/// `latest` is a newer semver than `current` (both may have a `v` prefix).
pub fn version_newer(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parse(latest) > parse(current)
}

/// Downloads the new exe next to the current one (`<name>.new.exe`).
pub fn download(url: &str) -> anyhow::Result<PathBuf> {
    let dst = std::env::current_exe()?.with_extension("new.exe");
    let mut resp = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()?;
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(&dst).with_context(|| format!("create {}", dst.display()))?;
    std::io::copy(&mut reader, &mut file).context("write update")?;
    Ok(dst)
}

/// Windows allows renaming a running exe: current → `.old.exe`, new → current path, relaunch.
pub fn apply_and_restart(new_path: &Path) -> anyhow::Result<()> {
    let cur = std::env::current_exe()?;
    let old = cur.with_extension("old.exe");
    if old.exists() {
        let _ = std::fs::remove_file(&old);
    }
    std::fs::rename(&cur, &old).context("rename current")?;
    std::fs::rename(new_path, &cur).context("rename new")?;
    std::process::Command::new(&cur).spawn().context("relaunch")?;
    std::process::exit(0);
}

/// Removes a stale `.new.exe` / `.old.exe` from a previous run.
pub fn cleanup_leftovers() {
    if let Ok(cur) = std::env::current_exe() {
        for ext in ["new.exe", "old.exe"] {
            let _ = std::fs::remove_file(cur.with_extension(ext));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_semver() {
        assert!(version_newer("0.5.3", "0.6.0"));
        assert!(version_newer("0.5.3", "v0.5.4"));
        assert!(!version_newer("0.6.0", "0.6.0"));
        assert!(!version_newer("0.6.1", "0.6.0"));
        assert!(version_newer("0.9.9", "0.10.0"));
        assert!(!version_newer("0.10.0", "0.9.9"));
    }
}
