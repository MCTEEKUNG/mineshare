//! In-app update: ask GitHub Releases for the newest published version, download the
//! installer that matches this OS, verify its SHA-256 against the digest GitHub computed
//! for the asset, then hand it to the platform installer.
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "MCTEEKUNG/mineshare";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    /// Whether this OS can install the update by itself (otherwise open `page`).
    pub installable: bool,
    pub page: String,
}

#[derive(Debug, PartialEq)]
struct Asset {
    name: String,
    url: String,
    sha256: String,
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut it = text.trim().trim_start_matches('v').split('.');
    let part = |s: Option<&str>| s?.split(['-', '+']).next()?.parse().ok();
    Some((part(it.next())?, part(it.next())?, part(it.next())?))
}

/// The installer for this OS, or `None` when it can only be updated by hand.
fn pick_asset<'a>(names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut names = names;
    if cfg!(target_os = "windows") {
        names.find(|n| n.ends_with("_x64-setup.exe"))
    } else if cfg!(target_os = "linux") && Path::new("/usr/bin/dpkg").exists() {
        names.find(|n| n.ends_with("_amd64.deb"))
    } else {
        None
    }
}

fn curl() -> Command {
    let mut command = Command::new("curl");
    // A generic client name: no device or account details leave this machine.
    command.args(["-fsSL", "--max-time", "600", "-A", "MineShare-updater"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

fn fetch_latest() -> Result<serde_json::Value> {
    let output = curl()
        .arg(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .output()
        .context("run curl (is it installed?)")?;
    ensure!(output.status.success(), "could not reach GitHub releases");
    serde_json::from_slice(&output.stdout).context("read release information")
}

fn info_from(release: &serde_json::Value) -> Result<UpdateInfo> {
    let tag = release["tag_name"].as_str().context("release has no tag")?;
    let latest = parse_version(tag).context("unreadable release version")?;
    let current = parse_version(CURRENT).context("unreadable app version")?;
    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    let installable = pick_asset(assets.iter().filter_map(|a| a["name"].as_str())).is_some();
    Ok(UpdateInfo {
        current: CURRENT.to_owned(),
        latest: tag.trim_start_matches('v').to_owned(),
        available: latest > current,
        installable,
        page: release["html_url"].as_str().unwrap_or_default().to_owned(),
    })
}

pub fn check() -> Result<UpdateInfo> {
    info_from(&fetch_latest()?)
}

fn asset_from(release: &serde_json::Value) -> Result<Asset> {
    let assets = release["assets"]
        .as_array()
        .context("release has no files")?;
    let name = pick_asset(assets.iter().filter_map(|a| a["name"].as_str()))
        .context("no installer for this system")?;
    let found = assets
        .iter()
        .find(|a| a["name"] == name)
        .context("asset vanished")?;
    let url = found["browser_download_url"]
        .as_str()
        .context("asset has no URL")?;
    ensure!(
        url.starts_with(&format!("https://github.com/{REPO}/releases/download/")),
        "unexpected download address"
    );
    let sha256 = found["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .context("GitHub published no checksum for this file; refusing to install")?
        .to_ascii_lowercase();
    Ok(Asset {
        name: name.to_owned(),
        url: url.to_owned(),
        sha256,
    })
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A fresh, unpredictable directory only this user can enter. Creation fails when the
/// path already exists, so a pre-planted directory or symlink can never be reused.
fn private_dir() -> Result<PathBuf> {
    use std::hash::{BuildHasher, Hasher};
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    let dir = base.join(format!("mineshare-update-{:016x}", hasher.finish()));
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .context("create private update folder")?;
    Ok(dir)
}

fn download(asset: &Asset) -> Result<PathBuf> {
    let dir = private_dir()?;
    let target = dir.join(&asset.name);
    let status = curl()
        .arg("-o")
        .arg(&target)
        .arg(&asset.url)
        .status()
        .context("download update")?;
    ensure!(status.success(), "download failed");
    ensure!(
        sha256_of(&target)? == asset.sha256,
        "downloaded file does not match its checksum"
    );
    Ok(target)
}

/// Download, verify and start the installer. Returns once the installer is running or
/// finished; on Windows the app exits so its files can be replaced, then the installer
/// script relaunches it.
pub fn install(exit: impl FnOnce()) -> Result<()> {
    let release = fetch_latest()?;
    ensure!(info_from(&release)?.available, "already up to date");
    let installer = download(&asset_from(&release)?)?;
    start_installer(&installer, exit)
}

#[cfg(target_os = "windows")]
fn start_installer(installer: &Path, exit: impl FnOnce()) -> Result<()> {
    use std::os::windows::process::CommandExt;
    // Paths travel as environment data, never inside the script text.
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            "Start-Sleep -Seconds 2; Start-Process -FilePath $env:MS_INSTALLER -ArgumentList '/S' -Wait; \
             Start-Process -FilePath $env:MS_APP",
        ])
        .env("MS_INSTALLER", installer)
        .env("MS_APP", std::env::current_exe()?)
        .creation_flags(0x0800_0000)
        .spawn()
        .context("start installer")?;
    exit();
    Ok(())
}

#[cfg(target_os = "linux")]
fn start_installer(installer: &Path, _exit: impl FnOnce()) -> Result<()> {
    // The package hooks stop and restart the GUI themselves.
    let status = Command::new("pkexec")
        .args(["apt-get", "install", "-y", "--reinstall"])
        .arg(installer)
        .status()
        .context("start installer (pkexec)")?;
    ensure!(status.success(), "installation was cancelled or failed");
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn start_installer(_: &Path, _: impl FnOnce()) -> Result<()> {
    anyhow::bail!("updates are not supported on this system")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("v0.0.10") > parse_version("0.0.9"));
        assert_eq!(parse_version("1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_version("nonsense"), None);
    }

    #[test]
    fn newer_release_is_available_and_older_is_not() {
        let release = |tag: &str| json!({"tag_name": tag, "html_url": "u", "assets": []});
        assert!(info_from(&release("v99.0.0")).unwrap().available);
        assert!(!info_from(&release("v0.0.1")).unwrap().available);
    }

    #[test]
    fn asset_needs_a_github_digest_and_trusted_url() {
        let name = if cfg!(windows) {
            "MineShare_9.9.9_x64-setup.exe"
        } else {
            "MineShare_9.9.9_amd64.deb"
        };
        let url = format!("https://github.com/{REPO}/releases/download/v9.9.9/{name}");
        let good = json!({"assets": [{"name": name, "browser_download_url": url, "digest": "sha256:ABCD"}]});
        let bad_url = json!({"assets": [{"name": name, "browser_download_url": "https://evil.example/x", "digest": "sha256:ab"}]});
        let no_digest = json!({"assets": [{"name": name, "browser_download_url": url}]});
        if cfg!(any(windows, all(target_os = "linux"))) && pick_asset([name].into_iter()).is_some()
        {
            assert_eq!(asset_from(&good).unwrap().sha256, "abcd");
            assert!(asset_from(&bad_url).is_err());
            assert!(asset_from(&no_digest).is_err());
        }
    }
}
