//! WASAPI streams with a bounded single-producer, single-consumer queue.
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    Device, DeviceId, FromSample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use orvia_core::{BLOCK, BlockLevels, Mixing, NoiseMethod, Processor, SAMPLE_RATE, Settings};
use orvia_engine::{is_orvia, is_virtual_cable, label};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

type Frame = [f32; 2];
type TimedFrame = (Frame, u64);
type MonitorFeed = Arc<Mutex<Option<Producer<TimedFrame>>>>;

fn forward_monitor(feed: &MonitorFeed, block: &[Frame; BLOCK], stamp: u64) {
    // A disconnected or slow monitor must never hold up the virtual microphone.
    if let Ok(mut guard) = feed.try_lock()
        && let Some(dest) = guard.as_mut()
    {
        for &frame in block {
            if dest.push((frame, stamp)).is_err() {
                break;
            }
        }
    }
}

#[derive(Default)]
struct Monitor {
    feed: MonitorFeed,
    stream: Option<(Stream, Arc<Counters>)>,
    id: Option<String>,
    attempted: Option<Instant>,
    error: Option<String>,
}

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
    gain: Arc<AtomicU16>,
    processing: (Mixing, Option<NoiseMethod>),
    replacements: Producer<Processor>,
    retired: Consumer<Processor>,
    incoming_levels: Consumer<(u64, BlockLevels)>,
    levels: VecDeque<(u64, BlockLevels)>,
    monitor: Monitor,
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
        let input_config = config(&input, Port::Microphone)?.config();
        let output_config = config(&output, Port::Cable)?;
        let counters = Arc::new(Counters::default());
        let start = Instant::now();
        let (mut tx, rx) = RingBuffer::<TimedFrame>::new(BLOCK * 10);
        let (mut envelope_tx, incoming_levels) = RingBuffer::new(200);
        let gain = Arc::new(AtomicU16::new(settings.gain_percent));
        let live_gain = gain.clone();
        let monitor = Monitor::default();
        let feed = monitor.feed.clone();
        let mut processor = Processor::new(settings);
        let (replacements, mut next_processor) = RingBuffer::new(1);
        let (mut return_processor, retired) = RingBuffer::new(1);
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
                            // Allocate and retire filter state on the control thread.
                            if !return_processor.is_full()
                                && let Ok(next) = next_processor.pop()
                            {
                                let old = std::mem::replace(&mut processor, next);
                                let _ = return_processor.push(old);
                            }
                            processor.set_gain(live_gain.load(Relaxed));
                            let (peak, levels) = processor.process_with_levels(&mut block);
                            let _ = envelope_tx.push((stamp, levels));
                            meter.peak.fetch_max(peak.to_bits(), Relaxed);
                            forward_monitor(&feed, &block, stamp);
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
            gain,
            processing: (settings.mixing, settings.active_noise_method()),
            replacements,
            retired,
            incoming_levels,
            levels: VecDeque::with_capacity(200),
            monitor,
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

    pub(super) fn update_controls(&mut self, settings: &Settings) {
        self.gain.store(settings.gain_percent, Relaxed);
        let _ = self.retired.pop();
        let requested = (settings.mixing, settings.active_noise_method());
        if self.processing != requested
            && !self.replacements.is_full()
            && self.replacements.push(Processor::new(settings)).is_ok()
        {
            self.processing = requested;
        }
        let now = self.start.elapsed().as_millis() as u64;
        while let Ok(entry) = self.incoming_levels.pop() {
            if self.levels.len() == 200 {
                self.levels.pop_front();
            }
            self.levels.push_back(entry);
        }
        while self
            .levels
            .front()
            .is_some_and(|(t, _)| now.saturating_sub(*t) > 2000)
        {
            self.levels.pop_front();
        }
        self.update_monitor(settings);
    }

    pub(super) fn envelopes(&self) -> orvia_core::SignalHistory {
        orvia_core::SignalHistory::from_blocks(self.levels.iter().map(|(_, levels)| levels))
    }

    pub(super) fn update_monitor(&mut self, settings: &Settings) {
        let wanted = settings
            .monitor
            .as_ref()
            .filter(|_| settings.monitor_enabled);
        if self.monitor.id.as_ref() != wanted {
            self.monitor.detach();
            self.monitor.id = wanted.cloned();
            self.monitor.attempted = None;
            self.monitor.error = None;
        }
        let Some(id) = wanted else { return };
        if let Some((_, meter)) = &self.monitor.stream {
            let age = (self.start.elapsed().as_millis() as u64)
                .saturating_sub(meter.played_ms.load(Relaxed));
            if !meter.failed.load(Relaxed) && age < 2000 {
                return;
            }
            self.monitor.detach();
        }
        if self
            .monitor
            .attempted
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.monitor.attempted = Some(Instant::now());
        self.monitor.error = self
            .monitor
            .connect(id, self.start)
            .err()
            .map(|e| format!("{e:#}"));
    }

    pub(super) fn monitor_status(&self) -> (bool, Option<String>) {
        (self.monitor.stream.is_some(), self.monitor.error.clone())
    }

    pub(super) fn metrics(&self) -> (f32, u64, u64) {
        (
            f32::from_bits(self.counters.peak.swap(0, Relaxed)),
            self.counters.missing.load(Relaxed),
            self.counters.dropped.load(Relaxed),
        )
    }
}

