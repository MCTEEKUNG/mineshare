# Opus Audio Quality — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve perceived audio quality in priority order — fidelity (bitrate + explicit VBR), robustness (seq-aware playback with packet-loss concealment, optional voice FEC), then measured latency tuning — without changing the codec or the canonical wire format unnecessarily.

**Architecture:** Phase 1 raises the sysout bitrate and sets VBR explicitly in `OpusEncoder`, after verifying current libopus defaults. Phase 2 makes `playback.rs` use the existing-but-unused `AudioFrame.seq` to detect gaps and synthesize concealment frames via Opus PLC instead of silence, with optional in-band FEC scoped to the mic stream. Phase 3 adds a small target-depth prebuffer and trials 10 ms frames as a measured experiment. Spec: `docs/superpowers/specs/2026-06-14-opus-audio-quality-design.md`.

**Tech Stack:** Rust, `opus` crate v0.3.1 (`set_bitrate`, `set_vbr`, `set_inband_fec`, `set_packet_loss_perc`, `decode_float(.., fec)`), `cpal` playback, `ringbuf`. Tests: `cargo test -p mineshare-audio`. Note: opus 0.3.1 does **not** expose a signal-type setter, so `signal=MUSIC/VOICE` from the spec is out of scope unless the crate is upgraded — bitrate is the dominant fidelity lever regardless.

---

## File Structure

- `crates/mineshare-audio/src/codec.rs` — **modify**: `OpusEncoder::new` gains explicit VBR + optional in-band FEC; `OpusDecoder` gains `decode_plc()` (loss concealment) and `decode_fec()` (recover from next packet).
- `crates/mineshare-audio/src/wasapi_loopback.rs`, `pipewire_monitor.rs` — **modify**: raise `OPUS_BITRATE_BPS` 96k → 128k; pass `fec=false`.
- `crates/mineshare-audio/src/cpal_mic.rs` — **modify**: pass `fec=true` (voice stream) and a packet-loss-perc.
- `crates/mineshare-audio/src/playback.rs` — **modify**: track last `seq`, conceal gaps with PLC, drop late/duplicate frames; add a target-depth prebuffer.
- `crates/mineshare-audio/src/lib.rs` — **modify** (Phase 3 only, gated): frame-size constants if the 10 ms experiment is adopted.

---

## Phase 1 — Fidelity

### Task 1: Verify defaults, raise bitrate, set VBR (TDD)

**Files:**
- Modify: `crates/mineshare-audio/src/codec.rs`, `wasapi_loopback.rs`, `pipewire_monitor.rs`, `cpal_mic.rs`

- [ ] **Step 1: Write a failing test** that pins the new encoder constructor shape and that encoding still yields a non-empty payload. Add to `codec.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::FRAME_SAMPLES_INTERLEAVED;

    #[test]
    fn encoder_builds_with_fec_flag_and_encodes() {
        let mut enc = OpusEncoder::new(128_000, true).expect("encoder");
        let pcm = vec![0.05f32; FRAME_SAMPLES_INTERLEAVED];
        let payload = enc.encode(&pcm).expect("encode");
        assert!(!payload.is_empty());
    }
}
```

- [ ] **Step 2: Run it, expect FAIL**

Run: `cargo test -p mineshare-audio encoder_builds_with_fec_flag_and_encodes`
Expected: FAIL — `OpusEncoder::new` takes one argument.

- [ ] **Step 3: Extend the constructor.** Replace `OpusEncoder::new` in `codec.rs`:

```rust
    /// `inband_fec`: enable Opus in-band FEC (LBRR). Worth it for the
    /// mic/voice stream; near-useless for stereo music sysout, so the
    /// callers pass `false` there.
    pub fn new(bitrate_bps: i32, inband_fec: bool) -> Result<Self> {
        let mut enc = opus::Encoder::new(SAMPLE_RATE, channels(), opus::Application::Audio)
            .context("opus encoder init")?;
        enc.set_bitrate(opus::Bitrate::Bits(bitrate_bps)).context("opus set bitrate")?;
        // Explicit VBR — libopus defaults to VBR on, but set it so the
        // intent is visible and stable across crate versions.
        enc.set_vbr(true).context("opus set vbr")?;
        if inband_fec {
            enc.set_inband_fec(true).context("opus set inband fec")?;
            // Tell the encoder roughly how lossy the link is so it sizes
            // LBRR redundancy. 10% is a reasonable LAN-with-jitter default.
            enc.set_packet_loss_perc(10).context("opus set packet loss perc")?;
        }
        Ok(Self { enc, out: vec![0u8; MAX_OPUS_PAYLOAD] })
    }
```

