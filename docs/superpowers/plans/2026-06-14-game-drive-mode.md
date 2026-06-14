# Game Drive Mode — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a user-toggled "Game Drive" mode that forwards raw relative mouse + keyboard continuously to the peer (bypassing cursor-crossing, warp, handover, and game-lock) so a cursor-locked game running on the peer can be driven smoothly from the local machine.

**Architecture:** A new `GameDrive { Off, Driving, Receiving }` state in `mineshare-input` gates behavior on both ends. A Ctrl+Alt+G hotkey (and a UI button) toggles it; the controller fires a `RemoteEvent` that the daemon converts to a new `ControlMsg::GameDrive { active }` sent to the peer. While active, edge-detection / warp / game-lock are suppressed and keyboard is forced to the peer; the receiver injects pure-relative input through the existing clamped `dispatch`. Spec: `docs/superpowers/specs/2026-06-14-game-drive-mode-design.md`.

**Tech Stack:** Rust workspace (`mineshare-input`, `mineshare-daemon`, `mineshare-net`), atomics for cross-thread state, Win32 low-level hooks (Ctrl+Alt+G), Tauri command + React (`ui/`). Tests: `cargo test -p <crate>` (existing test modules in `mineshare-input`). OS-hook / UI wiring verified by build + manual test.

---

## File Structure

- `crates/mineshare-input/src/lib.rs` — **modify**: `GameDrive` enum + atomic + accessors (`game_drive`, `set_game_drive`, `is_game_driving`, `is_game_receiving`); two new `RemoteEvent` variants; `should_forward_keys` forces peer while driving; suppress edge/game-lock helpers consult the state.
- `crates/mineshare-input/src/windows.rs` — **modify**: Ctrl+Alt+G hotkey; suppress edge-crossing & auto game-lock while game-drive active; revert the `peer_in_remote` part of `should_anchor_cursor` (keep desktop warp).
- `crates/mineshare-input/src/linux.rs` — **modify**: mirror the suppression hooks (Linux is cfg-gated off on Windows; edit per plan, build on the peer/CI).
- `crates/mineshare-daemon/src/runtime.rs` — **modify**: `ControlMsg::GameDrive { active }` variant; `RemoteEvent → ControlMsg` mapping; receiver handling (set state + `release_all_held` on stop); auto-exit on disconnect; `toggle_game_drive` entry point.
- `crates/mineshare-daemon/src/status.rs` — **modify**: add `game_drive: &'static str` to the status snapshot.
- `ui/src-tauri/src/lib.rs` — **modify**: `toggle_game_drive` Tauri command + register it; `game_drive` in the `Status` mirror.
- `ui/src/types.ts`, `ui/src/pages/Home.tsx`, `ui/src/i18n.tsx` — **modify**: status field, toggle button + indicator, strings.

---

## Phase 1 — Core state + API (`mineshare-input`)

### Task 1: `GameDrive` state enum + atomic accessors (TDD)

**Files:**
- Modify: `crates/mineshare-input/src/lib.rs`

- [ ] **Step 1: Write the failing test.** Add to the `#[cfg(test)] mod tests` in `lib.rs`:

```rust
#[test]
fn game_drive_state_roundtrips() {
    let _g = TEST_LOCK.lock();
    set_game_drive(GameDrive::Off);
    assert_eq!(game_drive(), GameDrive::Off);
    assert!(!is_game_driving());
    assert!(!is_game_receiving());

    set_game_drive(GameDrive::Driving);
    assert_eq!(game_drive(), GameDrive::Driving);
    assert!(is_game_driving());
    assert!(!is_game_receiving());

    set_game_drive(GameDrive::Receiving);
    assert!(!is_game_driving());
    assert!(is_game_receiving());

    set_game_drive(GameDrive::Off); // reset for other tests
}
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test -p mineshare-input game_drive_state_roundtrips`
Expected: FAIL — `GameDrive` / `set_game_drive` not found.

- [ ] **Step 3: Implement the state.** Near the other input-layer atomics in `lib.rs`:

