//! Desktop audio streams with a bounded single-producer, single-consumer queue.
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    Device, DeviceId, FromSample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use orvia_core::{
    BLOCK, BlockLevels, Mixing, NoiseMethod, Processor, SAMPLE_RATE, Settings, SignalWindow,
};
use orvia_engine::{
    described_virtual_cable, display_name,
    formats::{InputRate, Port, config, stream_config, stream_continues},
    host,
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

type Frame = [f32; 2];
type TimedFrame = (Frame, u64);
type Levels = (u64, BlockLevels);

/// Transfers the metering producer at block boundaries. The control thread owns
/// the consumer until the producer has returned, so allocation and destruction
/// of the history queue both occur outside the audio callback.
struct MeterTap {
    offered: Consumer<Producer<Levels>>,
    returned: Producer<Producer<Levels>>,
    active: Option<Producer<Levels>>,
}

struct MeterReader {
    offer: Producer<Producer<Levels>>,
    returns: Consumer<Producer<Levels>>,
    levels: Option<Consumer<Levels>>,
}

impl MeterTap {
    fn queue(&mut self, enabled: bool) -> Option<&mut Producer<Levels>> {
        if self.active.is_none() {
            self.active = self.offered.pop().ok();
        }
        if !enabled && let Some(queue) = self.active.take() {
            // The control thread offers at most one producer at a time.
            if let Err(rtrb::PushError::Full(queue)) = self.returned.push(queue) {
                self.active = Some(queue);
            }
            return None;
        }
        self.active.as_mut()
    }
}

impl MeterReader {
    fn new() -> (Self, MeterTap) {
        let (offer, offered) = RingBuffer::new(1);
        let (returned, returns) = RingBuffer::new(1);
        (
            Self {
                offer,
                returns,
                levels: None,
            },
            MeterTap {
                offered,
                returned,
                active: None,
            },
        )
    }

    fn update(&mut self, enabled: bool, now: u64, history: &mut SignalWindow) {
        let returned = self.returns.pop().ok();
        history.update(now, std::iter::from_fn(|| self.levels.as_mut()?.pop().ok()));
        if let Some(producer) = returned {
            if enabled {
                self.offer
                    .push(producer)
                    .expect("the producer just returned");
            } else {
                self.levels = None;
            }
        } else if enabled && self.levels.is_none() {
            let (write, read) = RingBuffer::new(200);
            self.offer.push(write).expect("the first producer fits");
            self.levels = Some(read);
        }
    }
}
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
    metering: AtomicBool,
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
    host: cpal::Host,
    input_id: DeviceId,
    input_channels: u16,
    controls: Arc<AtomicU64>,
    applied: Arc<AtomicU64>,
    processing: (Mixing, Option<NoiseMethod>),
    replacements: Producer<Processor>,
    retired: Consumer<Processor>,
    incoming_levels: MeterReader,
    pub(super) spectrum: crate::spectrum::Observation,
    levels: SignalWindow,
    monitor: Monitor,
    output: Device,
    _streams: (Stream, Stream),
    pub(super) name: String,
    counters: Arc<Counters>,
    start: Instant,
}

