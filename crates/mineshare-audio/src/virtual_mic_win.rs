//! Windows side of the **virtual mic** sink — VB-CABLE.
//!
//! VB-CABLE is a free third-party virtual audio device that exposes a
//! pair of WDM endpoints:
//!   * `CABLE Input` — appears as a *playback* device. We render the
//!     peer's mic frames into it.
//!   * `CABLE Output` — the matching *capture* device. Apps like
//!     Discord / Zoom / OBS pick it as their microphone, and what
//!     they "hear" is whatever we wrote into `CABLE Input`.
//!
//! VB-CABLE is donationware (https://vb-audio.com/Cable/) and is not
//! redistributable inside our installer, so the daemon detects it at
//! runtime and surfaces a clear log line with the install URL when
//! it's missing. The bridge keeps working without it — only the
//! "peer's mic shows up in my apps as a mic device" feature is
//! disabled until the user installs VB-CABLE separately.
//!
//! Detection: enumerate cpal's WASAPI output devices and match the
//! one whose name contains `"CABLE Input"` (case-insensitive). The
//! device's friendly name is the user-visible one in Sound Settings,
//! so the match is stable across VB-CABLE versions.

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

const RING_CAPACITY: usize = FRAME_SAMPLES_INTERLEAVED * 10;

pub struct VbCablePlayback {
    tx: mpsc::Sender<AudioFrame>,
}

impl VbCablePlayback {
    pub fn new() -> Result<Self> {
        let device = find_cable_input_device().with_context(|| {
            "VB-CABLE not detected — install from https://vb-audio.com/Cable/ \
             then restart the daemon. Without it, peer mic frames arrive \
             but apps on this machine can't pick them up as a mic input."
        })?;

        let (tx, rx) = mpsc::channel::<AudioFrame>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

        thread::Builder::new()
            .name("vb-cable-playback".into())
            .spawn(move || {
                run_playback_thread(device, rx, ready_tx);
            })
            .context("spawn vb-cable-playback thread")?;

        ready_rx
            .recv_timeout(Duration::from_secs(3))
            .context("vb-cable-playback thread did not signal readiness")??;
        Ok(Self { tx })
    }
}

impl AudioPlayback for VbCablePlayback {
    fn enqueue(&self, frame: AudioFrame) -> Result<()> {
        self.tx
            .send(frame)
            .map_err(|_| anyhow::anyhow!("vb-cable-playback thread terminated"))
    }
}

fn find_cable_input_device() -> Option<cpal::Device> {
    let host = cpal::default_host();
    let mut all = Vec::new();
    let outs = host.output_devices().ok()?;
    let mut hit: Option<cpal::Device> = None;
    for d in outs {
        let name = d.name().unwrap_or_else(|_| "?".to_string());
        all.push(name.clone());
        if hit.is_none() && name.to_ascii_lowercase().contains("cable input") {
            hit = Some(d);
            info!(device = %name, "matched VB-CABLE Input device for virtual mic");
        }
    }
    if hit.is_none() {
        debug!(devices = ?all, "no 'CABLE Input' device among cpal outputs");
    }
    hit
}

fn run_playback_thread(
    device: cpal::Device,
    rx: mpsc::Receiver<AudioFrame>,
    ready: mpsc::Sender<Result<()>>,
) {
    let stream = match build_stream(&device) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let StreamCtx {
        stream,
        mut producer,
    } = stream;
    let mut decoder = match OpusDecoder::new() {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "opus decoder init failed inside vb-cable thread");
            return;
        }
    };
    let mut scratch = vec![0f32; FRAME_SAMPLES_INTERLEAVED];

    while let Ok(frame) = rx.recv() {
        let n = match decoder.decode(&frame.opus, &mut scratch) {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "opus decode failed (mic) — dropping frame");
                continue;
            }
        };
        let pushed = producer.push_slice(&scratch[..n]);
        if pushed != n {
            warn!(
                dropped = n - pushed,
                "vb-cable ring full — dropping mic samples"
            );
        }
    }
    drop(stream);
    info!("vb-cable-playback thread exiting");
}

struct StreamCtx {
    stream: cpal::Stream,
    producer: ringbuf::HeapProd<f32>,
}

fn build_stream(device: &cpal::Device) -> Result<StreamCtx> {
    let device_name = device.name().unwrap_or_else(|_| "?".to_string());
    let config = pick_config(device)?;
    info!(
        device = %device_name,
        sample_rate = config.sample_rate().0,
        channels = config.channels(),
        sample_format = ?config.sample_format(),
        "vb-cable playback config picked"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY);
    let (producer, mut consumer) = rb.split();

    let err_fn = |e| warn!(error = %e, "vb-cable stream error");
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
        other => anyhow::bail!("unsupported vb-cable sample format: {other:?}"),
    }
    .context("build vb-cable output stream")?;
    stream.play().context("cpal play (vb-cable)")?;

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
            "vb-cable ring underrun (filled with silence)"
        );
    }
}

fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let supported = device
        .supported_output_configs()
        .context("query VB-CABLE configs")?
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
        .context("VB-CABLE has no default output config")?;
    warn!(
        rate = default.sample_rate().0,
        channels = default.channels(),
        format = ?default.sample_format(),
        "VB-CABLE doesn't expose 48 kHz stereo natively — falling back to default"
    );
    Ok(default)
}
