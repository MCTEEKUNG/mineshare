//! Deterministic microphone framing/Opus benchmark, without opening a device.
//! `legacy` reproduces the archived pre-round-A read/convert contract.
use mineshare_audio::{FRAME_SAMPLES_INTERLEAVED, StreamKind, codec::OpusEncoder, mic_pcm::MicPcm};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use std::{hint::black_box, time::Instant};

fn percentile(values: &mut [u64], p: f64) -> f64 {
    values.sort_unstable();
    values[((values.len() - 1) as f64 * p).round() as usize] as f64 / 1000.0
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let legacy = args.get(1).is_some_and(|s| s == "legacy");
    let rate: u32 = args
        .get(2)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(48_000);
    let channels: u16 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(2);
    let blocks: usize = args
        .get(4)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(10_000);
    let encode = args.get(5).is_some_and(|s| s == "opus");
    let mut pcm = MicPcm::new(rate, channels)?;
    let legacy_frames = ((rate as u64 * 960 + 24_000) / 48_000) as usize + 1;
    let mut legacy_input = vec![0.0; legacy_frames * channels as usize];
    let mut output = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
    let (mut producer, mut consumer) = ringbuf::HeapRb::<f32>::new(40_000).split();
    let mut encoder = OpusEncoder::new(48_000, StreamKind::Mic)?;
    let mut times = Vec::with_capacity(blocks);
    let mut source_frame = 0usize;
    let mut checksum = 0.0f64;
    let mut encoded_bytes = 0usize;
    let start = Instant::now();
    for block in 0..blocks + 500 {
        let needed = if legacy {
            legacy_input.len()
        } else {
            pcm.input_samples_needed()
        };
        while consumer.occupied_len() < needed {
            let sample = (source_frame as f64 * 523.0 * std::f64::consts::TAU / rate as f64).sin()
                as f32
                * 0.1;
            for _ in 0..channels {
                producer.try_push(sample).unwrap();
            }
            source_frame += 1;
        }
        let tick = Instant::now();
        if legacy {
            consumer.pop_slice(&mut legacy_input);
            mineshare_audio::resample::resample_and_chmap(
                black_box(&legacy_input),
                channels,
                rate,
                &mut output,
                2,
                48_000,
            );
        } else {
            assert!(pcm.read_frame(&mut consumer, black_box(&mut output)));
        }
        if encode {
            encoded_bytes += black_box(encoder.encode(black_box(&output))?).len();
        }
        let elapsed = tick.elapsed().as_nanos() as u64;
        checksum += black_box(output[0]) as f64;
        if block >= 500 {
            times.push(elapsed);
        }
    }
    let consumed_frames = source_frame - consumer.occupied_len() / channels as usize;
    let expected_frames = (blocks + 500) as u64 * 960 * rate as u64 / 48_000;
    println!(
        "{{\"mode\":\"{}\",\"rate\":{rate},\"channels\":{channels},\"opus\":{encode},\"blocks\":{blocks},\"warmup_blocks\":500,\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"max_us\":{},\"consumed_frames\":{consumed_frames},\"expected_frames\":{expected_frames},\"excess_consumed_frames\":{},\"encoded_bytes\":{encoded_bytes},\"wall_s\":{},\"checksum\":{checksum}}}",
        if legacy { "legacy" } else { "fixed" },
        percentile(&mut times, 0.5),
        percentile(&mut times, 0.95),
        percentile(&mut times, 0.99),
        percentile(&mut times, 1.0),
        consumed_frames as i64 - expected_frames as i64,
        start.elapsed().as_secs_f64()
    );
    Ok(())
}