impl Route {
    pub(super) fn open(settings: &Settings) -> Result<Self> {
        let host = host()?;
        let (output, output_desc) =
            orvia_engine::mic_test::described_output_on(&host, settings.output.as_deref())?;
        let input = match &settings.input {
            Some(id) => host
                .device_by_id(&id.parse()?)
                .context("Selected microphone is disconnected")?,
            None => host
                .default_input_device()
                .context("No default microphone")?,
        };
        let input_desc = input.description()?;
        ensure!(
            input_desc.supports_input() && !described_virtual_cable(&input, &input_desc),
            "Select a physical microphone to avoid feedback"
        );
        let input_id = input.id()?;
        let name = format!(
            "{} → {}",
            display_name(&input_desc),
            display_name(&output_desc)
        );
        let input_config = stream_config(&config(&input, Port::Microphone)?);
        let output_config = config(&output, Port::Cable)?;
        let counters = Arc::new(Counters::default());
        let start = Instant::now();
        let (mut tx, rx) = RingBuffer::<TimedFrame>::new(BLOCK * 10);
        let (incoming_levels, mut envelope) = MeterReader::new();
        let (mut spectrum_tap, spectrum) = crate::spectrum::Observation::new();
        let controls = Arc::new(AtomicU64::new(controls(settings)));
        let live = controls.clone();
        let applied = Arc::new(AtomicU64::new(u64::MAX));
        let acknowledged = applied.clone();
        let monitor = Monitor::default();
        let input_channels = input_config.channels;
        let mut processor = Processor::for_input(settings, input_channels);
        let mut current_controls = None;
        let (replacements, mut next_processor) = RingBuffer::new(1);
        let (mut return_processor, retired) = RingBuffer::new(1);
        let mut input_blocks = CaptureBlocks::new(input_config.sample_rate, input_config.channels);
        let meter = counters.clone();
        let input_stream = input.build_input_stream(
            input_config,
            move |samples: &[f32], _| {
                let stamp = start.elapsed().as_millis() as u64;
                meter.captured_ms.store(stamp, Relaxed);
                input_blocks.push(samples, |block| {
                    // Allocate and retire filter state on the control thread.
                    if !return_processor.is_full()
                        && let Ok(next) = next_processor.pop()
                    {
                        let old = std::mem::replace(&mut processor, next);
                        let _ = return_processor.push(old);
                        current_controls = None;
                    }
                    let word = live.load(Relaxed);
                    if current_controls != Some(word) {
                        processor.set_tone(orvia_core::Tone {
                            low_cut_hz: word as u16,
                            gain_percent: (word >> 16) as u16,
                            compressor_ratio: (word >> 32) as u16,
                        });
                        processor.set_deep_filter_limit((word >> 48) as u16);
                        current_controls = Some(word);
                    }
                    if let Some(envelope_tx) = envelope.queue(meter.metering.load(Relaxed)) {
                        let (peak, levels) = processor.process_with_levels(block);
                        let _ = envelope_tx.push((stamp, levels));
                        meter.peak.fetch_max(peak.to_bits(), Relaxed);
                    } else {
                        processor.process_unmetered(block);
                    }
                    spectrum_tap.capture(block);
                    acknowledged.store(word, Relaxed);
                    let count = tx.slots().min(BLOCK);
                    if let Ok(chunk) = tx.write_chunk_uninit(count) {
                        chunk.fill_from_iter(block.iter().map(|&frame| (frame, stamp)));
                    }
                    if count < BLOCK {
                        meter.dropped.fetch_add((BLOCK - count) as u64, Relaxed);
                    }
                });
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
            host,
            input_id,
            input_channels,
            controls,
            applied,
            processing: (settings.mixing, settings.active_noise_method()),
            replacements,
            retired,
            incoming_levels,
            spectrum,
            levels: SignalWindow::default(),
            monitor,
            output,
            _streams: (input_stream, output_stream),
            counters,
            name,
            start,
        })
    }

