//! Capture-only diagnostic: run with synthetic audio; timestamps are encoded-frame
//! handoff estimates, not native endpoint latency measurements.
use mineshare_audio::{CaptureSink, FRAME_SAMPLES_INTERLEAVED, codec::OpusDecoder};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let folder = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("output folder required"))?,
    );
    std::fs::create_dir_all(&folder)?;
    let mut raw = BufWriter::new(File::create(folder.join("captured.f32"))?);
    let mut index = BufWriter::new(File::create(folder.join("captured.jsonl"))?);
    let active = Arc::new(AtomicBool::new(true));
    let demand = active.clone();
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let mut capture = mineshare_audio::make_sysout_capture()?;
    capture.start(CaptureSink::new(tx, move || demand.load(Ordering::Acquire)))?;
    let base = Instant::now();
    let utc = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as i128;
    let mut decoder = OpusDecoder::new()?;
    let mut pcm = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
    let mut offset = 0u64;
    while base.elapsed() < Duration::from_secs(46) {
        let frame = match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => continue,
        };
        let n = decoder.decode(&frame.opus, &mut pcm)?;
        let timestamp = utc + frame.captured_at.saturating_duration_since(base).as_nanos() as i128;
        for sample in &pcm[..n] {
            raw.write_all(&sample.to_le_bytes())?;
        }
        writeln!(
            index,
            "{{\"offset\":{offset},\"frames\":{},\"t_ns\":{timestamp},\"seq\":{},\"flags\":0,\"valid\":true}}",
            n / 2,
            frame.seq
        )?;
        offset += (n / 2) as u64;
    }
    active.store(false, Ordering::Release);
    raw.flush()?;
    index.flush()?;
    println!(
        "{{\"sample_frames\":{offset},\"duration_s\":{},\"timestamp_kind\":\"encoded handoff estimate\"}}",
        base.elapsed().as_secs_f64()
    );
    Ok(())
}
