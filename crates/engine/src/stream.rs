//! WASAPI streams with a bounded single-producer, single-consumer queue.
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    Device, DeviceId, SampleFormat, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use orvia_core::{BLOCK, Processor, SAMPLE_RATE, Settings};
use orvia_engine::{is_orvia, label};
use rtrb::{Consumer, RingBuffer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed},
    },
    time::Instant,
};

type Frame = [f32; 2];
type TimedFrame = (Frame, u64);

#[derive(Default)]
struct Counters {
    peak: AtomicU32,
    missing: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicBool,
    captured_ms: AtomicU64,
    played_ms: AtomicU64,
}

pub(super) struct Route {
    _streams: (Stream, Stream),
    pub(super) input_id: DeviceId,
    pub(super) name: String,
    counters: Arc<Counters>,
    start: Instant,
}

pub(super) fn default_input_id() -> Option<DeviceId> {
    cpal::default_host().default_input_device()?.id().ok()
}

impl Route {
    pub(super) fn open(settings: &Settings) -> Result<Self> {
        let host = cpal::default_host();
        let output = host
            .output_devices()?
            .find(is_orvia)
            .context("Orvia Audio v1 is unavailable. Check the driver in Device Manager.")?;
        let input = match &settings.input {
            Some(id) => host
                .device_by_id(&id.parse()?)
                .context("Selected microphone is disconnected")?,
            None => host
                .default_input_device()
                .context("No default microphone")?,
        };
        ensure!(
            !is_orvia(&input),
            "Select a physical microphone to avoid feedback"
        );
        let input_id = input.id()?;
        let name = label(&input)?;
        let input_config = config(&input, true)?;
        let output_config = config(&output, false)?;
        let counters = Arc::new(Counters::default());
        let start = Instant::now();
        let (mut tx, rx) = RingBuffer::<TimedFrame>::new(BLOCK * 10);
        let mut processor = Processor::new(settings);
        let mut block = [[0.; 2]; BLOCK];
        let mut used = 0;
        let meter = counters.clone();
        let channels = usize::from(input_config.channels);
        let mut resampler = InputRate::new(input_config.sample_rate);
        let input_stream = input.build_input_stream(
            &input_config,
            move |samples: &[f32], _| {
                let stamp = start.elapsed().as_millis() as u64;
                meter.captured_ms.store(stamp, Relaxed);
                for frame in samples.chunks_exact(channels) {
                    resampler.push([frame[0], *frame.get(1).unwrap_or(&frame[0])], |sample| {
                        block[used] = sample;
                        used += 1;
                        if used == BLOCK {
                            used = 0;
                            let peak = processor.process(&mut block);
                            meter.peak.fetch_max(peak.to_bits(), Relaxed);
                            for frame in block {
                                if tx.push((frame, stamp)).is_err() {
                                    meter.dropped.fetch_add(1, Relaxed);
                                }
                            }
                        }
                    });
                }
            },
            on_error(&counters),
            None,
        )?;
        let meter = counters.clone();
        let mut playback = Playback::new(rx);
        let output_stream = output.build_output_stream(
            &output_config,
            move |out: &mut [f32], _| {
                let now = start.elapsed().as_millis() as u64;
                meter.played_ms.store(now, Relaxed);
                playback.fill(out, now, &meter);
            },
            on_error(&counters),
            None,
        )?;
        input_stream.play()?;
        output_stream.play()?;
        Ok(Self {
            _streams: (input_stream, output_stream),
            counters,
            input_id,
            name,
            start,
        })
    }

    pub(super) fn healthy(&self) -> bool {
        let now = self.start.elapsed().as_millis() as u64;
        !self.counters.failed.load(Relaxed)
            && now.saturating_sub(self.counters.captured_ms.load(Relaxed)) < 2000
            && now.saturating_sub(self.counters.played_ms.load(Relaxed)) < 2000
    }

    pub(super) fn metrics(&self) -> (f32, u64, u64) {
        (
            f32::from_bits(self.counters.peak.swap(0, Relaxed)),
            self.counters.missing.load(Relaxed),
            self.counters.dropped.load(Relaxed),
        )
    }
}

fn on_error(counters: &Arc<Counters>) -> impl FnMut(cpal::StreamError) + Send + 'static {
    let counters = counters.clone();
    move |_| counters.failed.store(true, Relaxed)
}

fn config(device: &Device, input: bool) -> Result<StreamConfig> {
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    ranges
        .into_iter()
        .filter(|r| r.sample_format() == SampleFormat::F32)
        .filter_map(|r| {
            let rate = r.max_sample_rate().min(SAMPLE_RATE);
            let suitable = if input {
                rate >= 8_000 && r.channels() > 0
            } else {
                rate == SAMPLE_RATE && r.channels() == 2
            };
            (suitable && rate >= r.min_sample_rate()).then(|| r.with_sample_rate(rate).config())
        })
        .max_by_key(|c| (c.sample_rate, c.channels() == 2))
        .ok_or_else(|| {
            anyhow!(
                "{} has no compatible shared format. Try 48 kHz in Windows Sound settings.",
                label(device).unwrap_or_default()
            )
        })
}

