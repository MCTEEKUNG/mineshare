//! Opus encoder / decoder wrappers.
//!
//! 48 kHz stereo, 20 ms frames, ~96 kbps target. Both sides agree on
//! the format implicitly via the canonical `SAMPLE_RATE` / `CHANNELS`
//! constants in the parent module — no negotiation messages.

use anyhow::{Context, Result};

use crate::{CHANNELS, FRAME_SAMPLES_INTERLEAVED, SAMPLE_RATE, StreamKind};

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
    /// Voice uses FEC and DTX. System audio must preserve quiet music and
    /// short sounds even after the encoder has learned a loud noise floor.
    pub fn new(bitrate_bps: i32, stream: StreamKind) -> Result<Self> {
        let voice = matches!(stream, StreamKind::Mic);
        let mut enc = opus::Encoder::new(SAMPLE_RATE, channels(), opus::Application::Audio)
            .context("opus encoder init")?;
        enc.set_bitrate(opus::Bitrate::Bits(bitrate_bps))
            .context("opus set bitrate")?;
        // Explicit VBR — libopus defaults to VBR on, but set it so the
        // intent is visible and stable across crate versions.
        enc.set_vbr(true).context("opus set vbr")?;
        // Speech DTX can mistake quiet system sounds for background noise.
        // CELT's compact digital-silence payloads remain transport-suppressed
        // without DTX; microphone speech keeps its existing DTX/FEC behavior.
        enc.set_dtx(voice).context("opus set dtx")?;
        if voice {
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
            ?stream,
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
        let mut enc = OpusEncoder::new(128_000, StreamKind::Mic).expect("encoder");
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
        let mut encoder = OpusEncoder::new(48_000, StreamKind::Mic).unwrap();
        let silence = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        let mut last = Vec::new();
        for _ in 0..25 {
            last = encoder.encode(&silence).unwrap();
        }
        assert!(
            last.len() <= 3,
            "DTX silence payload was {} bytes",
            last.len()
        );
    }

    #[test]
    fn quiet_sysout_bursts_survive_after_loud_noise() {
        let mut encoder = OpusEncoder::new(128_000, StreamKind::SysOut).unwrap();
        let mut noise = 123_456_789u32;
        let mut frame = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        let mut fill_noise = |frame: &mut [f32], gain: f32| {
            for stereo in frame.chunks_exact_mut(2) {
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let value = ((noise >> 8) as f32 / 8_388_608.0 - 1.0) * gain;
                stereo.fill(value);
            }
        };
        // Reproduce the noise-floor history of an existing audio session,
        // then short quiet notifications separated by digital silence.
        for _ in 0..500 {
            fill_noise(&mut frame, 0.3);
            encoder.encode(&frame).unwrap();
        }
        let mut audible_bursts = 0;
        for _ in 0..64 {
            let mut audible = false;
            for tick in 0..25 {
                if tick < 2 {
                    fill_noise(&mut frame, 0.0006);
                } else {
                    frame.fill(0.0);
                }
                let payload = encoder.encode(&frame).unwrap();
                if tick < 4 && payload.len() > 3 {
                    audible = true;
                }
            }
            audible_bursts += usize::from(audible);
        }
        assert_eq!(audible_bursts, 64, "quiet sysout was suppressed as silence");
    }

    #[test]
    fn sysout_digital_silence_stays_transport_suppressed() {
        let mut encoder = OpusEncoder::new(128_000, StreamKind::SysOut).unwrap();
        let silence = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        let mut last = Vec::new();
        for _ in 0..25 {
            last = encoder.encode(&silence).unwrap();
        }
        assert!(
            last.len() <= 3,
            "idle sysout payload was {} bytes",
            last.len()
        );
    }
}
