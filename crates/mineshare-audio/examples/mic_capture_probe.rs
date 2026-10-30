//! Native default-microphone smoke test. Saves no audio or microphone contents.
use mineshare_audio::{
    AudioCapture, CaptureSink, FRAME_SAMPLES_INTERLEAVED, codec::OpusDecoder, cpal_mic::CpalMic,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let active = Arc::new(AtomicBool::new(true));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let demand = active.clone();
    let mut capture = CpalMic::new()?;
    capture.start(CaptureSink::new(tx, move || demand.load(Ordering::Acquire)))?;
    let started = Instant::now();
    let mut last = None;
    let mut intervals = Vec::new();
    let mut decoder = OpusDecoder::new()?;
    let mut pcm = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
    let result = async {
        for _ in 0..250 {
            let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("microphone worker stopped"))?;
            let now = Instant::now();
            if let Some(previous) = last {
                intervals.push(now.duration_since(previous).as_secs_f64() * 1000.0);
            }
            last = Some(now);
            let decoded = decoder.decode(&frame.opus, &mut pcm)?;
            anyhow::ensure!(
                decoded == FRAME_SAMPLES_INTERLEAVED && pcm.iter().all(|v| v.is_finite()),
                "invalid decoded microphone frame"
            );
        }
        anyhow::Ok(())
    }
    .await;
    active.store(false, Ordering::Release);
    drop(rx);
    tokio::time::sleep(Duration::from_millis(300)).await;
    result?;
    intervals.sort_by(f64::total_cmp);
    println!(
        "{{\"frames\":250,\"decoded_samples_per_frame\":{FRAME_SAMPLES_INTERLEAVED},\"elapsed_s\":{},\"arrival_p50_ms\":{},\"arrival_p95_ms\":{},\"arrival_max_ms\":{},\"audio_saved\":false}}",
        started.elapsed().as_secs_f64(),
        intervals[124],
        intervals[236],
        intervals[248]
    );
    Ok(())
}
