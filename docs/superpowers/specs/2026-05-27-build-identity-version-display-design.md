# Build-identity & cross-machine version display

**Date:** 2026-05-27
**Status:** Approved (user)

## Problem

The app only knew its **semver** (`env!("CARGO_PKG_VERSION")`). During a session
we deployed binaries that reported "0.0.5" while actually running newer code, so
there was no reliable way to tell whether two connected machines run the *same
build*. We need an unambiguous build identifier, surfaced in the app, with a
clear same/different indicator across the bridge.

## Design

### 1. Compile-time build ID — `crates/mineshare-daemon/build.rs` (new)
- `git rev-parse --short=7 HEAD` → `MINESHARE_GIT_HASH` (fallback `unknown` if git
  absent, so off-repo builds still compile).
- `git status --porcelain` non-empty → `MINESHARE_GIT_DIRTY=-dirty`, else empty.
- UTC build date `YYYY-MM-DD` → `MINESHARE_BUILD_DATE`.
- `cargo:rerun-if-changed` for `.git/HEAD`, the resolved ref file, and `.git/index`
  (best-effort dev freshness). **Release accuracy** is guaranteed because the
  release flow bumps the workspace version → full recompile → build.rs re-runs.
- `mineshare_daemon::build_id()` → `"{semver} · {hash}{dirty} · {date}"`,
  e.g. `0.0.6 · 1a2b3c4 · 2026-05-27`. Also a compact `build_id_short()` →
  `"{semver} · {hash}{dirty}"` for tray/tooltip use.

### 2. Exchange across the bridge
- `PortAnnounce.daemon_version` (already exchanged in the handshake) switches from
  bare semver to `build_id()`. Each side now learns the other's exact build.

### 3. Status snapshot — `crates/mineshare-daemon/src/status.rs`
- Add `local_version: String` (always = `build_id()`).
- Add `peer_version: Option<String>` — set on connect from
  `peer_announce.daemon_version`, cleared on disconnect (extend
  `set_peer_connected`/`clear_peer_connected` or add dedicated setters).

### 4. GUI — `ui/src/App.tsx`
- Extend the `Status` TS type with `local_version` + `peer_version`.
- Always-visible footer in the sidebar: `MineShare {local_version}`.
- When connected, a second line comparing builds:
  - match → `Peer: {peer_version} ✓ same build` (muted/green).
  - mismatch → `⚠ different build (peer: {peer_version})` (highlighted/amber).
- New UI strings go through the existing `i18n.tsx` (TH/EN).

### 5. Tray — `ui/src-tauri/src/tray.rs`
- Tooltip → `MineShare {build_id_short}`.
- New disabled menu item showing the local build ID.
- Status header line gains `· ✓ same build` / `· ⚠ build mismatch` when paired.

## Out of scope
- No auto-update / no blocking on mismatch — display only (informational).
- `build_id` semver source is the daemon crate's `CARGO_PKG_VERSION` (= workspace
  version), kept in sync with the app crate by the existing release bump.

## Rollout
1. Implement the above.
2. Bump workspace + app version 0.0.5 → **0.0.6**; commit (clean tree → clean hash).
3. Build installers (`npm run tauri build`).
4. On **both** machines: uninstall/remove every old MineShare copy + stale
   shortcuts, then install v0.0.6 via the NSIS installer; re-pin.
5. Verify both apps show the **same build ID** and the GUI/tray report `✓ same build`.
6. Tag + GitHub release v0.0.6.

## Verification
- `build_id()` reflects the committed hash (no `-dirty` on a clean release build).
- Two machines on the same commit show identical IDs and `✓ same build`.
- A deliberately mismatched pair (one old binary) shows `⚠ different build`.
