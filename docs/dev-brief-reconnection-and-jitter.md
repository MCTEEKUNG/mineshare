# Development Brief — Cross-Device Auto-Reconnection & Input Jitter

**Status:** Draft for triage
**Scope:** `mineshare-daemon` (discovery / session lifecycle), `mineshare-net` (mDNS), `mineshare-input` (capture/inject), `mineshare-audio` (playback)
**Related work already shipped this cycle:** `8127aa5`, `c64260d`, `17b1b9e`, `21a5e29` (v0.0.2 → v0.0.4)

---

## 1. Reported Issues

1. **Noticeable lag / jitter** when driving the peer cursor.
2. **No automatic reconnection after one side closes.** When one device's app is closed (Quit or killed), the other device does *not* re-establish the link on its own. In practice users must **launch the app on both devices within a short window** to get connected again.
3. Question to resolve: **is the "launch both together" behaviour intended, or a bug?**

This brief answers (3) with evidence, proposes fixes for (2), summarizes the jitter work already done plus what remains for (1), and gives a concrete test plan.

---

## 2. Investigation — How Discovery & Reconnection Actually Work

Code paths traced (file:line as of this brief):

### Discovery layer — `crates/mineshare-net/src/discovery.rs`
- `browse()` (l.84) consumes `mdns_sd` events and emits `DiscoveryEvent::PeerOnline/PeerOffline`.
- It keeps **`announced: HashSet<DeviceId>`** (l.95). On `ServiceResolved` it only emits `PeerOnline` **if the id was not already in `announced`** (l.121–132). A re-advertisement from an already-known device — *even with a different address/port* — is **swallowed as a duplicate**.
- `PeerOffline` is emitted only on `ServiceRemoved` (l.137–151), which clears the `announced` entry.
- `shutdown()` (l.159) calls `daemon.shutdown()` (sends the mDNS goodbye) — **but it is never called anywhere.**

### Session layer — `crates/mineshare-daemon/src/runtime.rs`
- Control listener binds an **ephemeral TCP port** (`TcpListener::bind(...)` l.426 → `local_port = listener.local_addr()?.port()` l.432) and advertises it (`control_port: local_port` l.471). **The port changes on every launch** (observed in logs: `…:65207`, `…:64520`, `…:63282`, `…:55064`).
- Discovery event loop (l.484–577):
  - On `PeerOnline`: if the peer is **`already_known`**, it does **`continue`** (l.497–499) — no address refresh, no re-dial.
  - Otherwise it inserts the peer and **only the lower device-id dials**: `if local_id.0 < peer.device_id.0` (l.509). The higher id **defers** ("peer will initiate", l.568–570).
  - The dialer spawns a **reconnect loop** (l.537–567) that redials every 2 s **as long as the peer remains in `known_peers`**.
  - On `PeerOffline`: the peer is **removed from `known_peers`** (l.573), which makes the dialer's reconnect loop exit (l.543–547).
- Tray **"Quit" → `app.exit(0)`** (`ui/src-tauri/src/tray.rs:131`) — a hard exit. `discovery.shutdown()` is **not** invoked, so **no mDNS goodbye is sent**. Closing the window only hides to tray (daemon keeps announcing).

---

## 3. Root-Cause Analysis — **It's a bug, not a feature**

The "redial while the peer is in `known_peers`" design is sound **only for the case where the *dialer* (lower id) restarts**. It breaks for the other side because of three compounding facts:

