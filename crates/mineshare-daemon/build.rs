//! Capture a precise build identity at compile time so two machines can
//! confirm they run the *same* code even when the semver didn't change
//! (a bare "0.0.5" can hide newer commits). Emits three env vars consumed
//! by `build_id()` in lib.rs:
//!   MINESHARE_GIT_HASH   short commit hash, or "unknown" off-repo
//!   MINESHARE_GIT_DIRTY  "-dirty" if tracked files were modified, else ""
//!   MINESHARE_BUILD_DATE the HEAD commit date (YYYY-MM-DD) — identical on
//!                        both machines for the same commit, unlike a
//!                        per-machine wall-clock build date.

use std::process::Command;

fn main() {
    let hash = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    // -uno: ignore untracked files so stray docs/build output don't flag a
    // clean code tree as dirty; only modified *tracked* files count.
    let dirty = match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(s) if !s.is_empty() => "-dirty",
        _ => "",
    };
    let date = git(&["show", "-s", "--format=%cd", "--date=short", "HEAD"])
        .unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=MINESHARE_GIT_HASH={hash}");
    println!("cargo:rustc-env=MINESHARE_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=MINESHARE_BUILD_DATE={date}");

    // Best-effort: re-run when the commit (or staged content) changes. The
    // release flow bumps the workspace version → full recompile → this
    // re-runs regardless, so released artifacts always carry a fresh hash.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    if let Some(refp) = head_ref_path() {
        println!("cargo:rerun-if-changed={refp}");
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// `.git/HEAD` is `ref: refs/heads/<branch>` on a normal checkout; return the
/// path of the ref it points at so a plain `git commit` re-triggers the build.
/// In a detached HEAD or git-worktree (`.git` is a file) this just returns
/// None and we fall back to watching HEAD/index only.
fn head_ref_path() -> Option<String> {
    let head = std::fs::read_to_string("../../.git/HEAD").ok()?;
    let r = head.strip_prefix("ref:")?.trim();
    Some(format!("../../.git/{r}"))
}
