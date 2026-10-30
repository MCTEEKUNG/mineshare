//! Device PCM to consecutive canonical microphone frames.

use ringbuf::traits::{Consumer, Observer};

use crate::{CHANNELS, FRAME_SAMPLES_INTERLEAVED, FRAME_SAMPLES_PER_CHANNEL, SAMPLE_RATE};

pub struct MicPcm {
    rate: u32,
    channels: u16,
    input: Vec<f32>,
    // Source position as an exact remainder over SAMPLE_RATE. The ring keeps
    // interpolation lookahead; it is never consumed twice or thrown away.
    phase: u64,
}

impl MicPcm {
    pub fn new(rate: u32, channels: u16) -> anyhow::Result<Self> {
        anyhow::ensure!(rate > 0 && channels > 0, "invalid microphone format");
        let frames = (rate as u64 * FRAME_SAMPLES_PER_CHANNEL as u64).div_ceil(SAMPLE_RATE as u64)
            as usize
            + 2;
        Ok(Self {
            rate,
            channels,
            input: vec![0.0; frames * channels as usize],
            phase: 0,
        })
    }

    fn consumed_frames(&self) -> usize {
        ((self.phase + FRAME_SAMPLES_PER_CHANNEL as u64 * self.rate as u64) / SAMPLE_RATE as u64)
            as usize
    }

    pub fn input_samples_needed(&self) -> usize {
        let last = self.phase + (FRAME_SAMPLES_PER_CHANNEL as u64 - 1) * self.rate as u64;
        let lookahead = (last / SAMPLE_RATE as u64) as usize
            + 1
            + usize::from(!last.is_multiple_of(SAMPLE_RATE as u64));
        lookahead.max(self.consumed_frames()) * self.channels as usize
    }

    pub fn read_frame(
        &mut self,
        consumer: &mut ringbuf::HeapCons<f32>,
        output: &mut [f32],
    ) -> bool {
        assert_eq!(output.len(), FRAME_SAMPLES_INTERLEAVED);
        let needed = self.input_samples_needed();
        if consumer.occupied_len() < needed {
            return false;
        }
        if self.rate == SAMPLE_RATE && self.channels == CHANNELS {
            consumer.pop_slice(output);
            return true;
        }
        consumer.peek_slice(&mut self.input[..needed]);
        let mut position = self.phase;
        for frame in output.chunks_exact_mut(CHANNELS as usize) {
            let lo = (position / SAMPLE_RATE as u64) as usize;
            let fraction = (position % SAMPLE_RATE as u64) as f32 / SAMPLE_RATE as f32;
            for (channel, sample) in frame.iter_mut().enumerate() {
                let input_channel = channel.min(self.channels as usize - 1);
                let a = self.input[lo * self.channels as usize + input_channel];
                *sample = if fraction == 0.0 {
                    a
                } else {
                    let b = self.input[(lo + 1) * self.channels as usize + input_channel];
                    a + (b - a) * fraction
                };
            }
            position += self.rate as u64;
        }
        consumer.skip(self.consumed_frames() * self.channels as usize);
        self.phase = position % SAMPLE_RATE as u64;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Producer, Split};

    #[test]
    fn consecutive_same_rate_frames_preserve_every_sample() {
        let (mut producer, mut consumer) = ringbuf::HeapRb::<f32>::new(6_000).split();
        let input: Vec<_> = (0..2_000).flat_map(|i| [i as f32, -(i as f32)]).collect();
        producer.push_slice(&input);
        let mut framer = MicPcm::new(48_000, 2).unwrap();
        let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        for block in 0..2 {
            assert!(framer.read_frame(&mut consumer, &mut out));
            assert_eq!(&out, &input[block * out.len()..(block + 1) * out.len()]);
        }
        assert_eq!(consumer.occupied_len(), input.len() - 2 * out.len());
    }

    #[test]
    fn fractional_rates_keep_phase_and_lookahead_across_blocks() {
        for rate in [8_000, 16_000, 44_100, 47_999, 48_000, 96_000] {
            for channels in [1, 2, 6] {
                let (mut producer, mut consumer) = ringbuf::HeapRb::<f32>::new(30_000).split();
                let mut framer = MicPcm::new(rate, channels).unwrap();
                let mut next_source = 0usize;
                let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
                for block in 0..100 {
                    let missing = framer.input_samples_needed() - consumer.occupied_len();
                    for _ in 0..missing / channels as usize {
                        for c in 0..channels {
                            producer
                                .try_push(next_source as f32 + c as f32 * 1_000_000.0)
                                .unwrap();
                        }
                        next_source += 1;
                    }
                    assert!(framer.read_frame(&mut consumer, &mut out));
                    for (i, sample) in out.chunks_exact(2).enumerate() {
                        let expected = (block * 960 + i) as f64 * rate as f64 / 48_000.0;
                        assert!(
                            (sample[0] as f64 - expected).abs() < 0.02,
                            "rate={rate}, channels={channels}, block={block}, i={i}"
                        );
                        let right = expected + if channels == 1 { 0.0 } else { 1_000_000.0 };
                        assert!((sample[1] as f64 - right).abs() < 0.13);
                    }
                    let consumed = (block + 1) * 960 * rate as usize / 48_000;
                    assert_eq!(
                        next_source - consumer.occupied_len() / channels as usize,
                        consumed
                    );
                }
            }
        }
    }

    #[test]
    fn partial_input_does_not_advance_the_stream() {
        let (mut producer, mut consumer) = ringbuf::HeapRb::<f32>::new(4_000).split();
        let mut framer = MicPcm::new(44_100, 1).unwrap();
        let needed = framer.input_samples_needed();
        producer.push_slice(&vec![0.5; needed - 1]);
        let mut out = vec![f32::NAN; FRAME_SAMPLES_INTERLEAVED];
        assert!(!framer.read_frame(&mut consumer, &mut out));
        assert_eq!(consumer.occupied_len(), needed - 1);
        assert!(out.iter().all(|sample| sample.is_nan()));
        producer.try_push(0.5).unwrap();
        assert!(framer.read_frame(&mut consumer, &mut out));
        assert!(out.iter().all(|&sample| sample == 0.5));
    }
}
