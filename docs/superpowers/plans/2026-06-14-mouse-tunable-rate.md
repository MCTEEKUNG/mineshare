# Mouse Tunable Rate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user set the mouse forward/inject rate (60–1000 Hz) from an in-app slider with a live readout of the achieved rate, backed by a runtime-settable flush interval both OSes honor; lay the gated groundwork for true in-game 1000 Hz via a batch-delta wire variant.

**Architecture:** Phase 1 keeps the existing summed-delta model but turns the compile-time `FLUSH_INTERVAL_MS` into a runtime atomic driven through the existing `Settings` → `push_to_input_layer` plumbing, plus measured-rate telemetry surfaced to the GUI. Phase 2 (gated on a feasibility spike) adds an `InputEvent::MouseMoveBatch` variant replayed as N injects with version-negotiated fallback. Spec: `docs/superpowers/specs/2026-06-14-mouse-tunable-rate-design.md`.

**Tech Stack:** Rust workspace (`mineshare-input`, `mineshare-daemon`), atomics for cross-thread runtime config, Tauri commands `get_settings`/`set_settings` (already exist), React slider in `ui/src/pages/Settings.tsx`. Tests: `cargo test -p <crate>` (existing test modules in `mineshare-input`).

---

## File Structure

- `crates/mineshare-daemon/src/settings.rs` — **modify**: add `mouse_rate_hz` field (default 500, clamp 60..=1000); push to input layer.
- `crates/mineshare-input/src/lib.rs` — **modify**: add `TARGET_FLUSH_US` atomic + `set_mouse_rate_hz()` setter + `hz_to_flush_us()` pure helper + measured-rate telemetry counters and a `mouse_rate_stats()` accessor.
- `crates/mineshare-input/src/windows.rs` — **modify**: read the runtime interval instead of `const FLUSH_INTERVAL_MS`; micro-sleep for sub-ms; count forwarded events.
- `crates/mineshare-input/src/linux.rs` — **modify**: same for the inject coalescer (lift the 8 ms floor); count injected events.
- `ui/src-tauri/src/lib.rs` — **modify**: extend the `Settings` mirror type if one exists; add a `get_mouse_rate_stats` command.
- `ui/src/pages/Settings.tsx` — **modify**: add the Performance section with the rate slider + live readout (the UI plan left a stub here).
- Phase 2 only: `crates/mineshare-input/src/lib.rs` (new `MouseMoveBatch` variant + dispatch), `windows.rs`/`linux.rs` (batch produce/replay), version negotiation in the daemon.

---

## Phase 1 — Runtime-tunable rate + telemetry

### Task 1: Add `mouse_rate_hz` to Settings (TDD)

**Files:**
- Modify: `crates/mineshare-daemon/src/settings.rs`

- [ ] **Step 1: Write the failing test.** Append to `settings.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_rate_clamps_into_range() {
        let lo = Settings { mouse_rate_hz: 5, ..Settings::default() }.clamped();
        let hi = Settings { mouse_rate_hz: 9000, ..Settings::default() }.clamped();
        assert_eq!(lo.mouse_rate_hz, 60);
        assert_eq!(hi.mouse_rate_hz, 1000);
        assert_eq!(Settings::default().mouse_rate_hz, 500);
    }
}
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test -p mineshare-daemon mouse_rate_clamps_into_range`
Expected: FAIL — no field `mouse_rate_hz`.

- [ ] **Step 3: Add the field + default + clamp.** In the `Settings` struct add:

```rust
    /// Target mouse forward/inject rate in Hz. Drives the runtime
    /// flush interval in `mineshare-input` (both capture-forward on
    /// Windows and inject-coalesce on Linux). Clamped 60..=1000.
    #[serde(default = "default_mouse_rate_hz")]
    pub mouse_rate_hz: u32,
```

Add `fn default_mouse_rate_hz() -> u32 { 500 }`. In `Default::default()` set `mouse_rate_hz: 500`. In `clamped()` add `mouse_rate_hz: self.mouse_rate_hz.clamp(60, 1000),`.

- [ ] **Step 4: Push it into the input layer.** In `push_to_input_layer` add:

```rust
    mineshare_input::set_mouse_rate_hz(s.mouse_rate_hz);
```

