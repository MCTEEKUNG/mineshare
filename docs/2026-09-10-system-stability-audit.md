# MineShare system stability audit — 2026-09-10

## Scope and plan

Audit the existing working tree without reverting pending work. Fix reproduced
faults, retain the current protocol where possible, and do not claim zero bugs.

1. Establish Windows workspace and UI test/build baseline.
2. Trace discovery → authentication → session startup → teardown, especially
   simultaneous dialing, failed initialization, lost connections and stale workers.
3. Audit clipboard/file trust boundaries and cancellation for disconnects and data loss.
4. Audit input ownership, audio route lifecycle, settings persistence and UI commands.
5. Add targeted regression tests; rerun workspace tests, lint, UI and release builds.
6. Verify deployed binary identity and live connectivity when both machines are available.
   Physical mouse/keyboard/touchpad/audio acceptance and extended soak remain separate
   from automated test results.

## Baseline

- Rust workspace tests pass with the live runtime-owner test excluded; hardware tests
  are explicitly ignored. The ownership test currently uses the production mutex name.
- UI: 2 tests pass; TypeScript and Vite production build pass.
- Existing uncommitted input/audio/settings/UI changes preserved.

## Fixes delivered in this pass

| Area | Confirmed fault | Change / regression coverage |
| --- | --- | --- |
| Session ownership | Simultaneous inbound/outbound connections overwrite shared routing state. | One admission owner; both endpoints prefer the connection initiated by the lower authenticated static key. A replacement cancels and waits for the old lifetime/cleanup. Tests cover opposite arrival orders and failed claims. |
| Startup / teardown | Connected status published before fallible setup; worker aborts not awaited; TCP reader survives some exits. | Complete UDP/port/dispatcher setup first; monitor reader/writer/UDP task termination; abort and join all workers before resetting routes. |
| Connection stalls | Unbounded Noise/port exchange and control writes. | 5-second setup/write deadlines, 10-second control-idle deadline, maximum 8 inbound connection tasks. PIN entry retains its separate human timeout. |
| Disconnected input | Edge crossing, forced keyboard target and Game Drive can consume hardware without a peer. | Publish route readiness only after subscribers exist. Windows/Linux edge entry and shared key/button routing require a ready session, including held repeats after disconnect. Game Drive cannot start offline. Regression test covers all keyboard modes. |
| Clipboard | 64 KiB text plus framing exceeds Noise's ciphertext ceiling; inbound queue unbounded; failed watcher init permanently marks it running. | 60 KiB text cap, bounded nonblocking apply queue, reset watcher ownership on exit/start failure. Encrypted control round-trip and largest-text regression included. No clipboard/file payloads in control debug logs. |
| File integrity | Same-name receives share a staging file; counters restart at 1 on both peers; rename can overwrite files created after the offer. | Random JavaScript-safe IDs, unique create-new staging paths, no-overwrite publication, duplicate/direction validation. Real temporary-directory tests verify two concurrent same-name downloads preserve an existing file. |
| File cancellation | Cancelled work can become Active/Done again; some finalize errors remain in Verifying. | Terminal state guards, cancellation notification, receiver failure reporting, source size-change checks. Tests verify cancelled files never publish and outgoing ID collisions stay isolated. |
| Audio | A session starting before its worker can lose its prewarm transition; worker reads mixed session/active state. | Observe the first epoch transition and read identity/active state under their existing lock. Retain the existing jitter reserve and output-device settings. |
| Settings / trust | Non-finite sensitivity survives clamping; trust cache changes before disk success; direct JSON writes risk partial files. | Safe sensitivity default, persist-before-cache, reuse the existing atomic writer for trust/layout/identity, serialize layout application and propagation. |
| UI diagnostics | Packet/audio/error counters are declared but never published. | Publish real session counters on the existing 1 Hz stats tick and clear them at teardown. UI snapshot regression included. |
| Diagnostics CLI | `collect --push` can commit unrelated staged changes. | Scope diff and commit to the generated log path; distinguish Git failure from a changed file. No Git commit/push performed during this audit. |
| Verification | Runtime-owner test collides with a live installation; Linux CI lacks audio/input build packages; frontend tests omitted in CI. | Unique test mutex namespace; add required native packages, use npm ci and run frontend tests in CI. |