    /// Query the live host so a default-device check can reuse its sound server connection.
    pub(super) fn default_input_changed(&self) -> bool {
        self.host
            .default_input_device()
            .and_then(|device| device.id().ok())
            .as_ref()
            != Some(&self.input_id)
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

    pub(super) fn update_controls(&mut self, settings: &Settings, observed: bool) {
        self.controls.store(controls(settings), Relaxed);
        let _ = self.retired.pop();
        let requested = (settings.mixing, settings.active_noise_method());
        if (self.processing.1 != requested.1
            || (self.input_channels > 1 && self.processing.0 != requested.0))
            && !self.replacements.is_full()
            && self
                .replacements
                .push(Processor::for_input(settings, self.input_channels))
                .is_ok()
        {
            self.processing = requested;
        }
        let now = self.start.elapsed().as_millis() as u64;
        self.incoming_levels.update(observed, now, &mut self.levels);
        self.counters.metering.store(observed, Relaxed);
        self.update_monitor(settings);
    }

    pub(super) fn envelopes(&self) -> orvia_core::SignalHistory {
        self.levels.history()
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
            .connect(&self.host, id, &self.output, self.start)
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

/// Assemble 48 kHz stereo blocks across arbitrary device callback boundaries.
struct CaptureBlocks {
    buffer: [Frame; BLOCK],
    filled: usize,
    channels: usize,
    resampler: Option<InputRate>,
}

impl CaptureBlocks {
    fn new(rate: u32, channels: u16) -> Self {
        Self {
            buffer: [[0.; 2]; BLOCK],
            filled: 0,
            channels: usize::from(channels),
            resampler: (rate != SAMPLE_RATE).then(|| InputRate::new(rate)),
        }
    }

    fn push(&mut self, mut samples: &[f32], mut emit: impl FnMut(&mut [Frame; BLOCK])) {
        let channels = self.channels;
        if let Some(rate) = &mut self.resampler {
            let frames = samples
                .chunks_exact(channels)
                .map(|frame| [frame[0], *frame.get(1).unwrap_or(&frame[0])]);
            rate.push_batch(frames, |frame| {
                self.buffer[self.filled] = frame;
                self.filled += 1;
                if self.filled == BLOCK {
                    emit(&mut self.buffer);
                    self.filled = 0;
                }
            });
            return;
        }
        while samples.len() >= channels {
            let count = (samples.len() / channels).min(BLOCK - self.filled);
            let (batch, rest) = samples.split_at(count * channels);
            let output = &mut self.buffer[self.filled..][..count];
            match channels {
                1 => output.iter_mut().zip(batch).for_each(|(f, &x)| *f = [x; 2]),
                2 => output.copy_from_slice(batch.as_chunks::<2>().0),
                _ => {
                    for (slot, frame) in output.iter_mut().zip(batch.chunks_exact(channels)) {
                        *slot = [frame[0], frame[1]];
                    }
                }
            }
            self.filled += count;
            if self.filled == BLOCK {
                emit(&mut self.buffer);
                self.filled = 0;
            }
            samples = rest;
        }
    }
}

impl Monitor {
    fn connect(
        &mut self,
        host: &cpal::Host,
        id: &str,
        output: &Device,
        start: Instant,
    ) -> Result<()> {
        let receiver = orvia_engine::mic_test::receiver_on(host, output, self.receiver.as_deref())?;
        let capture_format = config(&receiver, Port::Recording)?;
        let device = host
            .device_by_id(&id.parse()?)
            .context("Headphones disconnected")?;
        let desc = device.description()?;
        ensure!(
            desc.supports_output() && !described_virtual_cable(&device, &desc),
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
    let native_rate = format.sample_rate() == SAMPLE_RATE;
    let mut rate = InputRate::new(format.sample_rate());
    let meter = counters.clone();
    device.build_input_stream(
        stream_config(format),
        move |data: &[T], _| {
            let stamp = start.elapsed().as_millis() as u64;
            meter.captured_ms.store(stamp, Relaxed);
            if native_rate {
                let frames = data.chunks_exact(channels);
                let count = tx.slots().min(frames.len());
                if let Ok(chunk) = tx.write_chunk_uninit(count) {
                    chunk.fill_from_iter(frames.map(|f| {
                        let left = f[0].to_sample();
                        ([left, f.get(1).map_or(left, |s| s.to_sample())], stamp)
                    }));
                }
                return;
            }
            let frames = data.chunks_exact(channels).map(|f| {
                let left = f[0].to_sample();
                let right = f.get(1).map_or(left, |s| s.to_sample());
                [left, right]
            });
            rate.push_batch(frames, |frame| {
                let _ = tx.push((frame, stamp));
            });
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
        let stale = |&(_, stamp): &TimedFrame| now.saturating_sub(stamp) > 100;
        let mut dropped = 0;
        if self.current.as_ref().is_some_and(stale) {
            self.current = None;
            self.fraction = 0.;
            dropped += 1;
        }
        // A stalled callback can leave several thousand expired frames. Release the
        // stale prefix in one queue update so the producer can enqueue fresh audio.
        while self.rx.peek().is_ok_and(stale) {
            let available = self.rx.slots();
            let chunk = self.rx.read_chunk(available).expect("single reader");
            let (first, second) = chunk.as_slices();
            let expired = first
                .iter()
                .chain(second)
                .take_while(|frame| stale(frame))
                .count();
            chunk.commit(expired);
            dropped += expired as u64;
        }
        if dropped > 0 {
            counts.dropped.fetch_add(dropped, Relaxed);
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
        let mut output = out.chunks_exact_mut(self.channels);
        let mut current = self.current.expect("primed playback");
        while output.len() != 0 || self.fraction >= 1. {
            let available = self.rx.slots();
            if available == 0 {
                counts.missing.fetch_add(output.len() as u64, Relaxed);
                self.current = None;
                self.fraction = 0.;
                return;
            }
            // Publish one read position per batch, including when the ring wraps.
            let chunk = self.rx.read_chunk(available).expect("single reader");
            let (first, second) = chunk.as_slices();
            let mut consumed = 0;
            for mut input in [first, second] {
                while let Some(&next) = input.first() {
                    if self.fraction >= 1. {
                        current = next;
                        input = &input[1..];
                        consumed += 1;
                        self.fraction -= 1.;
                        continue;
                    }
                    let Some(frame) = output.next() else { break };
                    let sample =
                        |c: usize| current.0[c] + (next.0[c] - current.0[c]) * self.fraction as f32;
                    if frame.len() == 1 {
                        frame[0] = T::from_sample((sample(0) + sample(1)) * 0.5);
                    } else {
                        frame[0] = T::from_sample(sample(0));
                        frame[1] = T::from_sample(sample(1));
                    }
                    self.fraction += step;
                }
            }
            chunk.commit(consumed);
        }
        self.current = Some(current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_queue_survives_quick_reopens_and_retires_off_the_callback() {
        let (mut reader, mut tap) = MeterReader::new();
        let mut history = SignalWindow::default();
        assert!(reader.levels.is_none());
        reader.update(true, 0, &mut history);
        // The window can close before the next audio block takes the producer.
        reader.update(false, 1, &mut history);
        assert!(tap.queue(false).is_none());
        reader.update(true, 2, &mut history);
        tap.queue(true)
            .unwrap()
            .push((3, BlockLevels::default()))
            .unwrap();
        assert!(tap.queue(false).is_none());
        reader.update(false, 4, &mut history);
        assert!(reader.levels.is_none());
        assert_eq!(history.history().peaks.len(), 1);
        reader.update(true, 5, &mut history);
        assert!(tap.queue(true).is_some());
    }

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
    fn capture_blocks_match_framewise_conversion_across_device_periods() {
        for rate in [16_000, 44_100, 48_000] {
            for channels in [1, 2, 4] {
                let mut blocks = CaptureBlocks::new(rate, channels as u16);
                let mut reference = InputRate::new(rate);
                let mut actual = Vec::new();
                let mut expected = Vec::new();
                for n in [127, 360, 1, 1024, 73, 1951] {
                    let samples: Vec<_> =
                        (0..n * channels).map(|i| (i as f32 * 0.13).sin()).collect();
                    blocks.push(&samples, |chunk| actual.extend_from_slice(chunk));
                    for f in samples.chunks_exact(channels) {
                        reference.push([f[0], *f.get(1).unwrap_or(&f[0])], |f| expected.push(f));
                    }
                }
                assert_eq!(actual, expected[..expected.len() / BLOCK * BLOCK]);
                assert_eq!(blocks.buffer[..blocks.filled], expected[actual.len()..]);
            }
        }
    }

    #[test]
    fn stale_audio_is_discarded_across_ring_wraps_but_the_age_boundary_is_kept() {
        let (mut tx, mut rx) = RingBuffer::new(10 * BLOCK);
        for _ in 0..9 * BLOCK {
            tx.push(([0.; 2], 0)).unwrap();
        }
        rx.read_chunk(9 * BLOCK).unwrap().commit_all();
        for _ in 0..2 * BLOCK {
            tx.push(([-0.5; 2], 0)).unwrap();
        }
        for _ in 0..5 * BLOCK {
            tx.push(([0.25; 2], 1)).unwrap();
        }
        let mut playback = Playback::new(rx, SAMPLE_RATE, 2);
        playback.current = Some(([-0.75; 2], 0));
        let counts = Counters::default();
        let mut output = [0.; 2 * BLOCK];
        playback.fill(&mut output, 101, &counts);
        assert_eq!(output, [0.25; 2 * BLOCK]);
        assert_eq!(counts.dropped.load(Relaxed), (2 * BLOCK + 1) as u64);
        assert_eq!(counts.missing.load(Relaxed), 0);
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
