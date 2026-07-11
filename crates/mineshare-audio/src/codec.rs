//! Opus encoder / decoder wrappers.
//!
//! 48 kHz stereo, 20 ms frames, ~96 kbps target. Both sides agree on
//! the format implicitly via the canonical `SAMPLE_RATE` / `CHANNELS`
//! constants in the parent module — no negotiation messages.

use anyhow::{Context, Result};

use crate::{CHANNELS, FRAME_SAMPLES_INTERLEAVED, SAMPLE_RATE};

/// Maximum Opus payload size we expect to see. 1500 is well above the
/// realistic worst case at our bitrate but keeps a safe ceiling for
/// the decoder's receive buffer.
pub const MAX_OPUS_PAYLOAD: usize = 1500;

pub struct OpusEncoder {
    enc: opus::Encoder,
    /// Scratch buffer for the encoded frame — sized to `MAX_OPUS_PAYLOAD`.
    out: Vec<u8>,
}

impl OpusEncoder {
    /// `inband_fec`: enable Opus in-band FEC (LBRR). Worth it for the
    /// mic/voice stream; near-useless for stereo music sysout, so the
    /// callers pass `false` there.
    pub fn new(bitrate_bps: i32, inband_fec: bool) -> Result<Self> {
        let mut enc =
            opus::Encoder::new(SAMPLE_RATE, channels(), opus::Application::Audio)
                .context("opus encoder init")?;
        enc.set_bitrate(opus::Bitrate::Bits(bitrate_bps))
            .context("opus set bitrate")?;
        // Explicit VBR — libopus defaults to VBR on, but set it so the
        // intent is visible and stable across crate versions.
        enc.set_vbr(true).context("opus set vbr")?;
        // DTX lets the transport suppress comfort-noise frames while a
        // source is silent instead of waking the Wi-Fi radio 50 times/s.
        enc.set_dtx(true).context("opus set dtx")?;
        if inband_fec {
            enc.set_inband_fec(true).context("opus set inband fec")?;
            // Tell the encoder roughly how lossy the link is so it sizes
            // LBRR redundancy. 10% is a reasonable LAN-with-jitter default.
            enc.set_packet_loss_perc(10)
                .context("opus set packet loss perc")?;
        }
        tracing::debug!(
            bitrate = enc.get_bitrate().ok().map(|b| match b {
                opus::Bitrate::Bits(n) => n,
                _ => -1,
            }),
            inband_fec,
            "opus encoder configured"
        );
        Ok(Self {
            enc,
            out: vec![0u8; MAX_OPUS_PAYLOAD],
        })
    }

    /// Encode one 20 ms interleaved-stereo frame (`FRAME_SAMPLES_INTERLEAVED`
    /// f32 samples in [-1, 1]) → Opus payload.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        anyhow::ensure!(
            pcm.len() == FRAME_SAMPLES_INTERLEAVED,
            "expected {} interleaved samples, got {}",
            FRAME_SAMPLES_INTERLEAVED,
            pcm.len()
        );
        let n = self
            .enc
            .encode_float(pcm, &mut self.out)
            .context("opus encode")?;
        Ok(self.out[..n].to_vec())
    }
}

pub struct OpusDecoder {
    dec: opus::Decoder,
}

impl OpusDecoder {
    pub fn new() -> Result<Self> {
        let dec = opus::Decoder::new(SAMPLE_RATE, channels()).context("opus decoder init")?;
        Ok(Self { dec })
    }

    /// Decode one Opus payload → exactly `FRAME_SAMPLES_INTERLEAVED`
    /// f32 samples in [-1, 1]. `pcm` must be pre-sized to fit the
    /// canonical frame.
    pub fn decode(&mut self, opus_bytes: &[u8], pcm: &mut [f32]) -> Result<usize> {
        anyhow::ensure!(
            pcm.len() >= FRAME_SAMPLES_INTERLEAVED,
            "decode buffer too small: {} < {}",
            pcm.len(),
            FRAME_SAMPLES_INTERLEAVED,
        );
        let n = self
            .dec
            .decode_float(opus_bytes, pcm, false)
            .context("opus decode")?;
        // `n` is samples per channel; multiply by channels for the
        // interleaved length the caller just wrote.
        Ok(n * CHANNELS as usize)
    }

    /// Synthesize one concealment frame for a lost packet (Opus PLC).
    /// Pass an empty input slice; libopus extrapolates from history.
    pub fn decode_plc(&mut self, pcm: &mut [f32]) -> Result<usize> {
        anyhow::ensure!(
            pcm.len() >= FRAME_SAMPLES_INTERLEAVED,
            "plc buffer too small"
        );
        let n = self.dec.decode_float(&[], pcm, false).context("opus plc")?;
        Ok(n * CHANNELS as usize)
    }

    /// Recover a lost frame from the FEC data embedded in the NEXT
    /// received packet. Call this with the next packet's bytes when a
    /// single packet was lost; then decode that same next packet
    /// normally afterwards.
    pub fn decode_fec(&mut self, next_opus: &[u8], pcm: &mut [f32]) -> Result<usize> {
        anyhow::ensure!(
            pcm.len() >= FRAME_SAMPLES_INTERLEAVED,
            "fec buffer too small"
        );
        let n = self
            .dec
            .decode_float(next_opus, pcm, true)
            .context("opus fec decode")?;
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

fn channels() -> opus::Channels {
    match CHANNELS {
        1 => opus::Channels::Mono,
        2 => opus::Channels::Stereo,
        _ => unreachable!("CHANNELS is fixed to 1 or 2 at the type level"),
    }
}

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

    #[test]
    fn frames_lost_counts_the_gap() {
        assert_eq!(frames_lost(None, 0), 0);
        assert_eq!(frames_lost(Some(4), 5), 0); // contiguous
        assert_eq!(frames_lost(Some(4), 7), 2); // dropped 5,6
        assert_eq!(frames_lost(Some(9), 9), 0); // duplicate -> caller drops
    }

    #[test]
    fn silence_converges_to_a_tiny_dtx_payload() {
        let mut encoder = OpusEncoder::new(48_000, true).unwrap();
        let silence = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        let mut last = Vec::new();
        for _ in 0..25 {
            last = encoder.encode(&silence).unwrap();
        }
        assert!(last.len() <= 3, "DTX silence payload was {} bytes", last.len());
    }
}
