# Opus Audio — Fidelity, Robustness, Latency

_Date: 2026-06-14 · Crate: `mineshare-audio`_

## Problem

The Opus pipeline works but leaves clear quality on the table, and the receive
path has no loss handling:

- **Encoder** (`codec.rs`): `Application::Audio`, bitrate set per source —
  sysout **96 kbps** (`wasapi_loopback.rs`, `pipewire_monitor.rs`), mic
  **48 kbps** (`cpal_mic.rs`). No `signal` type, no `set_packet_loss_perc`, no
  explicit complexity/VBR (relies on libopus defaults).
- **Receive path** (`playback.rs`): frames are Opus-decoded **in arrival order**
  and pushed into a passive ~200 ms ring (`RING_CAPACITY = FRAME_SAMPLES_INTERLEAVED
  * 10`). On underrun the callback fills **silence**.
- **`AudioFrame.seq` exists but is never read** in `playback.rs` — so there is
  **no reorder detection, no packet-loss concealment (PLC), and no FEC** today.
  A dropped/late packet becomes an audible silent gap.

## Goal

Improve perceived quality in priority order: **fidelity first** (cheap, high
payoff), then **robustness** (graceful loss handling), then **latency**
(measured, not assumed). User chose "both / robustness" — this spec delivers
all three, phased by payoff-to-effort.

## Design

### Phase 1 — Fidelity (cheap, high payoff)

1. **Verify before changing.** First confirm current *effective* libopus
   settings (VBR, complexity, bandwidth) via the encoder getters. libopus very
   likely already defaults to VBR on, complexity ~9, and fullband at these
   bitrates — **do not re-set what is already optimal**; only change what
   measurably helps.
2. **Raise sysout bitrate** from 96 kbps toward **128–160 kbps** for clearly
   better music/system-audio fidelity (stereo, 48 kHz). Keep it a single named
   constant per source so it's easy to tune.
3. **Set Opus signal hint:** `signal = MUSIC` for sysout, `signal = VOICE` for
   mic — lets the encoder bias its model correctly per stream.
4. **(Optional) expose complexity** if measurement shows the default isn't max;
   otherwise leave default.

### Phase 2 — Robustness (graceful loss handling)

Make the receive path **seq-aware** using the existing `AudioFrame.seq`:

1. **Reorder / gap detection.** Track the last decoded `seq`. If the next frame
   is the expected `seq+1`, decode normally. If `seq` jumps (loss), invoke
   **Opus PLC**: call the decoder's packet-loss-concealment path (decode with a
   null/empty packet, `fec=false`) to synthesize a concealment frame *instead of
   silence*, then decode the real frame. Drop frames whose `seq` is older than
   the last played (late/duplicate).
2. **In-band FEC (optional, voice-leaning).** Encoder: `set_packet_loss_perc`
   (e.g. 10–20%) so Opus embeds LBRR. Decoder: on a detected single-packet loss,
   decode the *next* successfully-received packet with **`fec = true`** to
   recover the missing frame (`decode_float(next, …, /*fec=*/ true)`), then
   decode it again normally. Flag explicitly: FEC mainly helps **voice/mic**;
   for stereo music it yields little useful LBRR, so gate it to the mic stream
   (or make it conditional) rather than blanket-enabling.
3. **DTX dropped** — it is a silence-bandwidth optimization, not a quality
   feature, and rarely triggers on loopback system audio. Out of scope.

The `seq`-aware logic lives in `playback.rs` (and/or a small jitter-buffer
module it owns); the encoder changes live in `codec.rs` + the capture modules.

### Phase 3 — Latency (measured, not assumed)

1. **Target-depth jitter buffer.** The ring is currently passive — it fills to
   whatever arrives and underruns to silence. Introduce a small **target depth**
   (e.g. start playback only once N frames are buffered, hold ~40–60 ms) so
   transient jitter doesn't underrun, and trim excess so depth doesn't grow
   unbounded (latency creep). Jitter-buffer depth dominates end-to-end latency
   far more than frame size.
2. **Frame size as an experiment, not a default.** Treat **20 ms → 10 ms** as a
   *measured* trial: it halves per-frame latency but **doubles packet rate and
   lowers Opus coding efficiency**. Only adopt if measurement shows a worthwhile
   latency win at acceptable bitrate/overhead. `FRAME_SAMPLES_*` are canonical
   constants shared by encoder + decoder, so any change is a coordinated,
   both-sides change.

## Risks / notes

- **Format is canonical and symmetric** (`SAMPLE_RATE`, `CHANNELS`,
  `FRAME_SAMPLES_*` in `lib.rs`); both sides agree implicitly. Bitrate/signal/
  FEC-perc are **encoder-local** and safe to change one-sidedly. Frame-size and
  any decode-side FEC expectations are **protocol-coupled** — change together.
- **Don't over-build FEC/DTX.** They are the heaviest plumbing for the least
  payoff on a 96k+ stereo system-audio stream; bitrate + signal type is the
  dominant fidelity lever. Keep Phase 2's FEC optional and voice-scoped.
- **PLC vs silence** is the single biggest *robustness* win and is cheap once
  the path is seq-aware — prioritize it over FEC.
- Measure with a known loss injection (drop X% of frames) to validate PLC/FEC
  actually conceal rather than regress.

## Non-goals

- No codec change (stay on Opus).
- No change to capture/playback device handling, resampling, or the cpal
  rebuild/watchdog logic.

## Verification

- Phase 1: A/B listen sysout at 96 vs 128–160 kbps; confirm signal hints set;
  confirm no regression in bandwidth beyond the intended increase.
- Phase 2: inject 5–15% synthetic frame loss; confirm PLC produces concealment
  (no hard silence clicks); with FEC on the mic stream, confirm single-loss
  recovery; confirm reordered/late frames are dropped cleanly.
- Phase 3: measure end-to-end audio latency before/after the target-depth
  buffer; if 10 ms frames are trialed, record latency + bitrate deltas in the PR
  and only keep if net-positive.
