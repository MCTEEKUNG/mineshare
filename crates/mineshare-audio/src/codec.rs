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
    pub fn new(bitrate_bps: i32) -> Result<Self> {
        let mut enc =
            opus::Encoder::new(SAMPLE_RATE, channels(), opus::Application::Audio)
                .context("opus encoder init")?;
        enc.set_bitrate(opus::Bitrate::Bits(bitrate_bps))
            .context("opus set bitrate")?;
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
}

fn channels() -> opus::Channels {
    match CHANNELS {
        1 => opus::Channels::Mono,
        2 => opus::Channels::Stereo,
        _ => unreachable!("CHANNELS is fixed to 1 or 2 at the type level"),
    }
}