- [ ] **Step 4: Update the three call sites.** In `wasapi_loopback.rs` and `pipewire_monitor.rs`: change `const OPUS_BITRATE_BPS: i32 = 96_000;` → `128_000;` and the call `OpusEncoder::new(OPUS_BITRATE_BPS)` → `OpusEncoder::new(OPUS_BITRATE_BPS, false)`. In `cpal_mic.rs`: keep `OPUS_BITRATE_BPS = 48_000` and change the call to `OpusEncoder::new(OPUS_BITRATE_BPS, true)`.

- [ ] **Step 5: Run test + build, expect PASS / clean**

Run: `cargo test -p mineshare-audio encoder_builds_with_fec_flag_and_encodes && cargo build -p mineshare-audio`
Expected: PASS, clean build.

- [ ] **Step 6: (Verification of defaults) Add a one-shot debug log** in `OpusEncoder::new` (behind `tracing::debug!`) printing `enc.get_bitrate()`, `enc.get_vbr()` (if available) so a dev run confirms effective settings. Keep it at `debug` level.

- [ ] **Step 7: Commit**

```bash
git add crates/mineshare-audio/src/codec.rs crates/mineshare-audio/src/wasapi_loopback.rs crates/mineshare-audio/src/pipewire_monitor.rs crates/mineshare-audio/src/cpal_mic.rs
git commit -m "feat(audio): sysout 128 kbps + explicit VBR, mic in-band FEC"
```

---

## Phase 2 — Robustness (PLC + FEC)

### Task 2: Seq-aware playback with packet-loss concealment

**Files:**
- Modify: `crates/mineshare-audio/src/codec.rs`, `crates/mineshare-audio/src/playback.rs`

- [ ] **Step 1: Add a PLC decode method + a pure gap helper (TDD).** In `codec.rs`:

```rust
impl OpusDecoder {
    /// Synthesize one concealment frame for a lost packet (Opus PLC).
    /// Pass an empty input slice; libopus extrapolates from history.
    pub fn decode_plc(&mut self, pcm: &mut [f32]) -> Result<usize> {
        anyhow::ensure!(pcm.len() >= FRAME_SAMPLES_INTERLEAVED, "plc buffer too small");
        let n = self.dec.decode_float(&[], pcm, false).context("opus plc")?;
        Ok(n * CHANNELS as usize)
    }
}

/// How many frames were lost between `last` and `current` seq, saturating
/// and treating wrap/reset as zero. `None` last means "first frame".
pub fn frames_lost(last: Option<u32>, current: u32) -> u32 {
    match last {
        Some(l) if current > l => current - l - 1,
        _ => 0,
    }
}
```

Add a test in `codec.rs`'s test module:

```rust
#[test]
fn frames_lost_counts_the_gap() {
    assert_eq!(frames_lost(None, 0), 0);
    assert_eq!(frames_lost(Some(4), 5), 0);   // contiguous
    assert_eq!(frames_lost(Some(4), 7), 2);   // dropped 5,6
    assert_eq!(frames_lost(Some(9), 9), 0);   // duplicate -> caller drops
}
```

- [ ] **Step 2: Run the test, expect FAIL then implement then PASS**

Run: `cargo test -p mineshare-audio frames_lost_counts_the_gap`
Expected: FAIL (not defined) → after Step 1 code, PASS.

- [ ] **Step 3: Use seq in `playback.rs`.** In `run_playback_thread`, add `let mut last_seq: Option<u32> = None;`. In the `Ok(frame)` arm, before decoding:
  - If `last_seq` is `Some(l)` and `frame.seq <= l`, it's a duplicate/late frame → `continue` (drop).
  - Compute `let lost = crate::codec::frames_lost(last_seq, frame.seq);` and for each lost frame (cap at e.g. 5 to avoid a long silence-fill burst), call `decoder.decode_plc(&mut scratch)` and push the concealment samples into the ring (same `push_slice` path as a real frame).
  - Then decode the real frame as today and push it.
  - Set `last_seq = Some(frame.seq);`.

