//! On-demand FFT of contiguous stereo output, computed on the engine control thread.

use orvia_core::{SAMPLE_RATE, spectrum::Spectrum};
use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex32};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::{
    f32::consts::TAU,
    sync::Arc,
    time::{Duration, Instant},
};

const WINDOW: usize = 8192;
const BIN_HZ: f32 = SAMPLE_RATE as f32 / WINDOW as f32;
const LAST_PEAK_BIN: usize = (Spectrum::MAX_HZ / BIN_HZ).ceil() as usize;
type Frame = [f32; 2];

fn silent_spectrum() -> Spectrum {
    Spectrum {
        bands: vec![0; Spectrum::BANDS],
        peak_hz: None,
    }
}

/// One bounded copy per audio block while a measurement is in progress.
pub(super) struct Tap {
    requests: Consumer<Vec<Frame>>,
    complete: Producer<Vec<Frame>>,
    filling: Option<Vec<Frame>>,
}

impl Tap {
    pub(super) fn capture(&mut self, block: &[Frame]) {
        let Some(mut window) = self.filling.take().or_else(|| self.requests.pop().ok()) else {
            return;
        };
        let count = block.len().min(WINDOW - window.len());
        // The control thread reserves the whole window before handing it to the callback.
        window.extend_from_slice(&block[..count]);
        if window.len() < WINDOW {
            self.filling = Some(window);
        } else if let Err(PushError::Full(window)) = self.complete.push(window) {
            self.filling = Some(window);
        }
    }
}

/// Exchanges one capture window with the processing thread while the spectrum is visible.
pub(super) struct Observation {
    requests: Producer<Vec<Frame>>,
    complete: Consumer<Vec<Frame>>,
    requested_at: Option<Instant>,
    analyzer: Option<Analyzer>,
    latest: Option<(Instant, Spectrum)>,
}

impl Observation {
    pub(super) fn new() -> (Tap, Self) {
        let (requests, incoming) = RingBuffer::new(1);
        let (completed, complete) = RingBuffer::new(1);
        (
            Tap {
                requests: incoming,
                complete: completed,
                filling: None,
            },
            Self {
                requests,
                complete,
                requested_at: None,
                analyzer: None,
                latest: None,
            },
        )
    }

