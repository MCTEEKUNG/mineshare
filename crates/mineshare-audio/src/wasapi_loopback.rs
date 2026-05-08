//! Windows sysout capture via WASAPI **loopback**.
//!
//! cpal opens the default output device with `AUDCLNT_STREAMFLAGS_LOOPBACK`
//! when you build an *input* stream on an output device — it then
//! delivers whatever the system mixer is rendering, which is exactly
//! what we want to forward to the peer.
//!
//! ## Format adaptation
//!
//! WASAPI shared-mode loopback emits the *system mixer format*, which
//! varies device to device (44.1 kHz / 48 kHz / 96 kHz; mono, stereo,
//! 5.1; f32 or i16). We always normalise to the canonical 48 kHz
//! stereo f32 wire format before Opus-encoding:
//!   * mono → stereo by duplication
//!   * non-48 kHz rates → linear-interpolated 48 kHz
//!   * surround layouts above 2 channels are rejected for now (a
//!     proper downmix matrix is M3 polish work)
//!
//! ## Threading
//!
//! cpal's `Stream` is `!Send` on Windows (it holds a COM pointer), so
//! the whole capture pipeline runs on a dedicated OS thread; we hand
//! encoded frames out via a Tokio mpsc to the runtime.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use crate::codec::OpusEncoder;
use crate::{
    AudioCapture, AudioFrame, CHANNELS, FRAME_SAMPLES_INTERLEAVED, FRAME_SAMPLES_PER_CHANNEL,
    SAMPLE_RATE, StreamKind,
};

/// Capture-side ring capacity in *interleaved* samples. ~5 frames at
/// the highest plausible mixer rate (96 kHz stereo) — large enough to
/// absorb scheduling jitter on the audio callback, small enough that
/// any drop is recent (low latency on overrun).
const CAPTURE_RING_CAP: usize = 96_000 / 50 * 2 * 5;
/// Opus bitrate target — 96 kbps stereo is transparent for music and
/// cheap on the wire.
const OPUS_BITRATE_BPS: i32 = 96_000;

pub struct WasapiLoopback {
    started: bool,
}

impl WasapiLoopback {
    pub fn new() -> Result<Self> {
        Ok(Self { started: false })
    }
}

impl AudioCapture for WasapiLoopback {
    fn start(&mut self, sink: UnboundedSender<AudioFrame>) -> Result<()> {
        if self.started {
            return Ok(());
        }
        self.started = true;

        thread::Builder::new()
            .name("wasapi-loopback".into())
            .spawn(move || {
                if let Err(e) = run_capture_thread(sink) {
                    warn!(error = %e, "WASAPI loopback capture thread exited");
                }
            })
            .context("spawn wasapi loopback thread")?;
        Ok(())
    }
}

fn run_capture_thread(sink: UnboundedSender<AudioFrame>) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default audio output device for WASAPI loopback")?;
    let device_name = device.name().unwrap_or_else(|_| "?".to_string());

    // The shared-mode mixer format — that's what loopback actually
    // delivers. cpal builds an input stream on an *output* device with
    // AUDCLNT_STREAMFLAGS_LOOPBACK, but the data shape matches the
    // output's render format, so we query that instead of the input
    // configs (which often don't include the mixer rate).
    let cfg = device
        .default_output_config()
        .context("query default loopback config")?;
    let in_rate = cfg.sample_rate().0;
    let in_channels = cfg.channels();
    info!(
        device = %device_name,
        sample_rate = in_rate,
        channels = in_channels,
        sample_format = ?cfg.sample_format(),
        "WASAPI loopback config picked"
    );

    if in_channels == 0 || in_channels > 2 {
        anyhow::bail!(
            "unsupported loopback channel count: {} — only mono/stereo are handled in Slice 1 \
             (downmix matrix lands in M3 polish)",
            in_channels
        );
    }

    let rb = HeapRb::<f32>::new(CAPTURE_RING_CAP);
    let (producer, consumer) = rb.split();
    let producer = Arc::new(Mutex::new(producer));
    let producer_cb = producer.clone();
    let err_fn = |e| warn!(error = %e, "WASAPI loopback stream error");

    let stream = match cfg.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            &cfg.config(),
            move |data: &[f32], _| {
                let mut p = producer_cb.lock();
                let written = p.push_slice(data);
                if written != data.len() {
                    debug!(
                        dropped = data.len() - written,
                        "wasapi capture ring full — dropping samples"
                    );
                }
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &cfg.config(),
            move |data: &[i16], _| {
                let mut p = producer_cb.lock();
                for &s in data {
                    let f = s as f32 / i16::MAX as f32;
                    if p.try_push(f).is_err() {
                        break;
                    }
                }
            },
            err_fn,
            None,
        ),
        other => anyhow::bail!("unsupported loopback sample format: {other:?}"),
    }
    .context("build WASAPI loopback input stream")?;
    stream.play().context("cpal play (loopback)")?;
    info!("WASAPI loopback stream started");

    drive_encode_loop(consumer, sink, in_rate, in_channels)?;

    drop(stream);
    Ok(())
}

fn drive_encode_loop(
    mut consumer: ringbuf::HeapCons<f32>,
    sink: UnboundedSender<AudioFrame>,
    in_rate: u32,
    in_channels: u16,
) -> Result<()> {
    let mut encoder = OpusEncoder::new(OPUS_BITRATE_BPS)?;
    // Number of input *frames* (one frame = one sample per channel)
    // we need to fill one 20 ms output frame after resampling. Round
    // up so we never under-feed the resampler.
    let in_frames_per_out = ((in_rate as u64 * FRAME_SAMPLES_PER_CHANNEL as u64
        + SAMPLE_RATE as u64 / 2)
        / SAMPLE_RATE as u64) as usize
        + 1;
    let in_samples_per_out = in_frames_per_out * in_channels as usize;

    let mut in_buf = vec![0f32; in_samples_per_out];
    let mut out_buf = vec![0f32; FRAME_SAMPLES_INTERLEAVED];
    let mut seq: u32 = 0;
    let needs_resample = in_rate != SAMPLE_RATE;
    let needs_chmap = in_channels != CHANNELS;
    if needs_resample || needs_chmap {
        info!(
            in_rate,
            in_channels,
            out_rate = SAMPLE_RATE,
            out_channels = CHANNELS,
            "WASAPI loopback will resample/channel-map every frame"
        );
    }

    loop {
        while consumer.occupied_len() < in_samples_per_out {
            std::thread::sleep(Duration::from_millis(2));
        }
        let n = consumer.pop_slice(&mut in_buf);
        if n < in_samples_per_out {
            continue;
        }

        crate::resample::resample_and_chmap(
            &in_buf[..n],
            in_channels,
            in_rate,
            &mut out_buf,
            CHANNELS,
            SAMPLE_RATE,
        );

        let opus_bytes = match encoder.encode(&out_buf) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "opus encode failed — skipping frame");
                continue;
            }
        };
        let frame = AudioFrame {
            stream: StreamKind::SysOut,
            seq,
            opus: opus_bytes,
        };
        seq = seq.wrapping_add(1);

        if sink.send(frame).is_err() {
            info!("audio sink closed — stopping WASAPI loopback");
            return Ok(());
        }
    }
}

