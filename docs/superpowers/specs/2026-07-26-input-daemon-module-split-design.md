# Input & Daemon Module Split — Design

> Refactor the three largest source files in MineShare into submodules by
> responsibility. Mechanical, behavior-preserving change. No new features, no
> bug fixes bundled in.

---

## 1. Goal

Three files have grown past the point of being easy to hold in context or
safely edit:

| File | Lines (current) | Uncommitted diff on `codex/wifi-performance-hardening` |
|---|---|---|
| `crates/mineshare-input/src/windows.rs` | 2706 | +1461 |
| `crates/mineshare-daemon/src/runtime.rs` | 2406 | +639 |
| `crates/mineshare-input/src/lib.rs` | 1604 | +222 |

Split each into a directory of submodules along the seams already visible in
the code (each file already contains several unrelated concerns bundled
together — this is a move, not a redesign).

**Success criteria:**
- No resulting file exceeds ~600–700 lines.
- Public API of `mineshare-input` and `mineshare-daemon` is unchanged — every
  external `use` site (`mineshare-daemon`, `ui/src-tauri`) compiles with no
  changes.
- `cargo build --workspace`, `cargo test --workspace`, and
  `cargo clippy --workspace -- -D warnings` pass after every round.
- Behavior is identical — verified by smoke-testing on the physical two-PC
  rig (see [[mineshare-test-rig]]) for the rounds that touch platform APIs or
  the network session.

## 2. Non-goals

- No public API changes.
- No behavior changes, bug fixes, or new features bundled into these commits.
  If a bug is spotted while moving code, note it and fix it separately.
- Does not implement the deeper testability refactor described in
  `docs/tech-debt-and-perf-assessment.md` (threading a `RoutingState` struct
  through `should_forward_keys`/`route_keystroke` instead of process-global
  statics). That assessment already identified `input/lib.rs`'s global state
  and `runtime.rs`'s god-function as debt; this spec executes the
  structural (file-split) half of that recommendation only. State-threading
  for unit-testability is explicitly out of scope and can follow as a later
  pass once the module boundaries exist.
- No change to the `mineshare-audio`, `mineshare-core`, `mineshare-net`, or
  `mineshare-ipc` crates.

## 3. Sequencing / prerequisite

**This work does not start until the `codex/wifi-performance-hardening`
branch's current uncommitted changes are committed (and merged, or at least
stable).** `windows.rs`, `runtime.rs`, and `input/lib.rs` all have pending
diffs right now; splitting these files first would create merge conflicts
with in-flight work. Order: finish and commit the Wi-Fi hardening work →
start this refactor on a fresh branch off the resulting commit.

## 4. Process (applies to every round)

One file per round. Each round is its own branch/commit(s), not squashed
together:

1. Create submodule files, move code verbatim (cut/paste, not rewritten),
   add `mod` declarations, add `pub use` re-exports where needed to keep the
   external API surface identical.
2. `cargo build --workspace` — fix only visibility/import errors introduced
   by the move.
3. `cargo test --workspace`.
4. `cargo clippy --workspace -- -D warnings`.
5. Manual smoke test on the rig for rounds 2 and 3 (see §8).
6. Commit.

Do not proceed to the next round until the current round's file builds,
tests, lints clean, and (where applicable) passes the rig smoke test.

## 5. Round 1 — `mineshare-input/src/lib.rs` (1604 lines)

Pure global-state module (atomics/getters/setters) plus shared types — no
platform API, lowest risk, done first to validate the process.

New layout under `crates/mineshare-input/src/`:

- `types.rs` — `Button`, `KeyCode`, `TouchpadGesture`, `InputEvent`,
  `ForwardedInputState`, `InputCapture`/`InputInject` traits, `RemoteEvent`,
  `PeerSide`.
- `settings.rs` — mouse sensitivity, scroll-invert flags, mouse rate/Hz,
  `GameDrive`, `KeyboardTarget` enum + cycling.
- `routing.rs` — `route_keystroke`, `route_mouse_button`, `should_forward_keys`,
  smart-decision reset, held-key/button tracking, activity-age tracking.
- `peer_state.rs` — `peer_side`/`set_peer_side`, `local_in_remote`,
  `peer_in_remote`, remote-event sender registration.
- `lib.rs` — reduced to `pub mod windows; pub mod linux;` plus the new
  `mod`/`pub use` declarations. No logic remains here.

## 6. Round 2 — `mineshare-input/src/windows.rs` (2706 lines)

Directly touches WinAPI (Raw Input, hooks, `SendInput`, touchpad injection) —
requires rig smoke-testing, not just `cargo test`.

New layout under `crates/mineshare-input/src/windows/`:

- `motion.rs` — motion-pacing/flush-watchdog (`flush_pending_motion*`,
  `queue_pending_motion`, `arm_pending_motion`, `wake_motion_flush_watchdog`,
  the watchdog thread, `monotonic_us`, `should_snap_virt_x`).
