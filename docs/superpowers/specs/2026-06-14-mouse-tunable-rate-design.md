# Mouse — User-Tunable Polling Rate (up to 1000 Hz)

_Date: 2026-06-14 · Crates: `mineshare-input` (+ `mineshare-daemon`, `ui/`)_

## Problem

Mouse motion forwarded to the peer is downsampled, and the rate is a
**compile-time constant** the user cannot influence:

- **Windows capture/forward** (`crates/mineshare-input/src/windows.rs`):
  raw input fires up to 1000 Hz, but `flush_pending_motion()` **sums** `dx/dy`
  into one combined `InputEvent::MouseMove` per `FLUSH_INTERVAL_MS = 2` (~500 Hz)
  window.
- **Linux inject** (`crates/mineshare-input/src/linux.rs`): the coalescer
  flushes at `FLUSH_INTERVAL_MS = 8` (~125 Hz) — the real Win→Linux bottleneck.

Two distinct limitations follow:

1. **Not user-controllable.** Users who want a higher (or lower, for bandwidth)
   rate cannot set it.
2. **Summing destroys high-rate cadence.** Because `dx/dy` are *summed* before
   sending, a game on the receiver reading Raw Input sees **one lumped delta per
   flush regardless of flush rate** — shrinking the interval alone can never
   produce true 1000 Hz in-game feel. (This is the architectural constraint that
   gates Phase 2.)

## Goal

Give the user an **in-app slider (60–1000 Hz) with a live readout** of the
actual achieved forward + inject rate, backed by a runtime-settable rate that
both OSes honor. Default safe; high rates opt-in. True in-game 1000 Hz fidelity
is a gated Phase 2 because it requires a wire-protocol change, not just a faster
flush.

User decisions (confirmed):
- **Control:** free slider, **60–1000 Hz**, default **500 Hz**, tick marks at
  125 / 250 / 500 / 1000.
- **Live readout:** yes — show measured forward Hz and inject Hz beside the
  slider.
- **Use case:** *"both, but desktop first"* — ship desktop smoothness now,
  treat true in-game 1000 Hz as a gated stretch.

## Design

### Phase 1 — Runtime-tunable rate + live telemetry (desktop smoothness)

This phase keeps the existing **summed-delta** model and makes the flush rate a
runtime value, driven by the UI. It delivers smooth fast-flick cursor motion on
the peer desktop (especially Win→Linux, by lifting the 8 ms floor) without any
wire-protocol change.

1. **Runtime rate, not a const.** Replace the per-platform `FLUSH_INTERVAL_MS`
   consts with a shared atomic (e.g. `static TARGET_FLUSH_US: AtomicU64` in a
   common location in `mineshare-input`), derived from the configured Hz:
   `flush_us = 1_000_000 / hz`. The Windows flush watchdog and the Linux
   coalescer both read this value each cycle.
   - Clamp to `[60, 1000]` Hz on ingest.
   - Sub-millisecond flush spacing: the current watchdogs `sleep(ms)`; for
     >500 Hz they must sleep in **microseconds** (use `Duration::from_micros`)
     and tolerate OS timer granularity (Windows default timer resolution is
     ~15.6 ms — request a higher resolution via `timeBeginPeriod(1)` while
     forwarding, released when idle, OR accept that achieved rate caps below
     the set rate and let the **live readout reflect reality**).
2. **Config plumbing.** UI slider → daemon command (e.g. `set_mouse_rate_hz`) →
   `mineshare-input` atomic. Persist via the existing config store so it
   survives restart. Expose current value via the status/IPC surface.
3. **Live telemetry.** Track forwarded-event count and injected-event count with
   timestamps; expose a rolling **measured Hz** for each direction through the
   existing `get_status` (or a small `get_mouse_stats`) so the UI can show
   "set 1000 Hz · forwarding ~820 Hz · injecting ~790 Hz". This makes hardware
   / OS-timer ceilings visible instead of silently lying.
4. **UI control.** A "Performance" section in `Settings` (per the UI spec):
   slider 60–1000 with tick marks, numeric display, and the live readout. Debounce
   writes so dragging the slider doesn't spam the daemon.

### Phase 2 — True in-game 1000 Hz (gated, opt-in)

Only pursue if the feasibility spike (below) shows the inject path can sustain
the rate. This is the architectural change:

1. **Stop summing — batch deltas.** Add a new wire variant
   `InputEvent::MouseMoveBatch { deltas: Vec<(i16, i16)> }` (or a small
   fixed-cap array) that carries the individual per-tick deltas captured within
   a send window, instead of one summed `MouseMove`.
2. **Replay on the receiver.** The inject side replays the batch as **N separate
   `mouse_move_rel` calls**, reproducing high-rate cadence a game's Raw Input
   reader can see.
3. **Wire-protocol versioning.** `peer_version` already exists. Negotiate batch
   support; if the peer is older or hasn't opted in, **fall back to the summed
   `MouseMove`** path. Never send a variant an old peer can't decode.

### Feasibility spike (must run before committing Phase 2)

Measure, do not assume: the receiver inject path serialises through a single
tokio task and (Windows) the Enigo mutex / `SendInput`; Linux uses `uinput`.
Spike: drive injects at 250/500/1000/sec on each OS and record the **max
sustained inject rate without lag/backpressure**. If a platform can't sustain
~1000/sec, cap that platform's effective rate there and surface it in the live
readout. The spike's result sizes (or kills) Phase 2.

## Caveats / risks

- **Anti-cheat rejects synthetic input.** Many anti-cheats (BattlEye, EAC,
  Vanguard, etc. — the very ones the app's game-lock guards) ignore or flag
  injected `SendInput`/`uinput` motion. So true in-game aim *on the peer* may
  not work for protected titles regardless of rate. Phase 2 is therefore a
  best-effort stretch for non-protected games, not a guarantee — documented in
  the UI near the slider.
- **OS timer granularity.** Sustained >500 Hz flushing needs a high-resolution
  timer; on Windows this means `timeBeginPeriod(1)` (raises system tick → minor
  power cost) only while actively forwarding. The live readout must show the
  *achieved* rate so users understand any gap from the set value.
- **Bandwidth/CPU.** Higher rates raise UDP packet volume and receiver inject
  load; the slider lets users trade fidelity for load. Default 500 Hz is a safe
  midpoint.
- **Forwarding only while `CURSOR_MODE == REMOTE`** — the rate applies only when
  actively driving the peer, never during idle local use (preserve this).

## Non-goals

- No change to cursor-crossing / edge-detection logic.
- No video; no change to keyboard routing.

## Verification

- Phase 1: set slider to 125/500/1000; confirm the live readout tracks and that
  Win→Linux fast flicks are visibly smoother than the old 8 ms floor; confirm
  value persists across restart; confirm old-peer interop unaffected (still
  summed `MouseMove`).
- Phase 2 (if pursued): batch variant round-trips; receiver replays N injects;
  old-peer fallback verified against a peer without batch support; spike numbers
  recorded in the PR.
