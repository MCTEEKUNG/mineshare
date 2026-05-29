# Keyboard Routing — "Last-Active-Screen" Smart Mode

_Date: 2026-05-29 · Crate: `mineshare-input` (+ `mineshare-daemon` beacon)_

## Problem

The Smart keyboard-routing decision (`should_forward_keys` in
`crates/mineshare-input/src/lib.rs`) mis-detects which screen the user wants
the keyboard on. Concrete failures today:

1. **Stale click hijack.** A click on either side wins for a full 30 s
   (`CLICK_FRESH_MS`), *overriding* fresh mouse motion. So "I clicked screen B
   20 s ago, then dragged my mouse to screen A" leaves the keyboard stuck on B
   — the keystrokes land on the wrong machine.
2. **Asymmetric magic thresholds.** Local motion counts as active for 1.5 s,
   peer motion for 2.5 s. The asymmetry produces surprising flips.
3. **Imprecise peer timing.** The peer's activity timestamp is stamped at
   *beacon-arrival time* with a boolean `mouse_active` (active within 700 ms),
   so the peer can appear up to ~1.2 s newer/older than reality — wrong side
   wins in close calls.

The user runs **both** input topologies, mixed:
- one shared mouse that crosses the screen edge (Barrier/Synergy style), and
- two independent mice (one per machine).

## Goal

A single, predictable rule the user can model in their head:

> **The keyboard follows the screen whose mouse was active most recently.**
> Drag the mouse to screen A → the keyboard goes to A. Immediately, every time.

Requirements: never mis-detect, feel smooth (no flap), cover both topologies.

## Decision rule (`should_forward_keys`, `KeyboardTarget::Smart`)

Let `la` = `local_activity_age()` and `pa` = `peer_activity_age()`, both in ms
where **smaller age = more recent**; `u64::MAX` means "never active this
session". "Activity" = a **deliberate** mouse drag (see debounce below) or any
click. Evaluated top to bottom:

```text
1. cursor_in_remote (shared mouse has crossed onto the peer screen)
        → forward to peer.                                  [hard override]
   Rationale: the physical local mouse keeps generating HW motion while it
   drives the peer cursor, so "most recent local motion" would wrongly say
   local. Crossing is an unambiguous "the pointer is on the peer now" signal.

2. else if BOTH la and pa are "never" (== u64::MAX):
        keep the sticky decision (LAST_SMART_TO_PEER; default = local).

3. else: more recent wins → to_peer = (pa < la).
```

Every path that reaches a concrete decision writes it back to
`LAST_SMART_TO_PEER` so step 2 has a value to fall back to.

No `SWITCH_MARGIN` knob: the deliberate-motion **debounce** below is what
prevents an accidental nudge from stealing the keyboard, so a separate
anti-flap margin is redundant. The only tunables are the two debounce
constants, which have a clear physical meaning (drag vs. nudge), not arbitrary
thresholds.

The other `KeyboardTarget` variants are unchanged:
`Auto` → strict `cursor_in_remote`; `ForcePeer` → always peer;
`ForceLocal` → always local.

**Removed:** the `CLICK_FRESH_MS` (30 s) click-priority branch and the
asymmetric `local_mouse_active_within(1500)` / `peer_mouse_active_within(2500)`
branches.

## "Activity" = deliberate motion OR click

- **Local:** `local_activity_age()` returns `now_ms() − max(LOCAL_MOUSE_AT,
  LOCAL_CLICK_AT)`, **except** it returns `u64::MAX` when both stamps are 0
  ("never active") — mirroring the existing `local_click_age()` sentinel, so
  `now − 0` does not masquerade as recent activity.
- `LOCAL_CLICK_AT` is stamped on every click (always deliberate — a click
  focuses a window). `LOCAL_MOUSE_AT` is stamped **only on deliberate drags**,
  per the debounce below.
- **Peer:** collapse `PEER_MOUSE_AT` + `PEER_CLICK_AT` into a single
  `PEER_ACTIVITY_AT`, written by one new function `note_peer_activity(age_ms)`.

## Deliberate-motion debounce (the anti-steal mechanism)

The user's word is "ลาก" (drag) — a *sustained* motion, not an accidental
nudge. So `bump_local_mouse_activity()` (called once per HW motion event on
both Win and Linux, with no delta argument) only stamps the routing activity
clock once a motion *run* has lasted long enough. Time-based so it needs no
delta and behaves identically across platforms and mouse DPIs:

```text
two extra statics: MOTION_RUN_START, LAST_MOTION_AT (raw, every event)
on each motion at `now`:
    if now − LAST_MOTION_AT > IDLE_RESET_MS  → MOTION_RUN_START = now   (new run)
    LAST_MOTION_AT = now
    if now − MOTION_RUN_START >= DEBOUNCE_MS → LOCAL_MOUSE_AT = now      (deliberate)
```

- `DEBOUNCE_MS ≈ 120` — a run shorter than this (a flick/nudge that stops)
  never stamps `LOCAL_MOUSE_AT`, so it cannot claim the keyboard from a peer
  you are actively using. A sustained drag claims after ~120 ms (imperceptible
  mid-drag) and keeps claiming for the rest of the drag.