Concretely, replace the decode block:

```rust
Ok(frame) => {
    if let Some(l) = last_seq {
        if frame.seq <= l { continue; } // duplicate / reordered-late
    }
    if let Some(ctx) = stream_ctx.as_mut() {
        let lost = crate::codec::frames_lost(last_seq, frame.seq).min(5);
        for _ in 0..lost {
            if let Ok(n) = decoder.decode_plc(&mut scratch) {
                ctx.producer.push_slice(&scratch[..n]);
            }
        }
        let n = match decoder.decode(&frame.opus, &mut scratch) {
            Ok(n) => n,
            Err(e) => { warn!(error = %e, "opus decode failed — dropping frame"); continue; }
        };
        let pushed = ctx.producer.push_slice(&scratch[..n]);
        frames_since_watchdog += 1;
        if pushed != n { warn!(dropped = n - pushed, "cpal playback ring full — dropping samples"); }
        last_seq = Some(frame.seq);
        // ... existing device-loss watchdog block unchanged ...
    } else {
        // ... existing "stream not built yet" drop path unchanged ...
    }
}
```

(Keep the existing watchdog and the `else` drop branch verbatim — only the decode/seq logic changes.)

- [ ] **Step 4: Verify**

Run: `cargo test -p mineshare-audio && cargo build -p mineshare-audio`
Manual loss test: temporarily drop ~10% of frames at the enqueue site (or via a debug env flag) and listen — concealment should sound like brief smearing rather than hard silent clicks. Remove the debug drop before committing.

- [ ] **Step 5: Commit**

```bash
git add crates/mineshare-audio/src/codec.rs crates/mineshare-audio/src/playback.rs
git commit -m "feat(audio): seq-aware playback with Opus PLC instead of silence on loss"
```

### Task 3: FEC recovery on the mic/voice stream

**Files:**
- Modify: `crates/mineshare-audio/src/codec.rs`, `crates/mineshare-audio/src/playback.rs`

- [ ] **Step 1: Add a FEC decode method** in `codec.rs`:

```rust
impl OpusDecoder {
    /// Recover a lost frame from the FEC data embedded in the NEXT
    /// received packet. Call this with the next packet's bytes when a
    /// single packet was lost; then decode that same next packet
    /// normally afterwards.
    pub fn decode_fec(&mut self, next_opus: &[u8], pcm: &mut [f32]) -> Result<usize> {
        anyhow::ensure!(pcm.len() >= FRAME_SAMPLES_INTERLEAVED, "fec buffer too small");
        let n = self.dec.decode_float(next_opus, pcm, true).context("opus fec decode")?;
        Ok(n * CHANNELS as usize)
    }
}
```

- [ ] **Step 2: Use FEC for a single-frame gap on the mic stream only.** In `playback.rs`, when `frame.stream == StreamKind::Mic` and `lost == 1`, prefer `decoder.decode_fec(&frame.opus, &mut scratch)` to reconstruct the missing frame *before* decoding `frame` normally; fall back to `decode_plc` if FEC returns an error. For `SysOut`, keep PLC (music yields little useful LBRR). Gate with a simple `if frame.stream == StreamKind::Mic && lost == 1 { fec } else { plc-loop }`.

- [ ] **Step 3: Verify**

Run: `cargo build -p mineshare-audio`
Manual: with the mic bridge active and ~5–10% loss injected, single-loss gaps on voice should be near-inaudible (recovered) vs. PLC smear.

- [ ] **Step 4: Commit**

```bash
git add crates/mineshare-audio/src/codec.rs crates/mineshare-audio/src/playback.rs
git commit -m "feat(audio): in-band FEC recovery for single-frame mic loss"
```

---

## Phase 3 — Latency

### Task 4: Target-depth prebuffer

**Files:**
- Modify: `crates/mineshare-audio/src/playback.rs`