impl Monitor {
    fn detach(&mut self) {
        if let Ok(mut feed) = self.feed.lock() {
            *feed = None;
        }
        self.stream = None;
    }

    fn connect(&mut self, id: &str, start: Instant) -> Result<()> {
        let device = cpal::default_host()
            .device_by_id(&id.parse()?)
            .context("Headphones disconnected")?;
        ensure!(
            device.description()?.supports_output() && !is_virtual_cable(&device),
            "Choose a physical playback device"
        );
        let format = config(&device, Port::Headphones)?;
        let (tx, rx) = RingBuffer::new(BLOCK * 10);
        let meter = Arc::new(Counters::default());
        meter
            .played_ms
            .store(start.elapsed().as_millis() as u64, Relaxed);
        let stream = match format.sample_format() {
            SampleFormat::F32 => playback_stream::<f32>(&device, format, rx, start, &meter),
            SampleFormat::I16 => playback_stream::<i16>(&device, format, rx, start, &meter),
            sample => return Err(anyhow!("Unsupported monitor format: {sample}")),
        }?;
        stream.play()?;
        *self
            .feed
            .lock()
            .map_err(|_| anyhow!("Monitor queue unavailable"))? = Some(tx);
        self.stream = Some((stream, meter));
        Ok(())
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

#[derive(Clone, Copy, PartialEq)]
enum Port {
    Microphone,
    Cable,
    Headphones,
}

fn config(device: &Device, port: Port) -> Result<SupportedStreamConfig> {
    let input = port == Port::Microphone;
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
            let rate = if input {
                r.max_sample_rate().min(SAMPLE_RATE)
            } else {
                SAMPLE_RATE.clamp(r.min_sample_rate(), r.max_sample_rate())
            };
            let suitable =
                rate >= 8_000 && r.channels() > 0 && (port != Port::Cable || r.channels() <= 2);
            (suitable && rate >= r.min_sample_rate()).then(|| r.with_sample_rate(rate))
        })
        .max_by_key(|c| {
            (
                std::cmp::Reverse(c.sample_rate().abs_diff(SAMPLE_RATE)),
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
    fn headphones_receive_the_processed_block_without_backpressure_on_the_source() {
        let (tx, mut rx) = RingBuffer::new(BLOCK);
        let feed = Arc::new(Mutex::new(Some(tx)));
        let mut frames = [[0.25, -0.25]; BLOCK];
        Processor::new(&Settings {
            gain_percent: 200,
            ..Default::default()
        })
        .process(&mut frames);
        forward_monitor(&feed, &frames, 10);
        forward_monitor(&feed, &frames, 20);
        for _ in 0..BLOCK {
            assert_eq!(rx.pop().unwrap(), ([0.5, -0.5], 10));
        }
        let changing_device = feed.lock().unwrap();
        forward_monitor(&feed, &frames, 30);
        assert!(rx.pop().is_err());
        drop(changing_device);
        Processor::new(&Settings {
            gain_percent: 0,
            ..Default::default()
        })
        .process(&mut frames);
        forward_monitor(&feed, &frames, 40);
        assert_eq!(rx.pop().unwrap(), ([0.0; 2], 40));
    }
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
