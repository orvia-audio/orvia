//! WASAPI streams with a bounded single-producer, single-consumer queue.
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    Device, DeviceId, FromSample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use orvia_core::{BLOCK, Processor, SAMPLE_RATE, Settings};
use orvia_engine::{is_orvia, is_virtual_cable, label};
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
        let output = match &settings.output {
            Some(id) => host
                .device_by_id(&id.parse()?)
                .context("Selected virtual output is disconnected")?,
            None => host
                .output_devices()?
                .find(is_orvia)
                .context("Choose a virtual output in microphone settings")?,
        };
        ensure!(
            output.description()?.supports_output() && is_virtual_cable(&output),
            "Select the playback side of a supported virtual cable"
        );
        let input = match &settings.input {
            Some(id) => host
                .device_by_id(&id.parse()?)
                .context("Selected microphone is disconnected")?,
            None => host
                .default_input_device()
                .context("No default microphone")?,
        };
        ensure!(
            input.description()?.supports_input() && !is_virtual_cable(&input),
            "Select a physical microphone to avoid feedback"
        );
        let input_id = input.id()?;
        let name = format!("{} → {}", label(&input)?, label(&output)?);
        let input_config = config(&input, true)?.config();
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
        let output_stream = match output_config.sample_format() {
            SampleFormat::F32 => {
                playback_stream::<f32>(&output, output_config, rx, start, &counters)
            }
            SampleFormat::I16 => {
                playback_stream::<i16>(&output, output_config, rx, start, &counters)
            }
            format => return Err(anyhow!("Unsupported output format: {format}")),
        }?;
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

fn playback_stream<T: SizedSample + FromSample<f32>>(
    output: &Device,
    config: SupportedStreamConfig,
    rx: Consumer<TimedFrame>,
    start: Instant,
    counters: &Arc<Counters>,
) -> Result<Stream> {
    let meter = counters.clone();
    let mut playback = Playback::new(rx, config.sample_rate(), config.channels());
    Ok(output.build_output_stream(
        &config.config(),
        move |out: &mut [T], _| {
            let now = start.elapsed().as_millis() as u64;
            meter.played_ms.store(now, Relaxed);
            playback.fill(out, now, &meter);
        },
        on_error(counters),
        None,
    )?)
}

fn config(device: &Device, input: bool) -> Result<SupportedStreamConfig> {
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    ranges
        .into_iter()
        .filter(|r| {
            r.sample_format() == SampleFormat::F32
                || (!input && r.sample_format() == SampleFormat::I16)
        })
        .filter_map(|r| {
            let rate = r.max_sample_rate().min(SAMPLE_RATE);
            let suitable = if input {
                rate >= 8_000 && r.channels() > 0
            } else {
                rate >= 8_000 && (1..=2).contains(&r.channels())
            };
            (suitable && rate >= r.min_sample_rate()).then(|| r.with_sample_rate(rate))
        })
        .max_by_key(|c| {
            (
                c.sample_rate(),
                c.channels() == 2,
                c.sample_format() == SampleFormat::F32,
            )
        })
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
    rate_ratio: f64,
    channels: usize,
}

impl Playback {
    fn new(rx: Consumer<TimedFrame>, rate: u32, channels: u16) -> Self {
        Self {
            rx,
            current: None,
            fraction: 0.,
            started: false,
            rate_ratio: f64::from(SAMPLE_RATE) / f64::from(rate),
            channels: usize::from(channels),
        }
    }

    fn fill<T: SizedSample + FromSample<f32>>(
        &mut self,
        out: &mut [T],
        now: u64,
        counts: &Counters,
    ) {
        const TARGET: usize = 2 * BLOCK;
        out.fill(T::EQUILIBRIUM);
        let frame_count = out.len() / self.channels;
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
                    counts.missing.fetch_add(frame_count as u64, Relaxed);
                }
                return;
            }
            self.current = self.rx.pop().ok();
            self.started = true;
        }
        let drift =
            ((buffered as f64 - TARGET as f64) / (TARGET as f64 * 800.)).clamp(-0.003, 0.003);
        let step = self.rate_ratio * (1. + drift);
        for (idx, frame) in out.chunks_exact_mut(self.channels).enumerate() {
            let (Some((left, _)), Ok((right, _))) = (self.current, self.rx.peek()) else {
                counts
                    .missing
                    .fetch_add((frame_count - idx) as u64, Relaxed);
                self.current = None;
                self.fraction = 0.;
                break;
            };
            let sample = |c: usize| left[c] + (right[c] - left[c]) * self.fraction as f32;
            if frame.len() == 1 {
                frame[0] = T::from_sample((sample(0) + sample(1)) * 0.5);
            } else {
                frame[0] = T::from_sample(sample(0));
                frame[1] = T::from_sample(sample(1));
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
    fn renders_48_khz_stereo_processing_to_a_44_1_khz_mono_pcm_cable() {
        let (mut tx, rx) = RingBuffer::new(3 * BLOCK);
        for _ in 0..2 * BLOCK {
            tx.push(([0.25, 0.75], 0)).unwrap();
        }
        let mut playback = Playback::new(rx, 44_100, 1);
        let mut out = [0_i16; 441];
        playback.fill(&mut out, 10, &Counters::default());
        assert_eq!(out, [16_384; 441]);
        assert!(playback.rx.slots().abs_diff(BLOCK) <= 1);
    }
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
        let mut playback = Playback::new(rx, SAMPLE_RATE, 2);
        let counts = Counters::default();
        let mut out = [1.0_f32; BLOCK * 2];
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