| # | Fact | Consequence |
|---|------|-------------|
| A | **Stale advertised endpoint.** The control port is **ephemeral** — `DEFAULT_CONTROL_PORT = 0` (OS-assigned), confirmed at `runtime.rs:32`, so it changes every launch; **and** the host IP changes under DHCP (observed in this fleet: a peer moved `192.168.1.106 → .110`). The UDP data port (`runtime.rs:799`/`:812`) is ephemeral too. | A cached advert points the peer at a **dead address:port** after the other side restarts or its lease rotates. |
| B | **Double dedup** (`announced` set in discovery *and* `already_known` in runtime) keyed only on `DeviceId` | A re-announce carrying the **new** endpoint is treated as a duplicate and **dropped** — the new connection info never reaches the dial logic. |
| C | **No goodbye on close** (`shutdown()` never called; Quit = `app.exit(0)`) | `ServiceRemoved` → `PeerOffline` only fires on **mDNS TTL expiry** (tens of seconds to minutes), so the dedup state is rarely reset promptly. |

> **Note on scope of the bug.** If both the IP *and* the ephemeral port happened to be unchanged across a restart, L's persistent reconnect loop would redial the same `address:port` and reconnect within ~2 s — no bug. The defect bites precisely when the **endpoint changes** (new ephemeral port on every restart — always — and/or a new DHCP IP) **and** the dedup suppresses the re-announce that carries the new endpoint. Because the control port is ephemeral, **every** restart changes the endpoint, so the trigger is the common case, not an edge case.

### The failing scenario (defer-side restarts)
Let **L** = lower device-id (the dialer), **H** = higher device-id (defers).

1. **H closes.** No goodbye is sent. L does **not** receive `ServiceRemoved` promptly → L keeps H's **old** advert in `known_peers` and H's id in `announced`; L's reconnect loop keeps dialing **H's old, now-dead endpoint** (connection refused every 2 s).
2. **H reopens** with a **new ephemeral port** (and possibly new IP), browses fresh, resolves L → `PeerOnline`. But **H is the higher id → it defers** and waits for L to initiate.
3. **L sees H's re-announce** (`ServiceResolved`) but H's id is still in L's `announced` set → **swallowed as duplicate → no `PeerOnline`** → the runtime handler never runs → L never learns H's **new endpoint** and never re-initiates.
4. **Deadlock** until the mDNS cache TTL finally expires on L, fires `ServiceRemoved` → `PeerOffline`, clears the dedup, and the next re-resolve is treated as new.

> By contrast, when the **dialer (L) restarts**, it comes up with empty `announced`/`known_peers`, re-resolves H, and — being the lower id — **re-initiates from a clean slate**, so that direction reconnects today. The asymmetry is why only one side's restart is "stuck."

### Why "launch both within a short window" appears to work
Restarting **both** processes wipes `announced` and `known_peers` on **both** sides simultaneously; both rediscover from a clean slate, the lower id dials the fresh port, and the link comes up. That's the **workaround masking the bug**, not an intended handshake requirement.

**Conclusion:** The required-simultaneous-launch behaviour is an emergent bug from (A)+(B)+(C). Single-sided restart *should* reconnect automatically and does not.

---

## 4. Proposed Solutions (reconnection)

Ordered by impact / effort. Recommend doing **P0 + P1 + P2** together; they reinforce each other.

> **P0 is the functional fix.** It is the only change that lets a single-sided restart reconnect on its own. P2–P4 make it faster/cleaner but are not substitutes.

> **⚠️ Prerequisite to verify before implementing P0** (load-bearing): does `mdns-sd` **0.13.11** itself re-emit `ServiceEvent::ServiceResolved` when an instance re-announces with a **changed SRV port / A record**, or does it dedup at its own cache layer and never deliver the event? If the library delivers it, P0 (below) is correct. **If the library suppresses it, P0 is impossible as written** and the fix shifts to **P3(a) — force an mDNS re-query on connection-refused** instead. *Cheap check:* a ~15-line harness that announces an instance, then re-announces the same instance name on a different port, and asserts whether `ServiceResolved` fires a second time. Do this first.

