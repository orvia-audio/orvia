//! Desktop audio streams with a bounded single-producer, single-consumer queue.
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    Device, DeviceId, FromSample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use orvia_core::{BLOCK, BlockLevels, Mixing, NoiseMethod, Processor, SAMPLE_RATE, Settings};
use orvia_engine::{
    formats::{InputRate, Port, config, stream_config, stream_continues},
    host, is_virtual_cable, label,
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

type Frame = [f32; 2];
type TimedFrame = (Frame, u64);
struct MonitorStreams {
    _source: Stream,
    _headphones: Stream,
    counters: Arc<Counters>,
}

#[derive(Default)]
struct Monitor {
    stream: Option<MonitorStreams>,
    id: Option<String>,
    receiver: Option<String>,
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

// Publish a complete block's controls with one lock-free load on the audio thread.
fn controls(settings: &Settings) -> u64 {
    let tone = settings.tone();
    u64::from(tone.low_cut_hz)
        | u64::from(tone.gain_percent) << 16
        | u64::from(tone.compressor_ratio) << 32
        | u64::from(settings.deep_filter_limit_db) << 48
}

pub(super) struct Route {
    controls: Arc<AtomicU64>,
    applied: Arc<AtomicU64>,
    processing: (Mixing, Option<NoiseMethod>),
    replacements: Producer<Processor>,
    retired: Consumer<Processor>,
    incoming_levels: Consumer<(u64, BlockLevels)>,
    levels: VecDeque<(u64, BlockLevels)>,
    monitor: Monitor,
    output: Device,
    _streams: (Stream, Stream),
    pub(super) input_id: DeviceId,
    pub(super) name: String,
    counters: Arc<Counters>,
    start: Instant,
}

pub(super) fn default_input_id() -> Option<DeviceId> {
    host().ok()?.default_input_device()?.id().ok()
}

impl Route {
    pub(super) fn open(settings: &Settings) -> Result<Self> {
        let host = host()?;
        let output = orvia_engine::mic_test::virtual_output(settings.output.as_deref())?;
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
        let input_config = stream_config(&config(&input, Port::Microphone)?);
        let output_config = config(&output, Port::Cable)?;
        let counters = Arc::new(Counters::default());
        let start = Instant::now();
        let (mut tx, rx) = RingBuffer::<TimedFrame>::new(BLOCK * 10);
        let (mut envelope_tx, incoming_levels) = RingBuffer::new(200);
        let controls = Arc::new(AtomicU64::new(controls(settings)));
        let live = controls.clone();
        let applied = Arc::new(AtomicU64::new(u64::MAX));
        let acknowledged = applied.clone();
        let monitor = Monitor::default();
        let mut processor = Processor::new(settings);
        let (replacements, mut next_processor) = RingBuffer::new(1);
        let (mut return_processor, retired) = RingBuffer::new(1);
        let mut block = [[0.; 2]; BLOCK];
        let mut used = 0;
        let meter = counters.clone();
        let channels = usize::from(input_config.channels);
        let mut resampler = InputRate::new(input_config.sample_rate);
        let input_stream = input.build_input_stream(
            input_config,
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
                            let word = live.load(Relaxed);
                            processor.set_tone(orvia_core::Tone {
                                low_cut_hz: word as u16,
                                gain_percent: (word >> 16) as u16,
                                compressor_ratio: (word >> 32) as u16,
                            });
                            processor.set_deep_filter_limit((word >> 48) as u16);
                            let (peak, levels) = processor.process_with_levels(&mut block);
                            acknowledged.store(word, Relaxed);
                            let _ = envelope_tx.push((stamp, levels));
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
            controls,
            applied,
            processing: (settings.mixing, settings.active_noise_method()),
            replacements,
            retired,
            incoming_levels,
            levels: VecDeque::with_capacity(200),
            monitor,
            output,
            _streams: (input_stream, output_stream),
            counters,
            input_id,
            name,
            start,
        })
    }

    pub(super) fn controls_applied(&self, settings: &Settings) -> bool {
        self.applied.load(Relaxed) == controls(settings)
    }

    pub(super) fn healthy(&self) -> bool {
        let now = self.start.elapsed().as_millis() as u64;
        !self.counters.failed.load(Relaxed)
            && now.saturating_sub(self.counters.captured_ms.load(Relaxed)) < 2000
            && now.saturating_sub(self.counters.played_ms.load(Relaxed)) < 2000
    }

    pub(super) fn update_controls(&mut self, settings: &Settings) {
        self.controls.store(controls(settings), Relaxed);
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
        if self.monitor.id.as_ref() != wanted || self.monitor.receiver != settings.receiver {
            self.monitor.stream = None;
            self.monitor.id = wanted.cloned();
            self.monitor.receiver = settings.receiver.clone();
            self.monitor.attempted = None;
            self.monitor.error = None;
        }
        let Some(id) = wanted else { return };
        if let Some(streams) = &self.monitor.stream {
            let meter = &streams.counters;
            let age = (self.start.elapsed().as_millis() as u64)
                .saturating_sub(meter.played_ms.load(Relaxed));
            let input_age = (self.start.elapsed().as_millis() as u64)
                .saturating_sub(meter.captured_ms.load(Relaxed));
            if !meter.failed.load(Relaxed) && age < 2000 && input_age < 2000 {
                return;
            }
            self.monitor.stream = None;
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
            .connect(id, &self.output, self.start)
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
    fn connect(&mut self, id: &str, output: &Device, start: Instant) -> Result<()> {
        let receiver = orvia_engine::mic_test::receiver(output, self.receiver.as_deref())?;
        let capture_format = config(&receiver, Port::Recording)?;
        let device = host()?
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
        meter
            .captured_ms
            .store(start.elapsed().as_millis() as u64, Relaxed);
        let source = match capture_format.sample_format() {
            SampleFormat::F32 => {
                monitor_capture::<f32>(&receiver, &capture_format, tx, start, &meter)
            }
            SampleFormat::I16 => {
                monitor_capture::<i16>(&receiver, &capture_format, tx, start, &meter)
            }
            sample => return Err(anyhow!("Unsupported recording format: {sample}")),
        }?;
        let stream = match format.sample_format() {
            SampleFormat::F32 => playback_stream::<f32>(&device, format, rx, start, &meter),
            SampleFormat::I16 => playback_stream::<i16>(&device, format, rx, start, &meter),
            sample => return Err(anyhow!("Unsupported monitor format: {sample}")),
        }?;
        source.play()?;
        stream.play()?;
        self.stream = Some(MonitorStreams {
            _source: source,
            _headphones: stream,
            counters: meter,
        });
        Ok(())
    }
}

/// Forward samples delivered by the receiving side of the cable to the headphone queue.
fn monitor_capture<T: SizedSample>(
    device: &Device,
    format: &SupportedStreamConfig,
    mut tx: Producer<TimedFrame>,
    start: Instant,
    counters: &Arc<Counters>,
) -> Result<Stream, cpal::Error>
where
    f32: FromSample<T>,
{
    let channels = usize::from(format.channels());
    let mut rate = InputRate::new(format.sample_rate());
    let meter = counters.clone();
    device.build_input_stream(
        stream_config(format),
        move |data: &[T], _| {
            let stamp = start.elapsed().as_millis() as u64;
            meter.captured_ms.store(stamp, Relaxed);
            for f in data.chunks_exact(channels) {
                let left = f[0].to_sample();
                let right = f.get(1).map_or(left, |s| s.to_sample());
                rate.push([left, right], |frame| {
                    let _ = tx.push((frame, stamp));
                });
            }
        },
        on_error(counters),
        None,
    )
}

fn on_error(counters: &Arc<Counters>) -> impl FnMut(cpal::Error) + Send + 'static {
    let counters = counters.clone();
    move |error| {
        if !stream_continues(&error) {
            counters.failed.store(true, Relaxed);
        }
    }
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
        stream_config(&config),
        move |out: &mut [T], _| {
            let now = start.elapsed().as_millis() as u64;
            meter.played_ms.store(now, Relaxed);
            playback.fill(out, now, &meter);
        },
        on_error(counters),
        None,
    )?)
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
        out.fill(T::EQUILIBRIUM);
        let frame_count = out.len() / self.channels;
        // Keep three capture blocks in reserve after a complete output callback,
        // plus the right-hand sample needed by the interpolator.
        let requested = (frame_count as f64 * self.rate_ratio).ceil() as usize;
        let target = (requested + 3 * BLOCK + 1).min(self.rx.buffer().capacity());
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
            if buffered < target {
                if self.started {
                    counts.missing.fetch_add(frame_count as u64, Relaxed);
                }
                return;
            }
            self.current = self.rx.pop().ok();
            self.started = true;
        }
        let drift =
            ((buffered as f64 - target as f64) / (target as f64 * 800.)).clamp(-0.003, 0.003);
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
    fn recoverable_audio_notifications_keep_the_stream_open() {
        let counters = Arc::new(Counters::default());
        let mut callback = on_error(&counters);
        callback(cpal::ErrorKind::Xrun.into());
        assert!(!counters.failed.load(Relaxed));
        callback(cpal::ErrorKind::RealtimeDenied.into());
        assert!(!counters.failed.load(Relaxed));
        #[cfg(windows)]
        {
            callback(cpal::Error::with_message(
                cpal::ErrorKind::BackendError,
                "Failed to promote audio thread to real-time priority: Access is denied.",
            ));
            assert!(!counters.failed.load(Relaxed));
        }
        callback(cpal::ErrorKind::DeviceNotAvailable.into());
        assert!(counters.failed.load(Relaxed));
    }
    #[test]
    fn renders_48_khz_stereo_processing_to_a_44_1_khz_mono_pcm_cable() {
        let (mut tx, rx) = RingBuffer::new(3 * BLOCK);
        for _ in 0..3 * BLOCK {
            tx.push(([0.25, 0.75], 0)).unwrap();
        }
        let mut playback = Playback::new(rx, 44_100, 1);
        let mut out = [0_i16; 441];
        playback.fill(&mut out, 10, &Counters::default());
        assert_eq!(out, [16_384; 441]);
        assert!(playback.rx.slots().abs_diff(2 * BLOCK) <= 1);
    }
    #[test]
    fn playback_preserves_speech_and_pauses_across_callback_sizes_and_late_input() {
        for period in [10, 20, 30] {
            let (mut tx, rx) = RingBuffer::new(10 * BLOCK);
            let mut playback = Playback::new(rx, SAMPLE_RATE, 2);
            let counts = Counters::default();
            let mut out = vec![0.0_f32; period * 48 * 2];
            let mut pending = Vec::new();
            for time in (10..=6_000).step_by(10) {
                let sample = if (1_000..1_200).contains(&time) {
                    0.
                } else {
                    0.25
                };
                pending.extend(std::iter::repeat_n(([sample; 2], time as u64), BLOCK));
                if !(3_000..3_030).contains(&time) {
                    for frame in pending.drain(..) {
                        tx.push(frame).unwrap();
                    }
                }
                if time % period == 0 {
                    playback.fill(&mut out, time as u64, &counts);
                    if (1_100..1_200).contains(&time) {
                        assert!(out.iter().all(|&s| s == 0.));
                    } else if time > 1_350 {
                        assert!(out.iter().all(|&s| s == 0.25));
                    }
                }
            }
            assert_eq!(counts.missing.load(Relaxed), 0);
            assert_eq!(counts.dropped.load(Relaxed), 0);
        }
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
        for _ in 0..3 * BLOCK {
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
        for _ in 0..3 * BLOCK {
            tx.push(([-0.5; 2], 130)).unwrap();
        }
        playback.fill(&mut out, 135, &counts);
        assert!(out.iter().all(|s| *s == -0.5));
    }
}
