//! Linear-interpolated resample + simple channel-map.
//!
//! Used by both the WASAPI loopback capture (Win) and the PipeWire
//! monitor capture (Linux) to normalise device-native PCM into the
//! canonical 48 kHz / stereo / f32 wire format before Opus encoding.
//! Good enough for transparent system-sound monitoring at typical
//! rate ratios (44.1↔48 kHz, 96↔48 kHz). Polyphase / sinc resampling
//! lives in M3 polish.

/// `input` is interleaved PCM at `in_rate` with `in_ch` channels.
/// `output` is filled with interleaved PCM at `out_rate` with `out_ch`
/// channels — the caller sizes it for `output_frames * out_ch` and
/// the function fills exactly that many.
///
/// Channel mapping:
///   * mono → stereo: duplicate sample to both channels
///   * stereo → mono: take left channel only (cheap; a real downmix
///     would average L/R)
///   * matched: pass through unchanged
///   * anything else: only the first 2 channels are touched.
pub fn resample_and_chmap(
    input: &[f32],
    in_ch: u16,
    in_rate: u32,
    output: &mut [f32],
    out_ch: u16,
    out_rate: u32,
) {
    let in_frames = input.len() / in_ch as usize;
    let out_frames = output.len() / out_ch as usize;
    if in_frames == 0 || out_frames == 0 {
        return;
    }
    let ratio = in_rate as f64 / out_rate as f64;
    let in_ch_us = in_ch as usize;
    let out_ch_us = out_ch as usize;

    for o in 0..out_frames {
        let src_pos = o as f64 * ratio;
        let src_lo = (src_pos.floor() as usize).min(in_frames - 1);
        let src_hi = (src_lo + 1).min(in_frames - 1);
        let frac = (src_pos - src_lo as f64) as f32;

        let mut samples = [0f32; 2];
        for c in 0..in_ch_us.min(2) {
            let v_lo = input[src_lo * in_ch_us + c];
            let v_hi = input[src_hi * in_ch_us + c];
            samples[c] = v_lo + (v_hi - v_lo) * frac;
        }
        if in_ch == 1 {
            samples[1] = samples[0];
        }

        for c in 0..out_ch_us.min(2) {
            output[o * out_ch_us + c] = samples[c];
        }
    }
}