### P0 — Treat a re-announce with changed endpoint as actionable (fixes B) — *contingent on the check above*
- **Discovery (`browse`, discovery.rs:84)**: store the last resolved `PeerAdvert` per id, not just a presence bit in `announced`. On `ServiceResolved`, if `addresses`/`control_port` **differ** from the stored advert, forward `PeerOnline` again (an "updated endpoint" event). Keep silent-dedup **only** for byte-identical adverts (the genuine multi-NIC duplicate the current code targets).
- **Runtime `PeerOnline` handler (runtime.rs:491)**: the blind `continue` on `already_known` (l.497–499) is what drops the update. Note the entry is *already* refreshed at l.494 (`k.insert(...)` runs before the check), so the stored advert isn't the gap — the gap is that **no dial is re-armed** and, upstream, the event may never arrive (discovery dedup). Change the handler so an already-known peer with a **changed endpoint** falls through to the re-arm logic in P1 instead of `continue`.

### P1 — Re-arm the dial when a known peer reappears (defense-in-depth)
- **Most of the machinery already exists**: `known_peers[id]` is updated on every event (l.494), and the reconnect loop re-reads `known_peers[peer_id]` each iteration (l.539–549), so a *running* loop will pick up a refreshed endpoint automatically.
- **The actual missing piece**: when the loop has **exited** (the defer side never had one; or the dialer's loop returned after a `PeerOffline`), nothing restarts it. Add: on an already-known peer that is **not in `in_flight`** (l.514) and has **no active session**, **(re)spawn the reconnect loop**. Track a per-peer "connected" flag so the handler distinguishes a stale duplicate (ignore) from "peer came back" (re-arm).

### P2 — Send mDNS goodbye on shutdown (fixes C)
- Call `discovery.shutdown()` on graceful exit so the peer gets a prompt `ServiceRemoved` and clears dedup immediately:
  - Tray **Quit**: before `app.exit(0)` (`tray.rs:131`), trigger daemon teardown that calls `Discovery::shutdown()`.
  - Also handle `Ctrl-C`/SIGTERM and window-destroy for the headless/daemon binary.
- This makes the **dialer-restart** path snappy too and prevents the stale-port redials.

### P3 — Reduce the blast radius of a dynamic port (mitigation / defense-in-depth)
- Option (a): **re-resolve before each dial** — on connection-refused, force an mDNS re-query rather than trusting the cached advert.
- Option (b): **stable/preferred control port** (try a fixed port, fall back to ephemeral) so a stale advert often still resolves to a live listener. Lower-effort safety net; not a substitute for P0–P2.

### P4 — Symmetric re-initiation guard
- Keep `local_id < peer.device_id` for the **initial** dial (prevents simultaneous double-dial races) but make it explicit that **either** side may re-initiate after a drop, arbitrated by the `in_flight` set (l.514–521) so we still never open two concurrent sessions.

**Acceptance for the reconnection fix:** restarting *either* side alone reconnects automatically in **< 5 s**, with no requirement to touch the other device.

---

## 5. Performance / Jitter

### Already shipped this cycle (baseline for re-measurement)
- **`8127aa5`** — WM_INPUT raw hardware mickeys on Windows (1:1 native feel, removes the post-acceleration screen-space delta + 30 px cap).
- **`c64260d`** — WM_INPUT delivery watchdog + **8 ms motion coalescing** on Windows (matches the Linux evdev path; caps event rate at ~125 Hz so a 1000 Hz mouse can't flood the peer's inject loop).
- **`17b1b9e`** — **input-priority send path**: input on its own broadcast channel, drained ahead of audio via `tokio::select! { biased; … }` (removes the ~20–40 ms an input event used to wait behind a queued Opus frame).
- Measured network RTT on LAN is **~1 ms** (Ping/Pong telemetry), so remaining "lag" is pipeline/scheduling, not the wire.

### Remaining jitter work to investigate
1. **Audio playback ring runs chronically dry** — logs show tens of thousands of `cpal playback ring underrun (filled with silence)` per hour. Even though it's separate from cursor jitter, it points to a send-cadence vs. playback-consumption mismatch worth a dedicated look (adaptive jitter buffer / dynamic target fill instead of the fixed 200 ms `RING_CAPACITY`).
2. **End-to-end input latency instrumentation** — RTT ≠ input-to-inject. Add a timestamp at capture and log delta at inject so we measure what the user actually feels (target p95 < ~5 ms LAN).
3. **Scheduler jitter** — consider a dedicated thread / elevated priority for the input send path so audio/file work can't preempt it under load.
4. **UDP socket buffers** — verify/raise `SO_RCVBUF`/`SO_SNDBUF` on the data socket (`runtime.rs:799`) to absorb bursts.

---

## 6. Test Plan — Reliable Cross-Device Connectivity

### A0. Reproduce the bug FIRST (before/after gate)
The fix is only proven if the failure is reproduced on `main` first, then shown fixed:
1. On **current `main`**, run **R2** (restart the higher-id/defer side only). **Expect: no reconnect** (stuck until both relaunch / TTL expiry). This is the regression test — keep it.
2. Apply **P0 (+P2)**. Re-run **R2**. **Expect: auto-reconnect < 5 s, other device untouched.**
3. Keep R2 as a permanent automated/manual regression check.

### A. Reconnection matrix (the core fix)
Run each row; record **time-to-reconnect** and whether the *other* device had to be touched. Watch `1-Hz stats peer=<ip>:<port>` to confirm the **new** endpoint is adopted and `injected`/`audio_recv` resume.

| Case | Action | Expected |
|------|--------|----------|
| R1 | Restart **lower-id** (dialer) only | auto-reconnect < 5 s |
| R2 | Restart **higher-id** (defer) only | auto-reconnect < 5 s ← *currently broken* |
| R3 | Restart **both** within ~3 s | auto-reconnect (baseline; works today) |
| R4 | **Graceful Quit** (tray) vs **hard kill** (Task Manager) for R1/R2 | both reconnect; goodbye makes graceful path faster |
| R5 | **Hide-to-tray then re-open** window only | session never dropped |
| R6 | **Wi-Fi off/on** / Ethernet unplug-replug on one side | reconnect after link returns, no manual relaunch |
| R7 | **Port-change check** | peer adopts the new ephemeral port after restart (assert via stats line) |

### B. Soak / endurance
- Keep the pair connected **12–24 h**; a scripted timer restarts **one** side (alternating) every N minutes. Log time-to-reconnect each cycle. **Pass = never requires both sides to restart and p95 reconnect < 5 s.**

### C. Jitter / latency
- Controlled mouse sweep (fixed speed via a macro) across the screen edge; capture RTT histogram (GUI) + new input-to-inject timestamps. Report p50/p95/p99 vs. a native local baseline.
- Audio: play a continuous tone; count underruns/min before vs. after any buffer change; confirm the v0.0.3 device-loss watchdog still recovers audio < 0.5 s when a headset is unplugged.

### D. Regression guards
- Confirm the **single-instance guard** (v0.0.4) still blocks a second daemon throughout the restart churn (no dual-daemon CPU spike / reconnect loop).
- Confirm no duplicate sessions to one peer after rapid restart cycles (assert the `in_flight` guard holds).

---

## 7. Suggested Sequencing

0. **Verify the mdns-sd re-resolution assumption** (the prerequisite check in §4) and **reproduce R2 on `main`** (§6.A0). These gate everything else and decide P0's shape.
1. **P2** (goodbye on shutdown) — small, low-risk; shortens stale-state lifetime and speeds the graceful path. *Not a functional fix on its own* (hard-kill / power-loss still rely on P0).
2. **P0 + P1** — **the actual functional fix** for single-sided restart (endpoint-aware re-announce + re-arm the dial when a known peer reappears). Land with R1–R3, R7. If the §4 check shows mdns-sd suppresses the event, swap P0 for **P3(a) force re-query** here.
3. **P3/P4** hardening + the jitter instrumentation items (§5).
4. Full matrix (R1–R7) + soak (B) before tagging the release; R2 stays as a permanent regression test.
