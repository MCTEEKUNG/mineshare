//! `mineshare-daemon collect [--push]` — bundle recent log files + system info
//! into `logs/<hostname>.log` in the current working directory, optionally
//! committing and pushing to the local git repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::identity::Identity;
use crate::logs;

const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024; // 4 MB cap per host
const MAX_RECENT_FILES: usize = 3; // bundle up to 3 most-recent daily logs

pub fn run(push: bool) -> Result<()> {
    let host = host_label();
    let target = std::env::current_dir()?
        .join("logs")
        .join(format!("{host}.log"));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).context("create logs/ dir")?;
    }

    let mut out = String::new();
    out.push_str(&build_header(&host)?);
    out.push_str("\n=== Daemon log (most recent first) ===\n\n");
    out.push_str(&read_recent_logs(MAX_RECENT_FILES, MAX_BUNDLE_BYTES)?);

    fs::write(&target, &out).with_context(|| format!("write {}", target.display()))?;
    println!("wrote {} ({} bytes)", target.display(), out.len());

    if push {
        ensure_git_repo()?;
        let rel = format!("logs/{host}.log");
        run_git(&["add", &rel])?;
        // No-op if nothing changed
        if !has_staged_changes()? {
            println!("no log changes to commit");
            return Ok(());
        }
        let stamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown".into());
        let msg = format!("logs: snapshot from {host} at {stamp}");
        run_git(&["commit", "-m", &msg])?;
        run_git(&["push"])?;
        println!("pushed {rel}");
    }
    Ok(())
}

fn host_label() -> String {
    // Sanitize hostname for use as a filename and OS suffix.
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{safe}-{}", std::env::consts::OS)
}

fn build_header(host: &str) -> Result<String> {
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".into());
    let mut s = String::new();
    s.push_str("=== MineShare log bundle ===\n");
    s.push_str(&format!("host:        {host}\n"));
    s.push_str(&format!("os:          {}\n", std::env::consts::OS));
    s.push_str(&format!("arch:        {}\n", std::env::consts::ARCH));
    s.push_str(&format!("collected:   {now}\n"));
    s.push_str(&format!("daemon ver:  {}\n", env!("CARGO_PKG_VERSION")));
    s.push_str(&format!(
        "log dir:     {}\n",
        logs::log_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "?".into())
    ));

    if let Ok(id) = Identity::load_or_create() {
        s.push_str(&format!("device id:   {}\n", id.device_id));
        s.push_str(&format!("display:     {}\n", id.display_name));
    }

    s.push_str(&format!("local addrs: {:?}\n", detect_local_addresses()));
    Ok(s)
}

fn detect_local_addresses() -> Vec<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr, UdpSocket};
    let mut addrs = Vec::new();
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0")
        && s.connect("8.8.8.8:80").is_ok()
        && let Ok(local) = s.local_addr()
    {
        addrs.push(local.ip());
    }
    if addrs.is_empty() {
        addrs.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    addrs
}

fn read_recent_logs(max_files: usize, max_bytes: u64) -> Result<String> {
    let dir = logs::log_dir()?;
    if !dir.exists() {
        return Ok("(no log directory yet)\n".into());
    }
    let mut files: Vec<(SystemTime, PathBuf, u64)> = fs::read_dir(&dir)?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("daemon"))
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let mt = m.modified().ok()?;
            Some((mt, e.path(), m.len()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    files.truncate(max_files);

    if files.is_empty() {
        return Ok("(no daemon log files found)\n".into());
    }

    let mut out = String::new();
    let mut budget = max_bytes;
    for (_, path, len) in &files {
        out.push_str(&format!("--- {} ({len} bytes) ---\n", path.display()));
        let take = (*len).min(budget);
        if take == 0 {
            out.push_str("(skipped — bundle size cap reached)\n\n");
            continue;
        }
        // Prefer tail-N reading rather than full-file when over budget
        let content = read_tail(path, take)?;
        out.push_str(&content);
        out.push('\n');
        budget = budget.saturating_sub(take);
    }
    Ok(out)
}

fn read_tail(path: &Path, max_bytes: u64) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn ensure_git_repo() -> Result<()> {
    let out = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .context("git not found in PATH")?;
    if !out.status.success() {
        bail!("not inside a git repository — `cd` to the cloned MineShare repo first");
    }
    Ok(())
}

fn run_git(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .status()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !status.success() {
        bail!("git {} failed (exit {:?})", args.join(" "), status.code());
    }
    Ok(())
}

fn has_staged_changes() -> Result<bool> {
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .status()?;
    // exit 0 = no changes, 1 = changes
    Ok(!status.success())
}
