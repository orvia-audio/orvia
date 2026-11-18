//! Streaming spectral subtraction for relatively stationary background noise.
use crate::BLOCK;
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::{f32::consts::PI, sync::Arc};

const N: usize = BLOCK * 2;
const BINS: usize = N / 2 + 1;

pub(super) struct Spectral {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    bins: Vec<Complex32>,
    window: Vec<f32>,
    previous: [f32; BLOCK],
    overlap: [f32; BLOCK],
    smoothed: Vec<f32>,
    minimum: Vec<f32>,
    last_minimum: Vec<f32>,
    hops: usize,
}

impl Spectral {
    pub(super) fn new() -> Self {
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(N);
        let inverse = planner.plan_fft_inverse(N);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        Self {
            forward,
            inverse,
            scratch: vec![Complex32::default(); scratch_len],
            bins: vec![Complex32::default(); N],
            window: (0..N)
                .map(|i| (PI * (i as f32 + 0.5) / N as f32).sin())
                .collect(),
            previous: [0.; BLOCK],
            overlap: [0.; BLOCK],
            smoothed: vec![0.; BINS],
            minimum: vec![f32::INFINITY; BINS],
            last_minimum: vec![0.; BINS],
            hops: 0,
        }
    }

    pub(super) fn process(&mut self, input: &[f32; BLOCK], output: &mut [f32; BLOCK]) {
        for (i, &v) in self.previous.iter().chain(input).enumerate() {
            self.bins[i] = Complex32::new(v * self.window[i], 0.);
        }
        self.previous.copy_from_slice(input);
        self.forward
            .process_with_scratch(&mut self.bins, &mut self.scratch);
        // Track the minimum smoothed power across two half-second windows.
        // Brief increases above this floor retain more of their original energy.
        for k in 0..BINS {
            let power = self.bins[k].norm_sqr();
            let smooth = 0.85 * self.smoothed[k] + 0.15 * power;
            self.smoothed[k] = smooth;
            self.minimum[k] = self.minimum[k].min(smooth);
            let noise = self.minimum[k].min(self.last_minimum[k]);
            let gain = (1. - 1.8 * noise / smooth.max(1e-16))
                .clamp(0.025, 1.)
                .sqrt();
            self.bins[k] *= gain;
            if k > 0 && k < N / 2 {
                self.bins[N - k] *= gain;
            }
        }
        self.hops += 1;
        if self.hops == 50 {
            self.last_minimum.copy_from_slice(&self.minimum);
            self.minimum.fill(f32::INFINITY);
            self.hops = 0;
        }
        self.inverse
            .process_with_scratch(&mut self.bins, &mut self.scratch);
        // With half-window overlap, the squared sine windows sum to one.
        for (i, sample) in output.iter_mut().enumerate() {
            *sample = self.overlap[i] + self.bins[i].re * self.window[i] / N as f32;
            self.overlap[i] = self.bins[i + BLOCK].re * self.window[i + BLOCK] / N as f32;
        }
    }
}
