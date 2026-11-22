//! On-demand FFT of contiguous stereo output, computed on the engine control thread.

use orvia_core::{SAMPLE_RATE, spectrum::Spectrum};
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex32};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    f32::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

const WINDOW: usize = 8192;
const BIN_HZ: f32 = SAMPLE_RATE as f32 / WINDOW as f32;
type Frame = [f32; 2];

/// One bounded copy per audio block while a measurement is in progress.
pub(super) struct Tap {
    samples: Producer<Frame>,
    request: Arc<AtomicBool>,
    remaining: usize,
}

impl Tap {
    pub(super) fn capture(&mut self, block: &[Frame]) {
        if self.remaining == 0 {
            if !self.request.load(Relaxed) {
                return;
            }
            self.request.store(false, Relaxed);
            self.remaining = WINDOW;
        }
        let count = block.len().min(self.remaining);
        // The reader only requests another window after consuming all WINDOW frames.
        let (copied, _) = self.samples.push_partial_slice(&block[..count]);
        self.remaining -= copied.len();
    }
}

pub(super) struct Observation {
    samples: Consumer<Frame>,
    request: Arc<AtomicBool>,
    requested_at: Option<Instant>,
    frames: Vec<Frame>,
    analyzer: Option<Analyzer>,
    latest: Option<(Instant, Spectrum)>,
}

impl Observation {
    pub(super) fn new() -> (Tap, Self) {
        let (writer, samples) = RingBuffer::new(WINDOW);
        let request = Arc::new(AtomicBool::new(false));
        (
            Tap {
                samples: writer,
                request: request.clone(),
                remaining: 0,
            },
            Self {
                samples,
                request,
                requested_at: None,
                frames: vec![[0.; 2]; WINDOW],
                analyzer: None,
                latest: None,
            },
        )
    }

    /// Discard stale windows after a stall or after the user leaves the spectrum.
    pub(super) fn poll(&mut self, visible: bool) -> Option<Spectrum> {
        const FRESH: Duration = Duration::from_millis(650);
        if let Some(start) = self.requested_at
            && self.samples.pop_entire_slice(&mut self.frames).is_ok()
        {
            self.requested_at = None;
            if visible && start.elapsed() <= FRESH {
                self.latest = self
                    .analyzer
                    .get_or_insert_with(Analyzer::new)
                    .measure(&self.frames)
                    .map(|s| (start, s));
            }
        }
        if !visible {
            self.latest = None;
            return None;
        }
        if self.requested_at.is_none() {
            self.requested_at = Some(Instant::now());
            self.request.store(true, Relaxed);
        }
        self.latest
            .as_ref()
            .filter(|(t, _)| t.elapsed() <= FRESH)
            .map(|(_, s)| s.clone())
    }
}

struct Analyzer {
    plan: Arc<dyn RealToComplex<f32>>,
    hann: Vec<f32>,
    input: Vec<f32>,
    bins: Vec<Complex32>,
    scratch: Vec<Complex32>,
    power: Vec<f32>,
}

impl Analyzer {
    fn new() -> Self {
        let plan = RealFftPlanner::new().plan_fft_forward(WINDOW);
        Self {
            hann: (0..WINDOW)
                .map(|i| 0.5 - 0.5 * (TAU * i as f32 / WINDOW as f32).cos())
                .collect(),
            input: plan.make_input_vec(),
            bins: plan.make_output_vec(),
            scratch: plan.make_scratch_vec(),
            power: vec![0.; WINDOW / 2 + 1],
            plan,
        }
    }