```rust
use std::sync::atomic::AtomicU8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameDrive {
    /// Normal cursor-crossing behavior.
    Off = 0,
    /// This machine forwards raw mouse+keyboard continuously to the peer.
    Driving = 1,
    /// This machine injects the peer's pure-relative input (game runs here).
    Receiving = 2,
}

static GAME_DRIVE: AtomicU8 = AtomicU8::new(0);

pub fn game_drive() -> GameDrive {
    match GAME_DRIVE.load(Ordering::Acquire) {
        1 => GameDrive::Driving,
        2 => GameDrive::Receiving,
        _ => GameDrive::Off,
    }
}

pub fn set_game_drive(s: GameDrive) {
    GAME_DRIVE.store(s as u8, Ordering::Release);
}

pub fn is_game_driving() -> bool {
    matches!(game_drive(), GameDrive::Driving)
}

pub fn is_game_receiving() -> bool {
    matches!(game_drive(), GameDrive::Receiving)
}
```

- [ ] **Step 4: Run it, expect PASS**

Run: `cargo test -p mineshare-input game_drive_state_roundtrips`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mineshare-input/src/lib.rs
git commit -m "feat(input): GameDrive state (Off/Driving/Receiving) + accessors"
```

### Task 2: `RemoteEvent` variants for the toggle (TDD)

**Files:**
- Modify: `crates/mineshare-input/src/lib.rs`

- [ ] **Step 1: Write the failing test.** Add to the test module:

```rust
#[test]
fn game_drive_remote_events_exist() {
    // Compile-time proof the toggle signal variants exist and are distinct.
    let a = RemoteEvent::GameDriveStart;
    let b = RemoteEvent::GameDriveStop;
    assert_ne!(format!("{a:?}"), format!("{b:?}"));
}
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test -p mineshare-input game_drive_remote_events_exist`
Expected: FAIL — `GameDriveStart` not a variant.

- [ ] **Step 3: Add the variants.** In the `RemoteEvent` enum (`lib.rs:134`) add:

```rust
    /// User toggled Game Drive ON locally → ask the peer to start receiving.
    /// Translates to `ControlMsg::GameDrive { active: true }`.
    GameDriveStart,
    /// User toggled Game Drive OFF → ask the peer to stop receiving.
    /// Translates to `ControlMsg::GameDrive { active: false }`.
    GameDriveStop,
```

- [ ] **Step 4: Run it, expect PASS**

Run: `cargo test -p mineshare-input game_drive_remote_events_exist`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/mineshare-input/src/lib.rs
git commit -m "feat(input): RemoteEvent::GameDriveStart/Stop signals"
```

### Task 3: Force keyboard to peer while driving (TDD)

**Files:**
- Modify: `crates/mineshare-input/src/lib.rs`

- [ ] **Step 1: Write the failing test.** `should_forward_keys` currently decides routing from cursor/Smart state. While `Driving`, it must always forward to the peer. Add:

```rust
#[test]
fn game_driving_forces_keys_to_peer() {
    let _g = TEST_LOCK.lock();
    reset();
    set_keyboard_target(KeyboardTarget::ForceLocal); // even pinned-local…
    set_game_drive(GameDrive::Driving);
    assert!(should_forward_keys(false), "Driving must forward keys to peer");
    set_game_drive(GameDrive::Off);
    assert!(!should_forward_keys(false), "Off + ForceLocal stays local");
}
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test -p mineshare-input game_driving_forces_keys_to_peer`
Expected: FAIL — driving does not yet force forwarding.

- [ ] **Step 3: Implement.** At the very top of `should_forward_keys` (`lib.rs:666`), before any other logic:

```rust
pub fn should_forward_keys(cursor_in_remote: bool) -> bool {
    // Game Drive: while we are driving the peer's game, every key goes to
    // the peer regardless of cursor position or Smart routing.
    if is_game_driving() {
        return true;
    }
    // ... existing logic unchanged ...
```

- [ ] **Step 4: Run it, expect PASS**

Run: `cargo test -p mineshare-input game_driving_forces_keys_to_peer && cargo test -p mineshare-input --lib`
Expected: PASS, no regressions.

- [ ] **Step 5: Commit**

```bash
git add crates/mineshare-input/src/lib.rs
git commit -m "feat(input): force keyboard to peer while Game Driving"
```

---

## Phase 2 — Wire protocol + daemon (`mineshare-daemon`)

### Task 4: `ControlMsg::GameDrive` + send mapping

**Files:**
- Modify: `crates/mineshare-daemon/src/runtime.rs`

- [ ] **Step 1: Add the variant.** In the `ControlMsg` enum (`runtime.rs:82`) add:

```rust
    /// Toggle the peer into / out of Game Drive "Receiving" — it should
    /// inject our pure-relative input and suppress its own cursor-crossing.
    GameDrive { active: bool },
```