/// Convert the capture clock to the model's fixed rate. Integer phase persists
/// between callbacks, including across partial 10 ms blocks.
struct InputRate {
    rate: u32,
    phase: u32,
    previous: Option<Frame>,
}

impl InputRate {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            phase: 0,
            previous: None,
        }
    }

    fn push(&mut self, frame: Frame, mut emit: impl FnMut(Frame)) {
        if self.rate == SAMPLE_RATE {
            emit(frame);
            return;
        }
        let previous = self.previous.replace(frame).unwrap_or(frame);
        while self.phase < SAMPLE_RATE {
            let ratio = self.phase as f32 / SAMPLE_RATE as f32;
            emit(std::array::from_fn(|c| {
                previous[c] + (frame[c] - previous[c]) * ratio
            }));
            self.phase += self.rate;
        }
        self.phase -= SAMPLE_RATE;
    }
}

/// Interpolate between adjacent frames to follow small capture/playback clock
/// differences. After starvation, prime the queue again before resuming.
struct Playback {
    rx: Consumer<TimedFrame>,
    current: Option<TimedFrame>,
    fraction: f64,
    started: bool,
}

impl Playback {
    fn new(rx: Consumer<TimedFrame>) -> Self {
        Self {
            rx,
            current: None,
            fraction: 0.,
            started: false,
        }
    }

    fn fill(&mut self, out: &mut [f32], now: u64, counts: &Counters) {
        const TARGET: usize = 2 * BLOCK;
        out.fill(0.);
        if self
            .current
            .is_some_and(|(_, t)| now.saturating_sub(t) > 100)
        {
            self.current = None;
            self.fraction = 0.;
            counts.dropped.fetch_add(1, Relaxed);
        }
        while self
            .rx
            .peek()
            .is_ok_and(|(_, t)| now.saturating_sub(*t) > 100)
        {
            let _ = self.rx.pop();
            counts.dropped.fetch_add(1, Relaxed);
        }
        let buffered = self.rx.slots();
        if self.current.is_none() {
            if buffered < TARGET {
                if self.started {
                    counts.missing.fetch_add((out.len() / 2) as u64, Relaxed);
                }
                return;
            }
            self.current = self.rx.pop().ok();
            self.started = true;
        }
        let step =
            1. + ((buffered as f64 - TARGET as f64) / (TARGET as f64 * 800.)).clamp(-0.003, 0.003);
        let frame_count = out.len() / 2;
        for (idx, frame) in out.chunks_exact_mut(2).enumerate() {
            let (Some((left, _)), Ok((right, _))) = (self.current, self.rx.peek()) else {
                counts
                    .missing
                    .fetch_add((frame_count - idx) as u64, Relaxed);
                self.current = None;
                self.fraction = 0.;
                break;
            };
            for c in 0..2 {
                frame[c] = left[c] + (right[c] - left[c]) * self.fraction as f32;
            }
            self.fraction += step;
            while self.fraction >= 1. {
                self.current = self.rx.pop().ok();
                self.fraction -= 1.;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_rates_produce_one_second_of_transport_frames() {
        for rate in [8_000, 16_000, 44_100, 48_000] {
            let mut converter = InputRate::new(rate);
            let mut total = 0;
            for _ in 0..rate {
                converter.push([0.125; 2], |f| {
                    assert_eq!(f, [0.125; 2]);
                    total += 1;
                });
            }
            assert_eq!(total, SAMPLE_RATE);
        }
    }
    #[test]
    fn stalled_output_discards_old_audio_and_primes_again() {
        let (mut tx, rx) = RingBuffer::new(3 * BLOCK);
        for _ in 0..2 * BLOCK {
            tx.push(([0.75; 2], 0)).unwrap();
        }
        let mut playback = Playback::new(rx);
        let counts = Counters::default();
        let mut out = [1.; BLOCK * 2];
        playback.fill(&mut out, 10, &counts);
        assert!(out.iter().all(|s| *s == 0.75));
        playback.fill(&mut out, 120, &counts);
        assert!(out.iter().all(|s| *s == 0.));
        assert!(counts.dropped.load(Relaxed) > 0);
        for _ in 0..2 * BLOCK {
            tx.push(([-0.5; 2], 130)).unwrap();
        }
        playback.fill(&mut out, 135, &counts);
        assert!(out.iter().all(|s| *s == -0.5));
    }
}