- `edge.rs` — screen geometry and edge-crossing (`local_screen_geometry`,
  `set_peer_screen`, `anchor`, `bounds`, `crossed_peer_edge`,
  `process_local_mouse_motion`, `enter_remote`/`exit_remote`,
  `local_in_remote`, `force_exit_remote`).
- `focus.rs` — `FocusHandoff`, `activate_window_under_cursor`,
  `TakeControlAction`, `should_anchor_cursor`, `take_control_action`,
  `on_peer_take_control`.
- `touchpad.rs` — gesture capture window + injection API
  (`set_touchpad_gesture_capture`, `touchpad_action_event`,
  `touchpad_swipe_event`, `TouchpadMotion`, `create_gesture_focus_window`,
  `setup_touchpad_gesture_capture`, `TouchpadInjectionApi`,
  `touchpad_injection_api`, `inject_touchpad_action`, `touchpad_contact`,
  `inject_touchpad_swipe`) — largest single chunk (~500 lines).
- `game_detect.rs` — `current_foreground_exe_basename`,
  `should_auto_lock_game`, `game_detect_thread`.
- `raw_input.rs` — `create_raw_input_window` (the WM_INPUT message loop,
  ~470 lines).
- `inject.rs` — `EnigoInject` + its `InputInject` impl,
  `send_relative_mouse_input`, `move_desktop_cursor_rel`,
  `send_wheel_input`/`windows_wheel_event`/`wheel_units`,
  `key_injection_spec`/`send_scancode_key_input`/`scancode_to_vk`,
  `captured_keycode`, `HeldKey`.
- `mod.rs` — `HighResTimerGuard`, `HookCapture` + its `InputCapture` impl,
  `sink_send`, module wiring, `pub use` re-exports to keep
  `mineshare_input::windows::*` call sites unchanged.

## 7. Round 3 — `mineshare-daemon/src/runtime.rs` (2406 lines)

Highest risk: `run_peer_session` is a single async fn spanning roughly lines
1167–1961 (~800 lines). A pure file move does not fix this — it needs
extract-function refactoring first.

**Step 1 — extract functions in place (no file move yet):**
Break `run_peer_session` into named async helpers along its existing phase
boundaries (handshake/pairing setup, stream/channel wiring, the main
select-loop, teardown). Verify with `cargo test --workspace` and a live
daemon run after each extraction before moving to Step 2.

**Step 2 — move into submodules** under
`crates/mineshare-daemon/src/runtime/`:

- `inject_dispatch.rs` — `InjectDispatcher`, `InjectQueue`, `PendingInject`,
  `MotionPacer`, `motion_pacing_step`, `InjectSender`, `can_merge_wire_motion`,
  `dispatch_inject_event`, `raise_input_thread_priority`.
- `control.rs` — `ControlMsg`, `ControlCollisionAction`,
  `control_collision_action`, `PortAnnounce`.
- `wire.rs` — `WireFrame`, `InputReceiveOrdering`, `SnapshotPressDeduper`,
  `write_encrypted`/`read_encrypted`, `hex_short`.
- `net.rs` — `accept_loop`, `handle_inbound`, `dial_and_run`, `pair_peer`,
  `enable_tcp_keepalive`, `detect_local_addresses`.
- `stats.rs` — `SessionStats`, `StatsSnapshot`.
- `session.rs` — the extracted helper functions from Step 1
  (`run_peer_session` and its phase helpers).
- `mod.rs` — `pub async fn run`, `RunOpts`, `NullInject`/`NullPlayback` test
  stubs, module wiring, `pub use` re-exports.

Requires rig smoke-testing (see §8) — this touches the full session
lifecycle: pairing, input forwarding, audio streams, reconnection.

## 8. Verification per round

After each round, before commit:

```
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Then, for Round 2 and Round 3 only, smoke-test on the physical rig
([[mineshare-test-rig]]):
- Round 2: cursor edge-crossing, keyboard/mouse forwarding, touchpad
  gestures, Game Drive toggle, game-mode auto-lock.
- Round 3: pairing a fresh device, reconnect after peer restart, input +
  audio flowing in both directions, latency telemetry still updating.

## 9. Risks

| Risk | Mitigation |
|---|---|
| Merge conflict with in-flight Wi-Fi hardening work | Do not start until that branch is committed (§3) |
| Silent behavior change during "mechanical" move (e.g. dropped `use`, reordered static init) | Diff review per round must show only moved lines, no logic edits; rig smoke test catches behavioral drift |
| `run_peer_session` extraction introduces a subtle ordering bug (this is the one round that isn't purely mechanical) | Two-step process (§7): extract-and-verify before moving files; extra rig test focus on reconnection/session teardown |
| Scope creep (fixing bugs noticed along the way) | Explicitly out of scope (§2) — log and defer |