- [ ] **Step 2: Map the RemoteEvent → ControlMsg.** In the send loop where `RemoteEvent::Entered => ControlMsg::TakeControl` is matched (`runtime.rs:890`), add:

```rust
                    Some(mineshare_input::RemoteEvent::GameDriveStart) => {
                        ControlMsg::GameDrive { active: true }
                    }
                    Some(mineshare_input::RemoteEvent::GameDriveStop) => {
                        ControlMsg::GameDrive { active: false }
                    }
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p mineshare-daemon`
Expected: clean (the match is now exhaustive over the new RemoteEvent variants).

- [ ] **Step 4: Commit**

```bash
git add crates/mineshare-daemon/src/runtime.rs
git commit -m "feat(daemon): ControlMsg::GameDrive + RemoteEvent mapping"
```

### Task 5: Receiver applies `ControlMsg::GameDrive`

**Files:**
- Modify: `crates/mineshare-daemon/src/runtime.rs`

- [ ] **Step 1: Handle the inbound message.** In the reader match where `Ok(ControlMsg::TakeControl) => { ... }` lives (`runtime.rs:1004`), add an arm. Use the same `inject` handle the reader already holds (`inject_recv`, used at line 1129):

```rust
                Ok(ControlMsg::GameDrive { active }) => {
                    if active {
                        mineshare_input::set_game_drive(mineshare_input::GameDrive::Receiving);
                        tracing::info!("game-drive: peer started driving — receiving");
                    } else {
                        mineshare_input::set_game_drive(mineshare_input::GameDrive::Off);
                        // Drop any keys/buttons the peer left held so WASD
                        // doesn't stick when the game session ends.
                        let _ = inject_recv.release_all_held();
                        tracing::info!("game-drive: peer stopped driving");
                    }
                }
```

- [ ] **Step 2: Verify build**

Run: `cargo build -p mineshare-daemon`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/mineshare-daemon/src/runtime.rs
git commit -m "feat(daemon): apply ControlMsg::GameDrive on the receiver"
```

### Task 6: Auto-exit Game Drive on disconnect

**Files:**
- Modify: `crates/mineshare-daemon/src/runtime.rs`

- [ ] **Step 1: Reset on session teardown.** At the same place the reader resets peer state on disconnect (`set_peer_in_remote(false)` at `runtime.rs:1362`), add:

```rust
    mineshare_input::set_game_drive(mineshare_input::GameDrive::Off);
```

- [ ] **Step 2: Verify build**

Run: `cargo build -p mineshare-daemon`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/mineshare-daemon/src/runtime.rs
git commit -m "feat(daemon): clear Game Drive state on peer disconnect"
```

---

## Phase 3 — Behavior suppression (`mineshare-input`)

### Task 7: Suppress edge-crossing & auto game-lock while game-drive active

**Files:**
- Modify: `crates/mineshare-input/src/windows.rs`, `crates/mineshare-input/src/linux.rs`

- [ ] **Step 1: Guard the Windows edge-cross detector.** In `windows.rs` where `crossed_edge` is computed (`windows.rs:910`, currently `!super::is_input_locked() && ...`), also require not game-driving so accidental edge motion never crosses while driving a game:

```rust
                let crossed_edge = !super::is_input_locked()
                    && super::game_drive() == super::GameDrive::Off
                    && /* …existing edge condition… */ ;
```

(Read the existing full expression at that line and insert the `game_drive() == Off` conjunct; do not drop any existing term.)

- [ ] **Step 2: Suppress auto game-lock while game-drive active.** In `game_detect_thread` (`windows.rs:631`) gate the auto-engage so it does not churn/fight while the user has deliberately opted into Game Drive:

```rust
        let should_lock = (cursor_hidden || cursor_clipped || anticheat_match.is_some())
            && super::game_drive() == super::GameDrive::Off;
```