(The function is created in Task 2; this line will not compile until then — implement Task 2 before building. If running tasks strictly independently, temporarily comment this line and uncomment in Task 2 Step 5.)

- [ ] **Step 5: Run the test, expect PASS**

Run: `cargo test -p mineshare-daemon mouse_rate_clamps_into_range`
Expected: PASS (after Task 2 provides the setter, or with the line temporarily commented).

- [ ] **Step 6: Commit**

```bash
git add crates/mineshare-daemon/src/settings.rs
git commit -m "feat(settings): add mouse_rate_hz (default 500, clamp 60..=1000)"
```

### Task 2: Runtime flush interval in the input crate (TDD)

**Files:**
- Modify: `crates/mineshare-input/src/lib.rs`

- [ ] **Step 1: Write the failing test.** Add to the existing `#[cfg(test)]` module in `lib.rs` (or create one):

```rust
#[test]
fn hz_maps_to_flush_micros() {
    assert_eq!(hz_to_flush_us(1000), 1_000);
    assert_eq!(hz_to_flush_us(500), 2_000);
    assert_eq!(hz_to_flush_us(125), 8_000);
    // clamp guards against div-by-zero / absurd values
    assert_eq!(hz_to_flush_us(0), hz_to_flush_us(60));
    assert_eq!(hz_to_flush_us(99999), hz_to_flush_us(1000));
}
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test -p mineshare-input hz_maps_to_flush_micros`
Expected: FAIL — `hz_to_flush_us` not found.

- [ ] **Step 3: Implement the atomic + setter + helper.** Near the other input-layer atomics (around the `MOUSE_SENS_BITS` block) in `lib.rs`:

```rust
use std::sync::atomic::AtomicU64;

/// Runtime flush interval in microseconds, derived from the user's
/// mouse-rate setting. Read each cycle by the Windows forward
/// watchdog and the Linux inject coalescer. Default 2000 us = 500 Hz.
static TARGET_FLUSH_US: AtomicU64 = AtomicU64::new(2_000);

/// Pure mapping Hz -> flush microseconds, clamped to 60..=1000 Hz.
pub(crate) fn hz_to_flush_us(hz: u32) -> u64 {
    let hz = hz.clamp(60, 1000) as u64;
    1_000_000 / hz
}

/// Set the target mouse rate. Called from `settings::push_to_input_layer`.
pub fn set_mouse_rate_hz(hz: u32) {
    TARGET_FLUSH_US.store(hz_to_flush_us(hz), Ordering::Relaxed);
}

/// Current flush interval in microseconds (read by platform flush loops).
pub(crate) fn target_flush_us() -> u64 {
    TARGET_FLUSH_US.load(Ordering::Relaxed)
}
```

- [ ] **Step 4: Run the test, expect PASS**

Run: `cargo test -p mineshare-input hz_maps_to_flush_micros`
Expected: PASS.

- [ ] **Step 5: Re-enable the daemon push.** Ensure the `mineshare_input::set_mouse_rate_hz(s.mouse_rate_hz);` line from Task 1 Step 4 is active. Build the workspace.

