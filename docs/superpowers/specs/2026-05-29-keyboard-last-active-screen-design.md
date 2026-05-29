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
session". Evaluated top to bottom:

```text
1. cursor_in_remote (shared mouse has crossed onto the peer screen)
        → forward to peer.                                  [hard override]
   Rationale: the physical local mouse keeps generating HW motion while it
   drives the peer cursor, so "most recent local motion" would wrongly say
   local. Crossing is an unambiguous "the pointer is on the peer now" signal.

2. else if BOTH la and pa are "never" (== u64::MAX):
        keep the sticky decision (LAST_SMART_TO_PEER; default = local).

3. else if |la − pa| < SWITCH_MARGIN_MS (~200 ms):
        keep the current decision (LAST_SMART_TO_PEER).      [anti-flap]
   Only genuinely-simultaneous activity hits this; a deliberate move jumps
   the margin instantly (its age drops to ~0 while the other side ages),
   so responsiveness is unaffected.

4. else: more recent wins → to_peer = (pa < la).
```

Every path that reaches a concrete decision writes it back to
`LAST_SMART_TO_PEER` so steps 2–3 have a value to fall back to.

The other `KeyboardTarget` variants are unchanged:
`Auto` → strict `cursor_in_remote`; `ForcePeer` → always peer;
`ForceLocal` → always local.

**Removed:** the `CLICK_FRESH_MS` (30 s) click-priority branch and the
asymmetric `local_mouse_active_within(1500)` / `peer_mouse_active_within(2500)`
branches.

## "Activity" = motion OR click, weighted equally

- **Local:** `local_activity_age()` = `now_ms() − max(LOCAL_MOUSE_AT, LOCAL_CLICK_AT)`.
  The existing `bump_local_mouse_activity()` and `bump_local_click()` call
  sites are unchanged; we just read the max of the two stamps.
- **Peer:** collapse `PEER_MOUSE_AT` + `PEER_CLICK_AT` into a single
  `PEER_ACTIVITY_AT`, written by one new function `note_peer_activity(age_ms)`.

## Beacon accuracy fix (`mineshare-daemon/runtime.rs`)

The `ControlMsg::ActivityBeacon` and its sender/receiver change so peer timing
is accurate to network latency only:

- **Wire (`runtime.rs:137`):** replace
  `{ mouse_active: bool, last_click_ago_ms: Option<u32> }` with
  `{ last_input_ago_ms: Option<u32> }` — age (ms, capped 60 s) of the peer's
  most-recent mouse activity (motion *or* click), `None` if none this session.
- **Sender (`runtime.rs:~955`):** compute
  `last_input_ago_ms = min(local mouse age, local click age)`. Re-send when the
  freshness category flips (e.g. crossing the <1 s / <5 s boundary) or on the
  ~2 s refresh tick, mirroring the current cadence so we don't spam the control
  channel.
- **Receiver (`runtime.rs:~1082`):** `note_peer_activity(age)` →
  `PEER_ACTIVITY_AT = now_ms().saturating_sub(age)`.

Wire-format note: `ControlMsg` is positional bincode (a breaking change), but
both peers run the same build and the app already enforces a cross-machine
"same build" check, so changing the variant in lockstep is safe.

A new local helper `local_input_age_ms() -> Option<u32>` (cap 60 s) feeds the
sender; it supersedes `local_click_age_ms()` for the beacon. Existing
`local_mouse_active_within` / `peer_mouse_active_within` are removed if no other
caller remains (verify during implementation; the GUI status snapshot must keep
compiling).

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
statics). Cases:

- local activity newer than peer → local (`should_forward_keys(false) == false`)
- peer activity newer than local → peer (`== true`)
- `cursor_in_remote == true` → peer regardless of timestamps
- timestamps within `SWITCH_MARGIN_MS` → keeps prior sticky decision
- both never active → sticky default (local)
- `ForcePeer` / `ForceLocal` / `Auto` honored regardless of activity
- held-key continuation: down forwarded → up forwarded even after decision flip

Tests manipulate the statics via existing `bump_*` / `note_peer_activity`
setters (or `#[cfg(test)]` helpers) and reset between cases.

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
