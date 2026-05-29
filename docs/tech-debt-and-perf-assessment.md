# MineShare — Technical Debt & Performance Assessment

_Date: 2026-05-29 · Version: 0.0.7_

**Scope assessed:** input capture/routing (`mineshare-input`), the daemon
transport hotpath (`mineshare-daemon/runtime.rs`), the React/Tauri UI, and
overall workspace structure. **Not** deeply reviewed: audio jitter/playback
(`mineshare-audio/playback.rs`) and the reconnection logic — these are
real-time-sensitive and deserve a separate pass.

---

## TL;DR

The hotpath is already well-engineered for its goal (low-latency input
forwarding): UDP datagrams, `set_nodelay(true)` on the control TCP, a
`biased` select that drains input ahead of audio, the OS hook calling the
sink directly with no extra MPSC hop, and a release profile with thin-LTO +
`codegen-units=1`. So this is **not** a "slow app that needs rescuing" — the
findings are mostly maintainability debt plus one real robustness/efficiency
win in the transport.

Tiered by impact × effort:

| Tier | Item | Type |
|------|------|------|
| **High** | No tests on `daemon` / `input` / `audio` (~9k of ~10k LOC) | Debt |
| **Medium** | Coalesce backlogged `MouseMove` deltas instead of dropping them | Perf + correctness |
| **Medium** | `runtime.rs` is a 1614-line file; `setup_peer_session` is a god-function | Debt |
| **Medium** | ~20 global mutable statics in `input/lib.rs` (the *reason* it has no tests) | Debt / testability |
| **Low** | `get_transfers` UI poll lacks the visibility-pause guard the other polls have | Perf hygiene |
| **Low** | Build artifact + sample logs tracked in git | Hygiene |
| **Low** | Micro-optimizations in the hotpath — **measure before touching** | Perf (likely noise) |

---

## High — test coverage

Tests exist only in `mineshare-core` (`layout`, `source_fsm`) and
`mineshare-net` (`discovery`, `pairing`). The three crates that hold almost
all the complex, bug-prone logic have **zero** automated tests:

- `mineshare-daemon` — 3785 LOC (session lifecycle, framing, stats)
- `mineshare-input` — 3330 LOC (Smart keyboard routing, held-key tracking, edge/hysteresis)
- `mineshare-audio` — 1956 LOC

The Smart-routing decision tree (`should_forward_keys`, `route_keystroke`,
`route_mouse_button` in `input/lib.rs`) is pure-ish logic with many edge
cases — modifiers stranded on the peer, sticky decisions, click-recency
windows — and is exactly the kind of code that regresses silently. It's the
highest-value place to add tests. Today it can't be tested because its inputs
are read from process-global statics (see the Medium item below).

## Medium — coalesce `MouseMove` instead of dropping (the one real perf/robustness win)

Input flows through `broadcast::channel::<InputEvent>(64)`
(`runtime.rs:234`). When `udp.send_to` backpressures on a slow/Wi‑Fi link the
buffer fills, and tokio broadcast then **drops the oldest events** and hands
the subscriber `Lagged(n)` (`runtime.rs:1305`). The code logs the skip but the
events are gone.

Because `MouseMove { dx, dy }` is a **relative** delta with no absolute
reference, every dropped motion event is permanently lost cursor distance →
**cursor drift / "my aim fell short"** under exactly the congested conditions
the recent game-mode commits care about.

**Fix:** before sending, coalesce any pending `MouseMove` events into a single
summed-delta packet. This is strictly *more correct* than dropping (no lost
distance) and *more efficient* (each tiny MouseMove otherwise pays a full
UDP/IP header + AEAD tag + bincode framing). Caveat: it slightly reduces motion
granularity, but only while already backlogged — a good trade. Button/key
events must still be forwarded individually and in order.

## Medium — `runtime.rs` structure

`run()` / the per-peer session setup is one ~1100-line function that spawns
the writer, ping, beacon, reader, recv, and stats tasks inline, with the
encode→encrypt→send logic in a local `macro_rules!`. It works, but it's hard
to read, impossible to unit-test, and every change risks the whole session.
Suggest extracting: `spawn_writer`, `spawn_reader`, `spawn_beacon`,
`spawn_recv`, and a small `WireCodec`/`send_wire` helper (function, not macro).

## Medium — global mutable state in `input/lib.rs`

~20 `static` atomics + `Mutex`es hold all routing/mode state (`INPUT_LOCKED`,
`KEYBOARD_TARGET`, `HELD_FORWARDED`, `LOCAL_MOUSE_AT`, `PEER_SIDE`, …). For a
one-machine-one-input-system app a process singleton is a *defensible* design
choice, not a bug — but it's the direct cause of the test gap: the routing
logic can't be exercised in isolation. If/when you add tests, the minimal
refactor is to thread a `&RoutingState` struct through `should_forward_keys` /
`route_keystroke` and keep a single global instance for production callers.

## Low — UI poll consistency (`ui/src/App.tsx`)

`get_status` and `get_latency` correctly pause their `setInterval` when the
window is hidden to tray (`visibilitychange`). The `get_transfers` poll
(`App.tsx:224`, every 700 ms) does **not** — it keeps firing IPC + React
renders forever even when minimized and even when zero transfers are active.
Add the same visibility guard, or move transfer-completion to a Tauri
`emit`/`listen` event push instead of polling. (`App.tsx` is also an 892-line
component that would benefit from splitting, but that's cosmetic.)

## Low — git hygiene

- `ui/tsconfig.tsbuildinfo` is tracked — it's a TypeScript incremental build
  cache and should be in `.gitignore` + `git rm --cached`.
- `logs/LAPTOP-4E1649F3-windows.log` and `logs/Ubuntu-Tee-linux.log` are
  force-included via the `!logs/*.log` rule. If those are real sample bundles
  kept on purpose, fine; if they're stray captures, drop them.
- (`mineshare-test.err/.log` in the repo root are already gitignored — not an issue.)

## Low — hotpath micro-optimizations (measure first; likely noise)

These are CPU/code-hygiene only. At input rates (≤1 kHz) they are almost
certainly invisible next to network RTT — **profile before spending effort**:

- Per-event `bincode::encode_to_vec` + `aead.seal` each heap-allocate a fresh
  `Vec`. A reusable scratch buffer would cut ~1–2k tiny allocs/sec.
- `bump_local_mouse_activity()` calls `SystemTime::now()` on every motion;
  Smart routing calls `now_ms()` several times per keystroke. A coarse
  monotonic clock would be cheaper.
- `HELD_FORWARDED: Mutex<[bool; 1024]>` is locked per keystroke — could be an
  atomic bitmap. Keystroke rate is low, so this is purely theoretical.

## Suggested next pass (out of scope here)

Review `mineshare-audio/playback.rs` (jitter buffer / underrun behavior) and
the reconnection path documented in
`docs/dev-brief-reconnection-and-jitter.md` — both are latency-sensitive and
weren't covered above.