    fn measure(&mut self, frames: &[Frame]) -> Option<Spectrum> {
        self.power.fill(0.);
        for channel in 0..2 {
            for ((sample, frame), weight) in self.input.iter_mut().zip(frames).zip(&self.hann) {
                *sample = frame[channel] * weight;
            }
            self.plan
                .process_with_scratch(&mut self.input, &mut self.bins, &mut self.scratch)
                .ok()?;
            // A full-scale, bin-centered sine has magnitude WINDOW / 4 after the Hann window.
            let scale = (4. / WINDOW as f32).powi(2);
            for (power, bin) in self.power.iter_mut().zip(&self.bins) {
                *power = power.max(bin.norm_sqr() * scale);
            }
        }
        let mut bands = Vec::with_capacity(Spectrum::BANDS);
        for band in 0..Spectrum::BANDS {
            let bin_at = |i| Spectrum::frequency_at(i / Spectrum::BANDS as f32) / BIN_HZ;
            let mid = bin_at(band as f32 + 0.5);
            let k = mid as usize;
            // Low-frequency bands can be narrower than an FFT bin.
            let center = self.power[k] + mid.fract() * (self.power[k + 1] - self.power[k]);
            let start = bin_at(band as f32).ceil() as usize;
            let end = bin_at((band + 1) as f32).ceil() as usize;
            let max = self.power[start..end]
                .iter()
                .copied()
                .fold(center, f32::max);
            let range = f32::from(Spectrum::RANGE_DB);
            bands.push((range + 10. * max.log10()).round().clamp(0., range) as u8);
        }
        let mut strongest = 10_f32.powf(-f32::from(Spectrum::RANGE_DB) / 10.);
        let mut peak_hz = None;
        for k in (Spectrum::MIN_HZ / BIN_HZ).floor() as usize
            ..=(Spectrum::MAX_HZ / BIN_HZ).ceil() as usize
        {
            let [left, center, right] = [self.power[k - 1], self.power[k], self.power[k + 1]];
            if center <= strongest || center <= left || center <= right {
                continue;
            }
            // Interpolate the peak in log power to resolve tones between FFT bins.
            let [a, b, c] = [left, center, right].map(|p| p.max(f32::MIN_POSITIVE).ln());
            let offset = if a + c < 2. * b {
                (0.5 * (a - c) / (a + c - 2. * b)).clamp(-0.5, 0.5)
            } else {
                0.
            };
            let hz = (k as f32 + offset) * BIN_HZ;
            if (Spectrum::MIN_HZ..=Spectrum::MAX_HZ).contains(&hz) {
                strongest = center;
                peak_hz = Some(hz.round() as u16);
            }
        }
        Some(Spectrum { bands, peak_hz })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orvia_core::BLOCK;

    #[test]
    fn preserves_tones_in_either_channel_including_opposite_polarity() {
        let mut analyzer = Analyzer::new();
        for (hz, left, right) in [
            (50., 0., 0.5),
            (60., 0.5, 0.),
            (440., 0.5, -0.5),
            (16_000., 0.5, 0.5),
        ] {
            let frames: Vec<_> = (0..WINDOW)
                .map(|i| {
                    let x = (TAU * hz * i as f32 / SAMPLE_RATE as f32).sin();
                    [left * x, right * x]
                })
                .collect();
            let s = analyzer.measure(&frames).unwrap();
            assert!(s.peak_hz.unwrap().abs_diff(hz as u16) <= 1);
            let band = ((hz / Spectrum::MIN_HZ).log10() / 3. * Spectrum::BANDS as f32) as usize;
            assert!((72..=74).contains(&s.bands[band]), "{hz} Hz: {:?}", s.bands);
        }
    }

    #[test]
    fn silence_and_dc_have_no_audible_peak() {
        let mut analyzer = Analyzer::new();
        for x in [0., 0.8] {
            let s = analyzer.measure(&vec![[x; 2]; WINDOW]).unwrap();
            assert!(s.peak_hz.is_none() && s.bands.iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn idle_tap_and_partial_windows_do_not_publish_a_spectrum() {
        let (mut tap, mut observation) = Observation::new();
        let block = [[0.; 2]; BLOCK];
        tap.capture(&block);
        assert_eq!(observation.samples.slots(), 0);
        assert!(observation.poll(true).is_none());
        for _ in 0..WINDOW / BLOCK {
            tap.capture(&block);
        }
        assert!(observation.poll(true).is_none());
        tap.capture(&block);
        assert!(observation.poll(true).is_some());
        assert!(observation.poll(false).is_none());
    }
}