Run: `cargo build -p mineshare-daemon -p mineshare-input`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/mineshare-input/src/lib.rs
git commit -m "feat(input): runtime mouse-rate atomic + hz_to_flush_us mapping"
```

### Task 3: Consume the runtime interval on both platforms

**Files:**
- Modify: `crates/mineshare-input/src/windows.rs`, `crates/mineshare-input/src/linux.rs`

- [ ] **Step 1: Windows forward watchdog — use the runtime interval.** In `windows.rs`, in `start_motion_flush_watchdog`'s loop, replace the fixed `thread::sleep(Duration::from_millis(FLUSH_INTERVAL_MS))` and the `>= FLUSH_INTERVAL_MS` comparison with the runtime micros:

```rust
let flush_us = super::target_flush_us();
thread::sleep(std::time::Duration::from_micros(flush_us));
// ...
let now = super::now_ms();
let last = LAST_FLUSH_MS.load(Ordering::Relaxed);
// compare in micros where possible; ms granularity is fine for the residual-drain guard
if now.saturating_sub(last) * 1000 >= flush_us {
    flush_pending_motion();
}
```

Keep the `CURSOR_MODE == MODE_REMOTE` guard. Remove the now-unused `const FLUSH_INTERVAL_MS` (or keep as a documented fallback constant only if referenced elsewhere — grep first: `grep -n FLUSH_INTERVAL_MS crates/mineshare-input/src/windows.rs`).

- [ ] **Step 2: Windows high-resolution timer for >500 Hz.** Sustained sub-2 ms sleeps need a finer timer than the default ~15.6 ms tick. Bracket the forwarding loop with `timeBeginPeriod(1)` / `timeEndPeriod(1)` (from `windows::Win32::Media::timeBeginPeriod`) — request it when the watchdog thread starts and release on exit. Add a comment that this raises the system timer resolution (minor power cost) only while the bridge is active.

- [ ] **Step 3: Linux inject coalescer — use the runtime interval.** In `linux.rs`, replace the `FLUSH_INTERVAL_MS = 8` usages (the coalescer flush window and the `flush_pump` sleep) with `super::target_flush_us()` converted appropriately (`Duration::from_micros`, and compare accumulated time in micros). This lifts the 125 Hz floor that bottlenecks Win→Linux. Grep first: `grep -n FLUSH_INTERVAL_MS crates/mineshare-input/src/linux.rs` and convert each site.

- [ ] **Step 4: Verify build on both targets.**

Run (Windows host): `cargo build -p mineshare-input`
Run (cross/CI for Linux if available, else note for the Linux build): `cargo build -p mineshare-input --target x86_64-unknown-linux-gnu` (or build on the Ubuntu peer).
Expected: clean on the platform(s) available; the other compiles in CI.

- [ ] **Step 5: Commit**

```bash
git add crates/mineshare-input/src/windows.rs crates/mineshare-input/src/linux.rs
git commit -m "feat(input): honor runtime mouse-rate on Windows forward + Linux inject"
```

### Task 4: Measured-rate telemetry

**Files:**
- Modify: `crates/mineshare-input/src/lib.rs`, `crates/mineshare-input/src/windows.rs`, `crates/mineshare-input/src/linux.rs`, `ui/src-tauri/src/lib.rs`

- [ ] **Step 1: Add counters + accessor in `lib.rs`:**

```rust
static FWD_EVENTS: AtomicU64 = AtomicU64::new(0);   // forwarded MouseMove flushes (Windows)
static INJ_EVENTS: AtomicU64 = AtomicU64::new(0);   // injected mouse moves (Linux/receiver)

pub(crate) fn bump_fwd_events() { FWD_EVENTS.fetch_add(1, Ordering::Relaxed); }
pub(crate) fn bump_inj_events() { INJ_EVENTS.fetch_add(1, Ordering::Relaxed); }

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct MouseRateStats { pub set_hz: u32, pub fwd_total: u64, pub inj_total: u64 }

pub fn mouse_rate_stats() -> MouseRateStats {
    MouseRateStats {
        set_hz: (1_000_000 / target_flush_us().max(1)) as u32,
        fwd_total: FWD_EVENTS.load(Ordering::Relaxed),
        inj_total: INJ_EVENTS.load(Ordering::Relaxed),
    }
}
```

- [ ] **Step 2: Bump the counters.** In `windows.rs` `flush_pending_motion`, after a non-empty forward, call `super::bump_fwd_events()`. In `linux.rs` where the coalesced motion is actually injected (the `inject.mouse_move_rel` site), call `super::bump_inj_events()`.

- [ ] **Step 3: Expose via a Tauri command.** In `ui/src-tauri/src/lib.rs` add:

```rust
#[tauri::command]
fn get_mouse_rate_stats() -> mineshare_input::MouseRateStats {
    mineshare_input::mouse_rate_stats()
}
```

Register it in the `tauri::generate_handler![ ... ]` list.

- [ ] **Step 4: Verify**

Run: `cargo build -p mineshare-input && cargo build` (workspace) — clean.

- [ ] **Step 5: Commit**

```bash
git add crates/mineshare-input/src ui/src-tauri/src/lib.rs
git commit -m "feat(input): forwarded/injected mouse-event counters + get_mouse_rate_stats"
```

### Task 5: UI rate slider + live readout

**Files:**
- Modify: `ui/src/pages/Settings.tsx`

Depends on the UI plan having created `Settings.tsx` with the Performance-section stub. If that plan hasn't run yet, add the section to whichever page currently hosts settings (`Advanced.tsx`) following its existing `mouse_sensitivity` slider pattern.

- [ ] **Step 1: Add the Performance section** to `Settings.tsx`, mirroring the existing `mouse_sensitivity` slider in `Advanced.tsx` (uses `get_settings`/`set_settings`). The new control reads/writes `settings.mouse_rate_hz`:

```tsx
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type MouseRateStats = { set_hz: number; fwd_total: number; inj_total: number };