Windows no-overwrite publication uses `MoveFileExW` without the replacement flag;
the flag's overwrite semantics were checked against [Microsoft documentation](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw).

No new crate dependencies, UI toggles, or wire-message variants were added.
Existing uncommitted refactors are included in the build but are not represented as
new work from this audit.

## Verification and remaining limits

- Windows `cargo test --workspace --quiet`: **201 passed, 8 hardware/manual tests ignored**.
  Audio 66, core 5, daemon 43, input 80, network 7. The runtime-owner test now runs normally.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.
- UI: **2 tests passed**, TypeScript + Vite production build passed.
- Tauri backend `cargo check --all-targets`: passed before the final input-readiness guard;
  the final release build also compiles that guard.
- Deployment/live reconnect results recorded below after verification.

### Physical acceptance still required

1. Mouse/keyboard from either machine: cross both ways, type immediately, use Alt+Tab
   and Win+Shift+S, scroll/click/drag, and verify no stuck Shift after handback.
2. Disconnect the peer while using it: local keyboard/mouse must remain usable; repeat
   with ForcePeer and Game Drive selected. Restart either app independently.
3. Laptop touchpad: scroll/pinch/3–4 finger gestures and cross the edge mid-gesture.
4. PC audio → laptop headset: first sound after idle, continuous video, and switch
   Windows default output. No acoustic latency result is claimed from silent traffic.
5. Copy a screenshot; send two files with the same name; cancel a transfer.

### Limits and next release gates

- This is a risk-led system audit, not a proof that every execution path is bug-free.
- No real Linux hardware build/run or extended soak was performed in this pass.
  Unix no-overwrite publication currently requires a hard-link-capable download filesystem;
  unsupported filesystems fail instead of overwriting an existing file.
- Clipboard sync is best-effort: oversized text is skipped and an overloaded apply
  queue can reject a screenshot. File completion on the sender still means queued to the
  reliable transport, not a new receiver acknowledgement protocol.
- Do not lower audio jitter reserves or claim ≤1 ms input/acoustic latency without
  measuring physical capture-to-output latency. Network RTT is not that measurement.
- Before a wider release: run Linux CI, the physical matrix above, Wi-Fi sleep/wake,
  and a 12–24 hour soak with alternating single-peer restarts.

## Deployment / live checks

- Built `ui/src-tauri/target/release/mineshare-app.exe` and deployed with
  `scripts/deploy-windows-pair.ps1 -RemoteHost $PEER_IP -SkipBuild`.
- Installed SHA-256 on **both** laptop and PC:
  `AEA032093DDA97C0A7E063A23222E071E82E8009C76E8D2F4FAD80AEC4638961`.
- Both instances run in interactive session 1 through `MineShareLaunch`; exactly one
  `mineshare-app` process on each machine. Prior binaries retained as
  `mineshare-app.pre-audit-20260910.exe` in each per-user MineShare installation folder.
- Live PC-only restart: restarted at `14:12:21.739Z`, encrypted UDP ready at
  `14:12:29.930Z`, **8.19 seconds** including Windows task/app startup. Daemon startup
  itself began at `14:12:26.726Z`. Laptop was not restarted for this check.
- Live laptop-only restart: restarted at `14:13:02.583Z`, encrypted UDP ready at
  `14:13:04.051Z`, **1.47 seconds**. PC was not restarted for this check.
- Laptop output prewarmed successfully (`cpal playback ready`) after each reconnect.
  This confirms device initialization, **not** audible delivery/latency.
- Final idle smoke sample: laptop `14:13:33Z–14:14:14Z` and PC
  `14:13:32Z–14:14:13Z`: 41/42 traffic-stat samples, no session endings,
  no reported injection/decryption errors, one app process per machine.
  Samples contained no audio/input payload activity, so this is not a load test.
- Stale-port connect timeout during restart was observed, followed by automatic
  recovery to the new endpoint. Therefore do not describe reconnect as always <5 s.
- Both machines report optional VB-CABLE virtual-mic backend unavailable; no driver
  installation was attempted. This is distinct from peer system-audio playback.
- The legacy display build ID still uses commit/dirty metadata (including its old date);
  the SHA-256 above, not the displayed dirty build label, identifies this exact release.