    /// Discard stale windows after a stall or after the user leaves the spectrum.
    pub(super) fn poll(&mut self, visible: bool) -> Option<Spectrum> {
        const FRESH: Duration = Duration::from_millis(650);
        let mut completed = self.complete.pop().ok();
        if let Some(window) = completed.as_deref_mut()
            && let Some(start) = self.requested_at.take()
            && visible
            && start.elapsed() <= FRESH
        {
            let spectrum = if self.analyzer.is_none() && window.iter().all(|f| *f == [0., 0.]) {
                Some(silent_spectrum())
            } else {
                self.analyzer
                    .get_or_insert_with(Analyzer::new)
                    .measure(window)
            };
            self.latest = spectrum.map(|s| (start, s));
        }
        if !visible {
            self.latest = None;
            self.analyzer = None;
            return None;
        }
        if self.requested_at.is_none() {
            // Only this thread allocates, recycles and releases the capture window.
            let mut window = completed.unwrap_or_else(|| Vec::with_capacity(WINDOW));
            window.clear();
            if self.requests.push(window).is_ok() {
                self.requested_at = Some(Instant::now());
            }
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
    bands: [(std::ops::Range<usize>, f32); Spectrum::BANDS],
    input: Vec<f32>,
    bins: Vec<Complex32>,
    scratch: Vec<Complex32>,
}

impl Analyzer {
    fn new() -> Self {
        let plan = RealFftPlanner::new().plan_fft_forward(WINDOW);
        Self {
            bands: std::array::from_fn(|band| {
                let bin_at =
                    |index| Spectrum::frequency_at(index / Spectrum::BANDS as f32) / BIN_HZ;
                let start = bin_at(band as f32).ceil() as usize;
                let end = bin_at((band + 1) as f32).ceil() as usize;
                (start..end, bin_at(band as f32 + 0.5))
            }),
            hann: (0..WINDOW)
                .map(|i| 0.5 - 0.5 * (TAU * i as f32 / WINDOW as f32).cos())
                .collect(),
            input: plan.make_input_vec(),
            bins: plan.make_output_vec(),
            scratch: plan.make_scratch_vec(),
            plan,
        }
    }

    fn window_channel(&mut self, frames: &[Frame], channel: usize) {
        for ((sample, frame), weight) in self.input.iter_mut().zip(frames).zip(&self.hann) {
            *sample = frame[channel] * weight;
        }
    }

    fn transform(&mut self) -> Option<()> {
        self.plan
            .process_with_scratch(&mut self.input, &mut self.bins, &mut self.scratch)
            .ok()
    }

    fn measure(&mut self, frames: &mut [Frame]) -> Option<Spectrum> {
        let left = frames.iter().any(|f| f[0] != 0.);
        let right = frames.iter().any(|[l, r]| l != r) && frames.iter().any(|f| f[1] != 0.);
        if !left && !right {
            return Some(silent_spectrum());
        }
        self.window_channel(frames, usize::from(!left));
        self.transform()?;
        if left && right {
            self.window_channel(frames, 1);
        }
        // Both inputs have now been consumed. Reuse the contiguous capture allocation for
        // the stronger channel's power, including the neighbor used by peak interpolation.
        let power = &mut frames.as_flattened_mut()[..LAST_PEAK_BIN + 2];
        let scale = (4. / WINDOW as f32).powi(2);
        for (p, bin) in power.iter_mut().zip(&self.bins) {
            *p = (bin.norm_sqr() * scale).max(0.);
        }
        if left && right {
            self.transform()?;
            for (p, bin) in power.iter_mut().zip(&self.bins) {
                *p = p.max(bin.norm_sqr() * scale);
            }
        }
        let mut bands = Vec::with_capacity(Spectrum::BANDS);
        for (bins, mid) in &self.bands {
            let k = *mid as usize;
            // Low-frequency bands can be narrower than an FFT bin.
            let center = power[k] + mid.fract() * (power[k + 1] - power[k]);
            let max = power[bins.clone()].iter().copied().fold(center, f32::max);
            let range = f32::from(Spectrum::RANGE_DB);
            bands.push((range + 10. * max.log10()).round().clamp(0., range) as u8);
        }
        let mut strongest = 10_f32.powf(-f32::from(Spectrum::RANGE_DB) / 10.);
        let mut peak_hz = None;
        for k in (Spectrum::MIN_HZ / BIN_HZ).floor() as usize..=LAST_PEAK_BIN {
            let [left, center, right] = [power[k - 1], power[k], power[k + 1]];
            if center <= strongest || center < left || center <= right {
                continue;
            }
            // Interpolate the peak in log power to resolve tones between FFT bins.
            let [a, b, c] = [left, center, right].map(|p| p.max(f32::MIN_POSITIVE).ln());
            let offset = if a + c < 2. * b {
                (0.5 * (a - c) / (a + c - 2. * b)).clamp(-0.5, 0.5)
            } else {
                0.
            };
            let hz = ((k as f32 + offset) * BIN_HZ).round();
            if (Spectrum::MIN_HZ..=Spectrum::MAX_HZ).contains(&hz) {
                strongest = center;
                peak_hz = Some(hz as u16);
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
            let mut frames: Vec<_> = (0..WINDOW)
                .map(|i| {
                    let x = (TAU * hz * i as f32 / SAMPLE_RATE as f32).sin();
                    [left * x, right * x]
                })
                .collect();
            let s = analyzer.measure(&mut frames).unwrap();
            assert!(s.peak_hz.unwrap().abs_diff(hz as u16) <= 1);
            let band = ((hz / Spectrum::MIN_HZ).log10() / 3. * Spectrum::BANDS as f32) as usize;
            assert!((72..=74).contains(&s.bands[band]), "{hz} Hz: {:?}", s.bands);
        }
    }

    #[test]
    fn silence_and_dc_have_no_audible_peak() {
        let mut analyzer = Analyzer::new();
        for x in [0., 0.8] {
            let s = analyzer.measure(&mut vec![[x; 2]; WINDOW]).unwrap();
            assert!(s.peak_hz.is_none() && s.bands.iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn detects_tones_at_both_display_limits_and_between_bins() {
        let mut analyzer = Analyzer::new();
        for hz in [Spectrum::MIN_HZ, 150.5 * BIN_HZ, Spectrum::MAX_HZ] {
            let mut frames: Vec<_> = (0..WINDOW)
                .map(|i| [(TAU * hz * i as f32 / SAMPLE_RATE as f32).sin(); 2])
                .collect();
            let readout = analyzer.measure(&mut frames).unwrap().peak_hz;
            assert!(
                readout.is_some_and(|peak| peak.abs_diff(hz.round() as u16) <= 1),
                "{hz} Hz: {readout:?}"
            );
        }
    }

    #[test]
    fn capture_reuses_a_contiguous_window_and_releases_it_after_hiding() {
        let (mut tap, mut observation) = Observation::new();
        let frames: Vec<_> = (0..WINDOW + BLOCK)
            .map(|i| [i as f32, -(i as f32)])
            .collect();
        assert!(observation.poll(true).is_none());
        for block in frames.chunks(BLOCK) {
            tap.capture(block);
        }
        let captured = observation.complete.peek().unwrap();
        assert_eq!(captured, &frames[..WINDOW]);
        let storage = captured.as_ptr();
        assert!(observation.poll(true).is_some());

        tap.capture(&frames[..BLOCK]);
        assert_eq!(tap.filling.as_ref().unwrap().as_ptr(), storage);
        assert!(observation.poll(false).is_none());
        tap.capture(&frames[BLOCK..WINDOW]);
        assert!(observation.poll(false).is_none());

        tap.capture(&frames[..BLOCK]);
        assert!(tap.filling.is_none());
        assert!(observation.complete.peek().is_err());
        assert!(observation.requested_at.is_none());
    }

    #[test]
    fn idle_tap_and_partial_windows_do_not_publish_a_spectrum() {
        let (mut tap, mut observation) = Observation::new();
        let block = [[0.; 2]; BLOCK];
        tap.capture(&block);
        assert!(tap.filling.is_none());
        assert_eq!(observation.complete.slots(), 0);
        assert!(observation.poll(true).is_none());
        for _ in 0..WINDOW / BLOCK {
            tap.capture(&block);
        }
        assert!(observation.poll(true).is_none());
        tap.capture(&block);
        assert!(observation.poll(true).is_some());
        assert!(observation.poll(false).is_none());
        assert!(observation.analyzer.is_none());
    }

    #[test]
    fn an_initial_silent_window_publishes_without_planning_an_fft() {
        let (mut tap, mut observation) = Observation::new();
        observation.poll(true);
        tap.capture(&vec![[0., -0.]; WINDOW]);
        assert_eq!(observation.poll(true), Some(silent_spectrum()));
        assert!(observation.analyzer.is_none());
        tap.capture(&vec![[0., 0.1]; WINDOW]);
        assert!(observation.poll(true).is_some());
        assert!(observation.analyzer.is_some());
    }
}
