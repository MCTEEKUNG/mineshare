//! Cross-platform mic capture via the system default input device.
//!
//! Both Linux (PipeWire-backed via cpal/ALSA) and Windows (WASAPI)
//! expose the user's primary microphone as cpal's
//! `default_input_device()` — so unlike sysout (which needs the
//! platform-specific loopback / monitor trick), the mic path is a
//! straight cpal capture on every OS we ship.
//!
//! Format adaptation: real mics rarely run at our canonical 48 kHz
//! stereo. Cheap headsets are mono 16 kHz, USB mics are typically
//! mono 48 kHz, gaming headsets often 16-bit 44.1 kHz. We resample +
//! channel-map to 48 kHz / 2-channel / f32 in the encode loop, same
//! as the WASAPI loopback path.
//!
//! ## Stream tag
//!
//! Frames carry `StreamKind::Mic` so the receiver can route them to
//! a virtual mic input (PipeWire null-sink monitor / VB-CABLE on
//! Windows) instead of mixing them into the sysout speaker path.

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

/// Capture-side ring sized for 96 kHz stereo × 5 frames = covers any
/// realistic mic rate up to 96 kHz with comfortable jitter slack.
const CAPTURE_RING_CAP: usize = 96_000 / 50 * 2 * 5;
/// Speech is fine at 48 kbps stereo Opus — we deliberately under-bit
/// the mic stream relative to sysout (96 kbps) since mics carry
/// monaural voice + room noise, not music.
const OPUS_BITRATE_BPS: i32 = 48_000;

pub struct CpalMic {
    started: bool,
}

impl CpalMic {
    pub fn new() -> Result<Self> {
        Ok(Self { started: false })
    }
}

impl AudioCapture for CpalMic {
    fn start(&mut self, sink: UnboundedSender<AudioFrame>) -> Result<()> {
        if self.started {
            return Ok(());
        }
        self.started = true;

        thread::Builder::new()
            .name("cpal-mic".into())
            .spawn(move || {
                if let Err(e) = run_capture_thread(sink) {
                    warn!(error = %e, "mic capture thread exited");
                }
            })
            .context("spawn cpal-mic thread")?;
        Ok(())
    }
}

fn run_capture_thread(sink: UnboundedSender<AudioFrame>) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no default audio input device — plug in a mic and try again")?;
    let device_name = device.name().unwrap_or_else(|_| "?".to_string());

    let cfg = device
        .default_input_config()
        .context("query default input config")?;
    let in_rate = cfg.sample_rate().0;
    let in_channels = cfg.channels();
    info!(
        device = %device_name,
        sample_rate = in_rate,
        channels = in_channels,
        sample_format = ?cfg.sample_format(),
        "mic capture device picked"
    );

    if in_channels == 0 {
        anyhow::bail!("input device reports 0 channels — driver issue");
    }

    let rb = HeapRb::<f32>::new(CAPTURE_RING_CAP);
    let (producer, consumer) = rb.split();
    let producer = Arc::new(Mutex::new(producer));
    let producer_cb = producer.clone();
    let err_fn = |e| warn!(error = %e, "mic stream error");

    let stream = match cfg.sample_format() {
        SampleFormat::F32 => device.build_input_stream(
            &cfg.config(),
            move |data: &[f32], _| {
                let mut p = producer_cb.lock();
                let written = p.push_slice(data);
                if written != data.len() {
                    debug!(
                        dropped = data.len() - written,
                        "mic ring full — dropping samples"
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
        SampleFormat::I32 => device.build_input_stream(
            &cfg.config(),
            move |data: &[i32], _| {
                let mut p = producer_cb.lock();
                for &s in data {
                    let f = s as f32 / i32::MAX as f32;
                    if p.try_push(f).is_err() {
                        break;
                    }
                }
            },
            err_fn,
            None,
        ),
        other => anyhow::bail!("unsupported mic sample format: {other:?}"),
    }
    .context("build mic input stream")?;
    stream.play().context("cpal play (mic)")?;
    info!("mic stream started");

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
            "mic capture will resample/channel-map every frame"
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
                warn!(error = %e, "mic opus encode failed — skipping frame");
                continue;
            }
        };
        let frame = AudioFrame {
            stream: StreamKind::Mic,
            seq,
            opus: opus_bytes,
        };
        seq = seq.wrapping_add(1);

        if sink.send(frame).is_err() {
            info!("mic sink closed — stopping mic capture");
            return Ok(());
        }
    }
}