- [ ] **Step 1: Add a small prebuffer gate.** The ring currently plays whatever arrives and underruns to silence. Add a `TARGET_PREBUFFER_FRAMES` (e.g. 3 = ~60 ms at 20 ms) and a `priming: bool` state: while priming, accumulate decoded frames into the ring but the cpal callback already tolerates partial fills; once `>= TARGET_PREBUFFER_FRAMES * FRAME_SAMPLES_INTERLEAVED` samples are buffered, mark primed. After an underrun (ring emptied), re-enter priming so playback re-establishes the cushion instead of stuttering frame-by-frame. Keep it small — the goal is jitter absorption, not added latency.

```rust
const TARGET_PREBUFFER_SAMPLES: usize = FRAME_SAMPLES_INTERLEAVED * 3; // ~60 ms
```

Implement the gate around the producer push / the existing ring occupancy (the `ringbuf` producer exposes free/occupied length). Trim if occupancy exceeds, say, `RING_CAPACITY - FRAME_SAMPLES_INTERLEAVED` to bound latency creep (the existing "ring full" warn already drops excess, so this may need no extra code — verify occupancy doesn't grow unbounded).

- [ ] **Step 2: Verify**

Run: `cargo build -p mineshare-audio`
Manual: measure perceived audio start-latency and stability before/after under light packet jitter; confirm no permanent added lag (occupancy returns toward the target depth, doesn't climb).

- [ ] **Step 3: Commit**

```bash
git add crates/mineshare-audio/src/playback.rs
git commit -m "feat(audio): target-depth prebuffer to absorb jitter without latency creep"
```

### Task 5: 10 ms frame experiment (GATED — measure, don't assume)

**Files:**
- Modify: `crates/mineshare-audio/src/lib.rs` and both encoder/decoder paths (coordinated, both sides).

- [ ] **Step 1: Branch + measure.** On an experiment branch, change `FRAME_SAMPLES_PER_CHANNEL` from 960 (20 ms) to 480 (10 ms). Both encoder and decoder derive from `FRAME_SAMPLES_*`, so they stay consistent, but the capture chunking that feeds `encode()` must also emit 10 ms chunks — audit `wasapi_loopback.rs` / `pipewire_monitor.rs` / `cpal_mic.rs` ring sizing (`CAPTURE_RING_CAP = 96_000 / 50 * 2 * 5` assumes 50 fps / 20 ms; update to /100 for 10 ms).

- [ ] **Step 2: Record the tradeoff.** Measure end-to-end latency and the actual on-wire bitrate/packet-rate at 10 ms vs 20 ms. Packet rate doubles (50→100 fps); Opus efficiency drops slightly.

- [ ] **Step 3: Decision gate.** Adopt 10 ms only if the latency win is worthwhile at acceptable overhead; otherwise discard the branch and keep 20 ms. Either way, record the numbers in the PR.

```bash
git add crates/mineshare-audio/src
git commit -m "experiment(audio): 10 ms frame trial with measured latency/bitrate deltas"
```

---

## Self-Review

**Spec coverage:**
- Verify defaults before changing: Task 1 Step 6 (debug log) + explicit VBR. ✓
- Raise sysout bitrate 96→128k: Task 1 Step 4. ✓
- Signal type MUSIC/VOICE: **documented out of scope** (opus 0.3.1 lacks the setter) — noted in header + spec deviation called out here. ✓ (deviation, justified)
- Seq-aware playback + PLC instead of silence: Task 2. ✓
- Optional voice-scoped in-band FEC: Tasks 1 (encoder) + 3 (decoder recovery). ✓
- DTX dropped: not implemented, per spec. ✓
- Target-depth jitter buffer: Task 4. ✓
- 10 ms frame as measured experiment, not default: Task 5 (gated). ✓

**Placeholder scan:** No TODO/TBD. The 10 ms task is explicitly an experiment with a discard path, not an unfinished stub.

**Type consistency:** `OpusEncoder::new(bitrate, inband_fec)` signature is consistent across codec definition and all three call sites. `decode`, `decode_plc`, `decode_fec`, and `frames_lost` are defined once in `codec.rs` and used consistently in `playback.rs`. `StreamKind::Mic`/`SysOut` matches the existing enum in `lib.rs`.

**Deviation from spec (justified):** spec listed `signal=MUSIC/VOICE`; opus 0.3.1 exposes no signal setter, so this is dropped rather than faked. Bitrate + VBR carry the fidelity goal; revisit if the `opus` crate is upgraded.
