//! `cpal` playback sink — receives Opus frames, decodes, pushes into
//! a ring buffer, and a `cpal` output stream pulls samples from it on
//! the audio device's callback thread.
//!
//! On Windows `cpal::Stream` is `!Send` (it holds a COM pointer), so
//! the stream lives on a dedicated OS thread and the public handle
//! holds only the `Sender` half of an mpsc channel — that part *is*
//! `Send + Sync` and so satisfies our [`AudioPlayback`] trait bound.
//!
//! Underrun handling: if the ring buffer empties before more frames
//! arrive, the callback fills with silence (zeros). Better than
//! introducing latency by blocking the device.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use tracing::{debug, info, warn};

use crate::codec::OpusDecoder;
use crate::{AudioFrame, AudioPlayback, CHANNELS, FRAME_SAMPLES_INTERLEAVED, SAMPLE_RATE};

/// Ring-buffer capacity in interleaved samples. ~10 frames @ 20 ms =
/// 200 ms. Generous enough to absorb network jitter, tight enough not
/// to feel laggy.
const RING_CAPACITY: usize = FRAME_SAMPLES_INTERLEAVED * 10;

pub struct CpalPlayback {
    tx: mpsc::Sender<AudioFrame>,
}

impl CpalPlayback {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<AudioFrame>();
        // Hand the device thread a one-shot status channel so we can
        // surface init errors back to the caller synchronously.
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        thread::Builder::new()
            .name("cpal-playback".into())
            .spawn(move || {
                run_playback_thread(rx, ready_tx);
            })
            .context("spawn cpal-playback thread")?;
        ready_rx
            .recv_timeout(Duration::from_secs(3))
            .context("cpal-playback thread did not signal readiness")??;
        Ok(Self { tx })
    }
}

impl AudioPlayback for CpalPlayback {
    fn enqueue(&self, frame: AudioFrame) -> Result<()> {
        // mpsc::Sender::send only fails if the receiver is dropped,
        // which means the playback thread is gone — surface that as
        // an error so the caller stops trying.
        self.tx
            .send(frame)
            .map_err(|_| anyhow::anyhow!("cpal-playback thread terminated"))
    }
}

fn run_playback_thread(rx: mpsc::Receiver<AudioFrame>, ready: mpsc::Sender<Result<()>>) {
    let stream = match build_stream() {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    // Stream is constructed and started; now drain frames from the
    // mpsc receiver, decode them, and push samples into the ring
    // shared with the cpal callback.
    let StreamCtx {
        stream,
        producer,
    } = stream;
    let mut decoder = match OpusDecoder::new() {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "opus decoder init failed inside playback thread");
            return;
        }
    };
    let mut scratch = vec![0f32; FRAME_SAMPLES_INTERLEAVED];
    let mut producer = producer;

    while let Ok(frame) = rx.recv() {
        let n = match decoder.decode(&frame.opus, &mut scratch) {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "opus decode failed — dropping frame");
                continue;
            }
        };
        let pushed = producer.push_slice(&scratch[..n]);
        if pushed != n {
            warn!(
                dropped = n - pushed,
                "cpal playback ring full — dropping samples"
            );
        }
    }
    // mpsc dropped → handle dropped → stop the stream and exit.
    drop(stream);
    info!("cpal playback thread exiting");
}

/// Wraps the cpal stream with the producer-half of the ring buffer
/// the decoded-frame loop pushes into. Both fields stay on the
/// playback thread — `Stream` is `!Send` on Windows.
struct StreamCtx {
    stream: cpal::Stream,
    producer: ringbuf::HeapProd<f32>,
}

fn build_stream() -> Result<StreamCtx> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default audio output device")?;
    let device_name = device.name().unwrap_or_else(|_| "?".to_string());

    let config = pick_config(&device)?;
    info!(
        device = %device_name,
        sample_rate = config.sample_rate().0,
        channels = config.channels(),
        sample_format = ?config.sample_format(),
        "cpal playback device picked"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY);
    let (producer, mut consumer) = rb.split();

    let err_fn = |e| warn!(error = %e, "cpal playback stream error");
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_output_stream(
            &config.config(),
            move |out: &mut [f32], _| {
                fill_callback(out, &mut consumer);
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => {
            let mut tmp = vec![0f32; 0];
            device.build_output_stream(
                &config.config(),
                move |out: &mut [i16], _| {
                    tmp.resize(out.len(), 0.0);
                    fill_callback(&mut tmp, &mut consumer);
                    for (dst, &src) in out.iter_mut().zip(tmp.iter()) {
                        *dst = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    }
                },
                err_fn,
                None,
            )
        }
        other => anyhow::bail!("unsupported cpal sample format: {other:?}"),
    }
    .context("build cpal output stream")?;
    stream.play().context("cpal play")?;

    Ok(StreamCtx { stream, producer })
}

fn fill_callback(out: &mut [f32], consumer: &mut ringbuf::HeapCons<f32>) {
    let popped = consumer.pop_slice(out);
    for s in &mut out[popped..] {
        *s = 0.0;
    }
    if popped < out.len() {
        debug!(
            underrun = out.len() - popped,
            "cpal playback ring underrun (filled with silence)"
        );
    }
}

fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    // Prefer 48 kHz / 2-channel / f32 — the canonical bridge format.
    // Fall back to whatever the device's default is if we can't get
    // an exact match.
    let supported = device
        .supported_output_configs()
        .context("query supported output configs")?
        .collect::<Vec<_>>();

    let target_rate = cpal::SampleRate(SAMPLE_RATE);
    if let Some(matched) = supported.iter().find(|c| {
        c.channels() == CHANNELS
            && c.min_sample_rate() <= target_rate
            && target_rate <= c.max_sample_rate()
            && c.sample_format() == SampleFormat::F32
    }) {
        return Ok(matched.clone().with_sample_rate(target_rate));
    }
    if let Some(matched) = supported.iter().find(|c| {
        c.channels() == CHANNELS
            && c.min_sample_rate() <= target_rate
            && target_rate <= c.max_sample_rate()
    }) {
        return Ok(matched.clone().with_sample_rate(target_rate));
    }
    let default = device
        .default_output_config()
        .context("no default output config")?;
    warn!(
        rate = default.sample_rate().0,
        channels = default.channels(),
        format = ?default.sample_format(),
        "no exact match for 48 kHz stereo f32 — falling back to device default"
    );
    Ok(default)
}