Keep the existing `GAME_FOREGROUND.store(...)` line as-is (it now stores this gated value, which is fine — `on_peer_take_control` doesn't run during Game Drive anyway).

- [ ] **Step 3: Revert the `peer_in_remote` warp-skip (keep desktop warp).** In `should_anchor_cursor` (`windows.rs`), drop the `peer_in_remote` term added during debugging — Game Drive now owns the game case, and unconditional anchoring would regress normal desktop crossing's exit hysteresis:

```rust
fn should_anchor_cursor(game_foreground: bool, _peer_in_remote: bool) -> bool {
    // Skip the warp only when a cursor-locked game owns the cursor. The
    // game-drive path no longer relies on this (it never warps), so do NOT
    // anchor for every peer-driven session — desktop crossing needs the warp.
    game_foreground
}
```

(The existing call site and `anchor_cursor_when_driven_or_game` test: update that test's `should_anchor_cursor(false, true)` expectation from `true` to `false`, and rename it to `anchor_cursor_only_when_game_foreground`.)

- [ ] **Step 4: Mirror on Linux.** In `linux.rs`, apply the same two guards: the edge-cross detection and the auto game-lock equivalent must also check `super::game_drive() == super::GameDrive::Off`. (Linux is cfg-gated off on a Windows host — edit per the plan and note it needs a Linux/CI build.)

- [ ] **Step 5: Verify**

Run (Windows): `cargo test -p mineshare-input --lib && cargo build -p mineshare-input`
Expected: tests pass (incl. the renamed anchor test), clean build. linux.rs not compiled here.

- [ ] **Step 6: Commit**

```bash
git add crates/mineshare-input/src/windows.rs crates/mineshare-input/src/linux.rs
git commit -m "feat(input): suppress edge-cross + auto game-lock during Game Drive; revert peer_in_remote warp-skip"
```

### Task 8: Continuous forward while Driving

**Files:**
- Modify: `crates/mineshare-input/src/windows.rs`, `crates/mineshare-input/src/linux.rs`

The capture path forwards motion only while `CURSOR_MODE == REMOTE`. Game Drive must forward continuously without entering the crossing state machine.

- [ ] **Step 1: Forward when driving OR remote.** In the Windows raw-input / motion-accumulate path, the gate that decides whether to forward+pin (the `CURSOR_MODE == MODE_REMOTE` checks around the flush watchdog `windows.rs:242` and the capture site) must also fire when `super::is_game_driving()`. Introduce a local helper at the top of `windows.rs`:

```rust
fn forwarding_active() -> bool {
    CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE || super::is_game_driving()
}
```

Replace the `CURSOR_MODE.load(...) == MODE_REMOTE` guards on the **forward/pin** path (flush watchdog + the per-event capture-and-pin block) with `forwarding_active()`. Do NOT change the edge-cross or `virt_x` logic (that stays REMOTE-only and is additionally suppressed by Task 7).

- [ ] **Step 2: Mirror on Linux** — the equivalent capture/forward gate in `linux.rs` also forwards when `super::is_game_driving()`.

- [ ] **Step 3: Verify build**

Run: `cargo build -p mineshare-input`
Expected: clean (Windows). linux.rs noted for CI.

- [ ] **Step 4: Commit**

```bash
git add crates/mineshare-input/src/windows.rs crates/mineshare-input/src/linux.rs
git commit -m "feat(input): forward mouse continuously while Game Driving"
```

---

## Phase 4 — Hotkey (Ctrl+Alt+G)

### Task 9: Ctrl+Alt+G toggle + local entry point

**Files:**
- Modify: `crates/mineshare-input/src/lib.rs`, `crates/mineshare-input/src/windows.rs`

- [ ] **Step 1: Add a shared toggle fn in `lib.rs`** that both the hotkey and the Tauri command call:

```rust
/// Toggle Game Drive from this machine. When turning ON we become `Driving`
/// and signal the peer to `Receiving`; when turning OFF we reset and signal
/// stop. No-op semantics if there is no peer are handled by the daemon
/// (the RemoteEvent simply isn't delivered).
pub fn toggle_game_drive() {
    if is_game_driving() {
        set_game_drive(GameDrive::Off);
        fire_remote_event(RemoteEvent::GameDriveStop);
    } else {
        set_game_drive(GameDrive::Driving);
        fire_remote_event(RemoteEvent::GameDriveStart);
    }
}
```

(`fire_remote_event` is `pub(crate)` at `lib.rs:944`; it is in scope here in `lib.rs`.)

- [ ] **Step 2: Register the hotkey.** In `windows.rs`, alongside the Ctrl+Alt+R/L/K handlers (the `if down && scan == SCAN_HOTKEY && MOD_CTRL && MOD_ALT` blocks near `windows.rs:1147`), add a block for the `G` scancode. Add a `const SCAN_G: u32 = 0x22;` (set-1 scancode for `G`) near the other scan consts, then:

```rust
        // Hotkey: Ctrl+Alt+G toggles Game Drive (drive the peer's game with
        // pure-relative mouse + keyboard, no cursor-crossing).
        if down
            && scan == SCAN_G
            && MOD_CTRL.load(Ordering::Relaxed)
            && MOD_ALT.load(Ordering::Relaxed)
        {
            info!("hotkey Ctrl+Alt+G — toggling Game Drive");
            super::toggle_game_drive();
            return LRESULT(1);
        }
```

(Verify `SCAN_G = 0x22` against the existing scancode convention used for R/L/K in this file; adjust if the file uses VK codes instead.)

- [ ] **Step 3: Verify build**

Run: `cargo build -p mineshare-input`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mineshare-input/src/lib.rs crates/mineshare-input/src/windows.rs
git commit -m "feat(input): Ctrl+Alt+G hotkey toggles Game Drive"
```

---

## Phase 5 — Status + UI

### Task 10: Surface `game_drive` in status

**Files:**
- Modify: `crates/mineshare-daemon/src/status.rs`, `ui/src-tauri/src/lib.rs`

- [ ] **Step 1: Add the field.** In the status struct (`status.rs:53` area, beside `input_locked`) add:

```rust
    /// "off" | "driving" | "receiving" — Game Drive state for the GUI.
    pub game_drive: &'static str,
```

And in the builder (`status.rs:98` area) set it:

```rust
        game_drive: match mineshare_input::game_drive() {
            mineshare_input::GameDrive::Driving => "driving",
            mineshare_input::GameDrive::Receiving => "receiving",
            mineshare_input::GameDrive::Off => "off",
        },
```

- [ ] **Step 2: Mirror in the Tauri Status type.** In `ui/src-tauri/src/lib.rs` the `Status` struct mirror (near `keyboard_target`) add `game_drive: String` (or `&'static str` matching the daemon snapshot serialization).

- [ ] **Step 3: Verify build**

Run: `cargo build -p mineshare-daemon && cargo check --manifest-path ui/src-tauri/Cargo.toml`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mineshare-daemon/src/status.rs ui/src-tauri/src/lib.rs
git commit -m "feat: surface game_drive in status snapshot"
```

### Task 11: `toggle_game_drive` Tauri command

**Files:**
- Modify: `ui/src-tauri/src/lib.rs`

- [ ] **Step 1: Add the command** (mirror `set_input_lock` at `ui/src-tauri/src/lib.rs:61`):

```rust
#[tauri::command]
fn toggle_game_drive() {
    mineshare_input::toggle_game_drive();
}
```

Register `toggle_game_drive` in the `tauri::generate_handler![ ... ]` list.

- [ ] **Step 2: Verify build**

Run: `cargo check --manifest-path ui/src-tauri/Cargo.toml`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add ui/src-tauri/src/lib.rs
git commit -m "feat(ui): toggle_game_drive Tauri command"
```

### Task 12: UI toggle button + status indicator

**Files:**
- Modify: `ui/src/types.ts`, `ui/src/pages/Home.tsx`, `ui/src/i18n.tsx`

- [ ] **Step 1: Add the status field type.** In `ui/src/types.ts`, add to the `Status` type: `game_drive: "off" | "driving" | "receiving";`.

- [ ] **Step 2: Add a Game Drive card/button to Home.** In `ui/src/pages/Home.tsx`, add a component rendered under the mode pills:

```tsx
function GameDriveCard({ s }: { s: Status }) {
  const { t } = useT();
  const state = s.game_drive;
  const active = state !== "off";
  return (
    <div className={"rounded-xl border p-4 mb-6 flex items-center justify-between " +
      (active ? "border-purple-500/30 bg-purple-500/10" : "border-white/[0.08] bg-ds-surface")}>
      <div className="min-w-0">
        <p className={"text-sm font-semibold " + (active ? "text-purple-300" : "text-slate-100")}>
          {state === "driving" ? "🎮 " + t("gd_driving")
            : state === "receiving" ? "🎮 " + t("gd_receiving")
            : t("gd_title")}
        </p>
        <p className="text-xs text-slate-400 mt-1 max-w-prose leading-relaxed">
          {t("gd_desc")} <kbd className="font-mono text-slate-300 bg-white/[0.08] px-1 py-0.5 rounded text-[10px]">Ctrl+Alt+G</kbd>.
          {active ? " " + t("gd_anticheat_note") : ""}
        </p>
      </div>
      <button
        onClick={() => invoke("toggle_game_drive")}
        disabled={!s.peer_connected}
        className={"rounded-lg px-4 py-2 text-sm font-medium shrink-0 ml-4 " +
          (active ? "bg-purple-500 hover:bg-purple-400 text-white"
                  : "border border-white/[0.10] bg-white/[0.04] hover:bg-white/[0.08] text-slate-300 disabled:opacity-40")}
      >
        {active ? t("gd_stop") : t("gd_start")}
      </button>
    </div>
  );
}
```

Render `<GameDriveCard s={status} />` in `HomePage` (after `GameLockCard`). Import `invoke` and `useT` if not already in `Home.tsx`.

- [ ] **Step 3: Add i18n keys** to both `en` and `th` in `ui/src/i18n.tsx`:

```tsx
gd_title: "Game Drive — off",
gd_driving: "Driving peer's game",
gd_receiving: "Peer is driving (game)",
gd_desc: "Drive a game running on the peer with this machine's mouse + keyboard (pure relative, no cursor crossing). Toggle with",
gd_anticheat_note: "Injected input — an anti-cheat may flag it.",
gd_start: "Start",
gd_stop: "Stop",
```

- [ ] **Step 4: Verify**

Run: `cd ui && npx tsc -b`
Expected: clean.
Manual: button disabled when no peer; clicking toggles; indicator turns purple and shows the right label on each side; anti-cheat note appears while active.

- [ ] **Step 5: Commit**

```bash
git add ui/src/types.ts ui/src/pages/Home.tsx ui/src/i18n.tsx
git commit -m "feat(ui): Game Drive toggle button + status indicator"
```

---

## Phase 6 — End-to-end verification

### Task 13: Build, deploy to both machines, manual game test

- [ ] **Step 1: Build the app on both machines** (Windows) and the Linux peer if applicable:

Run (each Windows machine): `cd ui && bun run tauri build` (or `cargo build --release --manifest-path ui/src-tauri/Cargo.toml` for a quick exe), then install/replace the running `mineshare-app.exe`.
Both machines MUST run the new build (new `ControlMsg` variant).

- [ ] **Step 2: The decisive manual test.** Open the game (Roblox) on the machine where it runs. From the other machine, press **Ctrl+Alt+G** (or the UI button). Confirm:
  - The indicator shows `Driving` (controller) / `Peer is driving (game)` (receiver).
  - Mouse-look camera moves **smoothly** (no fling), and WASD controls the character.
  - Pressing Ctrl+Alt+G again exits cleanly; no keys stay stuck.

- [ ] **Step 3: Record the outcome.** A smooth result confirms pure-relative injection works (the cursor-crossing machinery was the cause). A persistent fling isolates the cause to the anti-cheat itself (Hyperion) — documented as "not viable for this title", which is the honest conclusion.

---

## Self-Review

**Spec coverage:**
- `GameDrive { Off, Driving, Receiving }` state: Task 1. ✓
- Ctrl+Alt+G + UI toggle: Tasks 9, 11, 12. ✓
- ControlMsg::GameDrive wire protocol + send/receive: Tasks 4, 5. ✓
- Controller continuous forward + cursor pin (reuses REMOTE pin via `forwarding_active`): Task 8. ✓
- Receiver pure-relative inject, no warp, no game-lock interference: Tasks 5, 7 (clamp safety net already in `dispatch`). ✓
- Keyboard forced to peer while driving: Task 3. ✓
- Suspend edge-crossing + game-lock while active: Task 7. ✓
- UI status indicator + ban-risk note (non-blocking): Tasks 10, 12. ✓
- Auto-exit on disconnect + release held: Tasks 5 (stop), 6 (disconnect). ✓
- Revert the debugging `peer_in_remote` warp-skip; keep clamp: Task 7 Step 3. ✓
- Unit tests for state, RemoteEvent, keyboard force: Tasks 1–3, 7. ✓

**Placeholder scan:** No TODO/TBD. Two explicit "verify against this file's convention" notes (SCAN_G value; ControlMsg decoder forward-compat) are real verification steps with the concrete value/path given, not deferred work.

**Type consistency:** `GameDrive`/`game_drive()`/`set_game_drive`/`is_game_driving`/`is_game_receiving` consistent across input, daemon, status. `RemoteEvent::GameDriveStart/Stop` ↔ `ControlMsg::GameDrive { active }` mapping consistent (Tasks 2, 4, 5). `game_drive` status string `"off"|"driving"|"receiving"` consistent across `status.rs`, the Tauri mirror, `types.ts`, and the UI. `forwarding_active()` (Task 8) and the Task 7 edge guard both key off `game_drive()`.

**Note:** linux.rs edits (Tasks 7, 8) are not compile-verified on a Windows host — build on the Ubuntu peer / CI before shipping the Linux side.
