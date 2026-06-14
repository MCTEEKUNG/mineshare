# Game Drive Mode — Drive a Peer's Game Without Cursor-Crossing

_Date: 2026-06-14 · Crates: `mineshare-input`, `mineshare-net` (ControlMsg), `mineshare-daemon`, `ui/`_

## Problem

A user wants to play a game running on the **peer** machine using the local
keyboard + mouse (e.g. Roblox on the Ubuntu/secondary PC, driven from the
primary). The existing cursor-crossing model breaks this badly: in-game the
camera **flings violently**.

Root cause established by systematic debugging (daemon-log evidence, three
failed incremental fixes):

1. **Handover transients.** Every time the peer takes Remote control, the
   sender computes the first motion delta against a stale anchor, producing a
   `~screen-half` `MouseMove` (logged: `dx: -969`). Clamping it on the receiver
   to ±200 reduced but did not remove the jerk.
2. **Cursor warp.** `on_peer_take_control` `SetCursorPos`-warps the receiver's
   cursor to the screen edge; a LockCenter game (Roblox) recenters every frame,
   so the warp starts a tug-of-war read as a huge camera swing. Skipping the
   warp (verified firing via `peer_in_remote`) did **not** fix it either.
3. **Unreliable game detection.** Roblox's Hyperion anti-cheat blocks process
   introspection, so `game_foreground()` flaps — MineShare cannot reliably even
   *detect* the game to adapt.

Conclusion: the whole **cursor-crossing / edge-detect / warp / handover /
game-lock** machinery is architecturally incompatible with driving a
cursor-locked game on the peer. Patching its parts is whack-a-mole.

## Goal

A distinct, explicit **Game Drive** mode that **bypasses cursor-crossing
entirely**: while active, the local machine forwards raw relative mouse +
keyboard continuously to the peer, and the peer injects pure relative input
with no warp, no edge-handover, and no game-lock interference. Toggled by the
user (Ctrl+Alt+G + UI button). It is both the fix and the definitive test of
whether the anti-cheat itself mangles injected input (if pure-relative is
smooth, it works; if it still flings, Hyperion is the wall — and we will know).

User decisions (confirmed):
- **Activation:** hotkey **Ctrl+Alt+G** + a UI toggle button.
- **Input scope:** mouse **and** keyboard (so the game is fully playable).
- **Ban risk:** a non-blocking **status indicator** only (the user opts in
  knowingly; no confirmation dialog).

## Design

### State & lifecycle

A new per-process state in `mineshare-input`:

```
enum GameDrive { Off, Driving, Receiving }
```

backed by an atomic (e.g. `AtomicU8`). Helpers: `game_drive()`,
`set_game_drive(state)`, `is_game_driving()`, `is_game_receiving()`.

- **Toggle (Ctrl+Alt+G or UI):** on a machine with a connected peer, `Off →
  Driving` locally **and** send `ControlMsg::GameDrive { active: true }`; the
  peer sets `Receiving`. Toggling again (or the peer toggling) sends
  `active: false` and both return to `Off`.
- **While `Driving` or `Receiving`:** the normal cursor-crossing edge-detection
  is suspended (no accidental cross), and the game-lock auto-engage
  (`game_detect_thread`) is suppressed — the status indicator carries the
  ban-risk warning instead of locking.
- **Auto-exit:** on peer disconnect or app shutdown, force `Off` on both sides
  and call `release_all_held` so no key/button stays logically pressed.

### Wire protocol

Add a variant to the existing `ControlMsg` enum (`mineshare-net`):

```rust
ControlMsg::GameDrive { active: bool }
```

The `Driving` side sends it on enter/exit. The receiver applies it:
`active: true → set_game_drive(Receiving)`, `false → set_game_drive(Off) +
release_all_held`. Older peers that don't understand the variant: the daemon's
control-message decoder already tolerates unknown variants by logging+ignoring
(verify); if not, gate emission on `peer_version` capability.

### Controller behavior (`Driving`)

Reuses the existing capture/forward path, made unconditional:

- **Pin the local cursor** so it doesn't wander on the controller's screen
  (reuse the REMOTE-mode `SetCursorPos`-to-anchor each event).
- **Forward continuously**: raw relative mouse motion, buttons, scroll —
  through the existing coalesce+forward pipeline, with **edge-exit detection
  disabled** (exit only via the toggle). No `virt_x` / threshold logic runs.
- **Forward keyboard**: force `keyboard_target = peer` for the duration
  (override Smart routing); restore the previous target on exit.

### Receiver behavior (`Receiving`)

Reuses `EnigoInject::dispatch` (already clamps to `MAX_INJECT_DELTA_PX = 200`
as a safety net):

- Inject pure relative mouse + buttons + scroll + keys.
- **No warp**: `on_peer_take_control` is not invoked for a Game Drive session
  (there is no take-control handover — the mode is entered explicitly), so no
  `SetCursorPos` fights the game.
- **Game-lock does not interfere**: it already does not gate injection; while
  `Receiving`, also suppress its auto-engage so it doesn't churn.

### Keyboard

While `Driving`, all key events forward to the peer regardless of cursor/Smart
state. Implement as a forced branch in `should_forward_keys` (or an explicit
override) keyed on `is_game_driving()`. The receiver injects them via the
existing key path; `release_all_held` on exit prevents stuck WASD.

### UI + status indicator

- **`get_status`** gains a field, e.g. `game_drive: "off" | "driving" |
  "receiving"`.
- **A toggle** (button/pill) calls a new Tauri command `toggle_game_drive`
  (→ `mineshare_input` / daemon to set state + send the ControlMsg). The hotkey
  path and the button converge on the same daemon entry point.
- **A persistent indicator** while active: `🎮 Driving peer's game` (controller)
  / `🎮 Peer is driving (game)` (receiver), each with a small muted caption:
  *"injected input — an anti-cheat may flag it."* No blocking dialog.

### Error handling / edges

- Toggle with no peer connected: no-op + a transient UI hint.
- Peer disconnect mid-session: auto-exit both sides, release held inputs.
- Mutual exclusion with the manual game-lock (Ctrl+Alt+L): entering Game Drive
  clears/overrides any auto game-lock for the session; they cannot both pin
  input.

### Testing

- **Unit (`mineshare-input`):** state transitions (`Off↔Driving↔Receiving`);
  `ControlMsg::GameDrive` application sets the right state + releases held on
  stop; `should_forward_keys` forces peer while `Driving`; edge-exit is
  inert while game-driving.
- **Manual (the decisive test):** with Game Drive on, drive the peer's Roblox
  with the local mouse+keyboard — confirm the camera moves smoothly (no fling)
  and WASD works. A smooth result confirms pure-relative injection is viable; a
  persistent fling isolates the cause to the anti-cheat itself.

## Non-goals

- No change to the normal cursor-crossing model for desktop use.
- No per-game profiles or auto-activation — purely user-toggled.
- No attempt to evade or defeat anti-cheat; the indicator simply discloses the
  risk.

## Risks

- **Anti-cheat ban risk.** Driving an anti-cheat game (Roblox/Hyperion) via
  injected input may flag/ban the account. Surfaced via the indicator; the user
  opts in. This mode does not reduce that risk — it only removes MineShare's
  *self-inflicted* fling.
- **Hyperion may still mangle injected input** even with clean pure-relative
  motion. If so, this mode will reveal it (smooth vs. still-flinging), and the
  honest outcome is "not viable for this title" — accepted.
- **Both machines must run a build with the new `ControlMsg` variant.** Gate on
  `peer_version` if the decoder isn't already forward-compatible.
