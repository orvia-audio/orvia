//! DeepFilterNet's STFT (Vorbis window, 50% overlap) and DFN3 input features.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::net::{NB_DF, NB_ERB};

pub(crate) const FFT: usize = 960;
pub(crate) const HOP: usize = 480;
pub(crate) const BINS: usize = FFT / 2 + 1;
/// DeepFilterNet rounds `exp(-HOP / SR / tau)` with tau = 1 s to 0.99.
const ALPHA: f32 = 0.99;

pub(crate) struct Stft {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    scratch: Box<[Complex32]>,
    window: [f32; FFT],
    time: [f32; FFT],
    analysis: [f32; HOP],
    synthesis: [f32; HOP],
}

impl Stft {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::new();
        let (forward, inverse) = (planner.plan_fft_forward(FFT), planner.plan_fft_inverse(FFT));
        let scratch = forward.get_scratch_len().max(inverse.get_scratch_len());
        let pi = std::f64::consts::PI;
        Self {
            scratch: vec![Complex32::default(); scratch].into_boxed_slice(),
            window: std::array::from_fn(|i| {
                let s = (0.5 * pi * (i as f64 + 0.5) / HOP as f64).sin();
                (0.5 * pi * s * s).sin() as f32
            }),
            forward,
            inverse,
            time: [0.0; FFT],
            analysis: [0.0; HOP],
            synthesis: [0.0; HOP],
        }
    }

    pub fn reset(&mut self) {
        self.analysis.fill(0.0);
        self.synthesis.fill(0.0);
    }

    pub fn analyze(&mut self, input: &[f32; HOP], spec: &mut [Complex32; BINS]) {
        let (w, t) = (&self.window, &mut self.time);
        for i in 0..HOP {
            t[i] = self.analysis[i] * w[i];
            t[HOP + i] = input[i] * w[HOP + i];
        }
        self.analysis.copy_from_slice(input);
        self.forward
            .process_with_scratch(t, spec, &mut self.scratch)
            .expect("fixed-size FFT");
        let norm = 1.0 / ((FFT * FFT) as f32 / (2 * HOP) as f32);
        spec.iter_mut().for_each(|x| *x *= norm);
    }

    /// Consumes `spec` (used as FFT scratch).
    pub fn synthesize(&mut self, spec: &mut [Complex32; BINS], output: &mut [f32; HOP]) {
        spec[0].im = 0.0;
        spec[BINS - 1].im = 0.0;
        let _ = self
            .inverse
            .process_with_scratch(spec, &mut self.time, &mut self.scratch);
        let (w, t) = (&self.window, &self.time);
        for i in 0..HOP {
            output[i] = t[i] * w[i] + self.synthesis[i];
            self.synthesis[i] = t[HOP + i] * w[HOP + i];
        }
    }
}

/// DeepFilterNet's ERB filter bank: number of FFT bins per band.
pub(crate) fn erb_widths() -> [usize; NB_ERB] {
    let freq2erb = |f: f32| 9.265 * (f / (24.7 * 9.265)).ln_1p();
    let erb2freq = |e: f32| 24.7 * 9.265 * ((e / 9.265).exp() - 1.0);
    let (sr, min_bins) = (48_000.0_f32, 2_i32);
    let (low, high) = (freq2erb(0.0), freq2erb(sr / 2.0));
    let step = (high - low) / NB_ERB as f32;
    let mut widths = [0; NB_ERB];
    let (mut prev, mut over) = (0_i32, 0_i32);
    for (i, width) in widths.iter_mut().enumerate() {
        let bin = (erb2freq(low + (i + 1) as f32 * step) / (sr / FFT as f32)).round() as i32;
        let mut n = bin - prev - over;
        over = if n < min_bins { min_bins - n } else { 0 };
        n = n.max(min_bins);
        *width = n as usize;
        prev = bin;
    }
    widths[NB_ERB - 1] += 1;
    let excess = widths.iter().sum::<usize>() - BINS;
    widths[NB_ERB - 1] -= excess;
    widths
}

/// Exponentially normalized ERB band energies (dB) and unit-normalized complex bins.
pub(crate) struct Features {
    pub erb_widths: [usize; NB_ERB],
    mean: [f32; NB_ERB],
    unit: [f32; NB_DF],
    pub erb: [f32; NB_ERB],
    pub re: [f32; NB_DF],
    pub im: [f32; NB_DF],
}

impl Features {
    pub fn new() -> Self {
        let mut f = Self {
            erb_widths: erb_widths(),
            mean: [0.0; NB_ERB],
            unit: [0.0; NB_DF],
            erb: [0.0; NB_ERB],
            re: [0.0; NB_DF],
            im: [0.0; NB_DF],
        };
        f.reset();
        f
    }

    pub fn reset(&mut self) {
        let lerp = |a: f32, b: f32, n: usize, i: usize| a + i as f32 * ((b - a) / (n - 1) as f32);
        self.mean = std::array::from_fn(|i| lerp(-60.0, -90.0, NB_ERB, i));
        self.unit = std::array::from_fn(|i| lerp(0.001, 0.0001, NB_DF, i));
    }

    pub fn compute(&mut self, spec: &[Complex32; BINS]) {
        let mut bins = spec.iter();
        for ((&n, out), state) in self
            .erb_widths
            .iter()
            .zip(&mut self.erb)
            .zip(&mut self.mean)
        {
            let k = 1.0 / n as f32;
            let power = bins
                .by_ref()
                .take(n)
                .fold(0.0, |s, x| s + (x.re * x.re + x.im * x.im) * k);
            let db = (power + 1e-10).log10() * 10.0;
            *state = db * (1.0 - ALPHA) + *state * ALPHA;
            *out = (db - *state) / 40.0;
        }
        for (i, (x, state)) in spec.iter().zip(&mut self.unit).enumerate() {
            // Keep silence finite with FTZ/DAZ enabled and avoid denormal slowdowns.
            *state = (x.norm() * (1.0 - ALPHA) + *state * ALPHA).max(2.0 * f32::MIN_POSITIVE);
            let s = state.sqrt();
            (self.re[i], self.im[i]) = (x.re / s, x.im / s);
        }
    }
}