function PerformanceSection({ settings, update }: {
  settings: { mouse_rate_hz: number },
  update: (next: any) => void,
}) {
  const [stats, setStats] = useState<MouseRateStats | null>(null);
  const [prev, setPrev] = useState<{ fwd: number; inj: number; t: number } | null>(null);
  const [liveFwd, setLiveFwd] = useState<number>(0);
  const [liveInj, setLiveInj] = useState<number>(0);

  useEffect(() => {
    const id = setInterval(async () => {
      const s = await invoke<MouseRateStats>("get_mouse_rate_stats").catch(() => null);
      if (!s) return;
      const now = performance.now();
      if (prev) {
        const dt = (now - prev.t) / 1000;
        if (dt > 0) {
          setLiveFwd(Math.round((s.fwd_total - prev.fwd) / dt));
          setLiveInj(Math.round((s.inj_total - prev.inj) / dt));
        }
      }
      setPrev({ fwd: s.fwd_total, inj: s.inj_total, t: now });
      setStats(s);
    }, 1000);
    return () => clearInterval(id);
  }, [prev]);

  return (
    <div>
      <div className="flex items-baseline justify-between mb-2">
        <label className="text-sm text-ds-text">Mouse rate</label>
        <span className="font-mono text-sm text-ds-text">{settings.mouse_rate_hz} Hz</span>
      </div>
      <input
        type="range" min={60} max={1000} step={5}
        value={settings.mouse_rate_hz}
        onChange={(e) => update({ ...settings, mouse_rate_hz: parseInt(e.target.value, 10) })}
        list="mouse-rate-ticks"
        className="w-full"
      />
      <datalist id="mouse-rate-ticks">
        <option value="125" /><option value="250" /><option value="500" /><option value="1000" />
      </datalist>
      <p className="text-xs text-ds-text-muted mt-2">
        Live: forwarding ~{liveFwd} Hz · injecting ~{liveInj} Hz
        {stats ? ` · set ${stats.set_hz} Hz` : ""}
      </p>
      <p className="text-[11px] text-ds-text-muted mt-1">
        Higher rates feel smoother but use more network/CPU. If the live rate stays below your setting,
        your hardware or the peer's inject path is the limit. (Some anti-cheats ignore injected motion in games.)
      </p>
    </div>
  );
}
```

Wire `PerformanceSection` into the page using the same `get_settings`/`set_settings` load+update logic the existing settings page already implements (debounced writes on slider drag). Add the section under `section_performance` (i18n key added in the UI plan).

- [ ] **Step 2: Verify**

Run: `cd ui && npx tsc -b`
Manual: drag the slider 60↔1000; confirm the value persists across restart (written via `set_settings`); with a peer connected and the cursor driving the peer, confirm the live readout shows non-zero forward/inject rates that rise as you raise the slider and plateau at the hardware/OS ceiling.

- [ ] **Step 3: Commit**

```bash
git add ui/src/pages/Settings.tsx
git commit -m "feat(ui): mouse-rate slider with live forward/inject readout"
```

---

## Phase 2 — True in-game 1000 Hz (GATED)

> Do not start Phase 2 until Task 6 (the spike) shows the inject path sustains the target rate. If it can't, cap effective rate per platform and stop here — Phase 1 already delivers the user-facing slider + desktop smoothness.

### Task 6: Inject-throughput feasibility spike

**Files:**
- Create: `crates/mineshare-input/examples/inject_bench.rs`

- [ ] **Step 1: Write a standalone bench** that constructs the platform injector and calls `mouse_move_rel(1, 0)` (alternating +1/-1 to avoid drifting the cursor off-screen) in a tight loop targeting 250, 500, 1000 calls/sec for 5 s each, measuring achieved rate and max latency per call. Print a table.

- [ ] **Step 2: Run on each OS.**

Run (Windows): `cargo run -p mineshare-input --example inject_bench`
Run (Ubuntu peer): same.
Record the **max sustained inject rate without growing latency** per OS in the PR description.

- [ ] **Step 3: Decision gate.** If both platforms sustain ≥ ~900/sec cleanly, proceed to Task 7. If not, set a per-platform cap (store it, clamp `mouse_rate_hz`'s effective value, and surface the cap in the live readout text) and **stop** — commit the spike results and the cap.

```bash
git add crates/mineshare-input/examples/inject_bench.rs
git commit -m "test(input): inject-throughput feasibility bench (Phase 2 gate)"
```

### Task 7: `MouseMoveBatch` wire variant + replay + fallback (only if Task 6 passes)

**Files:**
- Modify: `crates/mineshare-input/src/lib.rs`, `windows.rs`, `linux.rs`, daemon version-negotiation site.

- [ ] **Step 1: Add the variant + dispatch.** In `lib.rs`:

```rust
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseMoveBatch { deltas: Vec<(i16, i16)> }, // individual per-tick deltas
    MouseButton { btn: Button, down: bool },
    Key { code: KeyCode, down: bool },
    Scroll { dx: f32, dy: f32 },
}
```

In `dispatch`, handle the new arm by replaying each delta as a separate inject and bumping `bump_inj_events()` per delta:

```rust
InputEvent::MouseMoveBatch { deltas } => {
    for (dx, dy) in deltas {
        self.mouse_move_rel(dx as i32, dy as i32)?;
        crate::bump_inj_events();
    }
    Ok(())
}
```

- [ ] **Step 2: Produce batches on Windows** when a "high-rate" mode is active (set rate > a threshold, e.g. > 500 Hz, AND peer supports batch): instead of summing into `PENDING_DX/DY`, push each raw delta into a small bounded `Vec` and flush it as `MouseMoveBatch` at the flush interval. Cap the batch length to avoid unbounded packets.

- [ ] **Step 3: Version-negotiate.** Using the existing `peer_version` exchange, gate batch emission on the peer advertising support (bump a capability/version). If the peer is older or capability is absent, **emit the summed `MouseMove`** path exactly as today. Add a test asserting the fallback selection logic picks `MouseMove` for an unsupported peer.

- [ ] **Step 4: Verify**

Run: `cargo test -p mineshare-input && cargo build`
Manual: with two batch-capable peers, confirm a game on the receiver reading Raw Input sees high-rate motion (smoother aim); with one old peer, confirm it still works via summed `MouseMove`.

- [ ] **Step 5: Commit**

```bash
git add crates/mineshare-input/src
git commit -m "feat(input): MouseMoveBatch high-rate path with version-negotiated fallback"
```

---

## Self-Review

**Spec coverage:**
- In-app 60–1000 Hz slider, default 500, ticks: Task 5 + Task 1. ✓
- Live readout of achieved forward/inject rate: Tasks 4–5. ✓
- Runtime-settable rate honored on both OSes (Linux 8 ms floor lifted): Tasks 2–3. ✓
- Persistence across restart: Task 1 (Settings JSON) + existing `set_settings`. ✓
- OS-timer caveat for >500 Hz: Task 3 Step 2. ✓
- Phase 2 batch architecture + version fallback + anti-cheat caveat: Tasks 6–7 + slider help text (Task 5 Step 1). ✓
- Feasibility spike before committing Phase 2: Task 6 (explicit gate). ✓

**Placeholder scan:** No TODO/TBD. The only cross-task dependency (Task 1 Step 4 ↔ Task 2 setter) is called out with an explicit ordering instruction.

**Type consistency:** `hz_to_flush_us`, `target_flush_us`, `set_mouse_rate_hz`, `mouse_rate_stats`/`MouseRateStats`, `bump_fwd_events`/`bump_inj_events` are defined once in `lib.rs` and referenced consistently. `mouse_rate_hz` field name matches across `settings.rs`, the input setter call, and the UI slider.

**Note:** opus/signal is irrelevant here (audio plan). The `windows`-crate `timeBeginPeriod` import path should be verified against the project's `windows` crate features during Task 3 Step 2.