- `IDLE_RESET_MS ≈ 250` — a pause longer than this starts a fresh run, so two
  separate nudges don't accumulate into a false "drag".

The debounce lives on the side that owns the mouse, so the beacon-reported peer
age is *already* debounced — the mechanism is symmetric with no extra peer-side
logic. Clicks bypass the debounce entirely (instant claim).

## Beacon accuracy fix (`mineshare-daemon/runtime.rs`)

The `ControlMsg::ActivityBeacon` and its sender/receiver change so peer timing
is accurate to network latency only:

- **Wire (`runtime.rs:137`):** replace
  `{ mouse_active: bool, last_click_ago_ms: Option<u32> }` with
  `{ last_input_ago_ms: Option<u32> }` — age (ms, capped 60 s) of the peer's
  most-recent mouse activity (motion *or* click), `None` if none this session.
- **Sender (`runtime.rs:~955`):** compute `last_input_ago_ms` from
  `local_input_age_ms()` = the smaller of the present local mouse / click ages
  (`None` only if neither exists). **Cadence:** because the decision is a
  most-recent-age race, a stale `PEER_ACTIVITY_AT` would skew it — so while the
  local side is freshly active (`age < ~3 s`) send **every tick (500 ms)** to
  keep the peer's view current; otherwise fall back to the change-triggered +
  ~2 s refresh so we don't spam the control channel while idle. (The previous
  code only sent on a bool flip + 2 s refresh, which left peer age up to ~2 s
  stale during active use.)
- **Receiver (`runtime.rs:~1082`):** `note_peer_activity(age)` →
  `PEER_ACTIVITY_AT = now_ms().saturating_sub(age)`.

Wire-format note: `ControlMsg` is positional bincode (a breaking change), but
both peers run the same build and the app already enforces a cross-machine
"same build" check, so changing the variant in lockstep is safe.

A new local helper `local_input_age_ms() -> Option<u32>` (cap 60 s) feeds the
sender; it supersedes `local_click_age_ms()` for the beacon. `local_click_age_ms`,
`local_mouse_active_within`, `peer_mouse_active_within`, and
`note_peer_mouse_active` are removed — **confirmed** (grep) to have no callers
outside `should_forward_keys` and the beacon sender/receiver, all of which this
change rewrites. The GUI status snapshot reads only `keyboard_target()` /
`local_in_remote()` / `peer_in_remote()`, which are untouched.

## Held-key continuation — KEEP UNCHANGED

`route_keystroke` / `HELD_FORWARDED` and `route_mouse_button` /
`MOUSE_BTNS_FORWARDED` stay exactly as they are. They guarantee a key/button
whose *down* was forwarded also forwards its *up* to the same destination even
if the Smart decision flips mid-press — this is what prevents stuck
modifiers/buttons and is essential to "smooth". The decision rule above only
governs *fresh* key-downs.

## Session reset

Extend `reset_smart_decision()` (called on session start) to also clear
`PEER_ACTIVITY_AT` so a new peer never inherits stale peer-activity timing. It
already clears `LAST_SMART_TO_PEER`, held keys, and held buttons. Local stamps
persist (they reflect real local hardware).

## Testing

`mineshare-input` currently has **zero** tests; this adds the first. A
`#[cfg(test)]` module on the routing logic, run serially (`serial`-style: a
shared `Mutex` guard or single combined test fn, since state is process-global
statics). To avoid flakiness, tests **inject absolute stamp values** through
`#[cfg(test)]` setters (e.g. set `LOCAL_MOUSE_AT`/`PEER_ACTIVITY_AT` to
`now_ms() − N`) rather than sleeping on the wall clock. Cases:

- local activity newer than peer → local (`should_forward_keys(false) == false`)
- peer activity newer than local → peer (`== true`)
- `cursor_in_remote == true` → peer regardless of stamps
- both never active → sticky default (local)
- `ForcePeer` / `ForceLocal` / `Auto` honored regardless of activity
- held-key continuation: down forwarded → up forwarded even after decision flip
- **debounce:** a single nudge run (< `DEBOUNCE_MS`) does **not** advance
  `LOCAL_MOUSE_AT`, so an active peer keeps the keyboard; a sustained run
  (≥ `DEBOUNCE_MS`) does advance it and claims local. Driven by calling
  `bump_local_mouse_activity()` with injected `LAST_MOTION_AT`/`MOTION_RUN_START`
  values (no real sleeps).

Reset all statics between cases.

## Out of scope

- No change to mouse-event forwarding, cursor-cross edge/hysteresis, or the
  audio/file paths.
- No GUI changes beyond keeping the status pill compiling (it reads
  `keyboard_target()`, which is unchanged).

## Files touched

- `crates/mineshare-input/src/lib.rs` — decision rule, activity helpers,
  peer-activity stamp, reset, tests.
- `crates/mineshare-daemon/src/runtime.rs` — `ActivityBeacon` variant + sender
  + receiver.
