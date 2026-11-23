//! Real-time DeepFilterNet3 speech enhancement for 48 kHz mono audio.
//!
//! Construction allocates; [`Denoiser::process`] and [`Denoiser::reset`]
//! perform no heap allocation, locking, or I/O. Weights are shared and may be
//! embedded (the default) or supplied externally through [`Model`].
//!
//! ```
//! # #[cfg(feature = "embedded-model")] {
//! let mut denoiser = dfn3_rs::Denoiser::new();
//! let input = [0.0_f32; dfn3_rs::FRAME_SIZE];
//! let mut output = [0.0_f32; dfn3_rs::FRAME_SIZE];
//! let lsnr = denoiser.process(&input, &mut output);
//! # }
//! ```

#![deny(unsafe_op_in_unsafe_fn)]

mod dsp;
mod net;
mod ramp;
mod simd;
mod weights;

pub use weights::{Model, ModelError, WEIGHTS_SIZE};

use dsp::{BINS, Features, HOP, Stft};
use net::{DF_ORDER, NB_DF, NB_ERB, Network};
use ramp::Ramp;
use realfft::num_complex::Complex32;
use std::time::Duration;

pub const SAMPLE_RATE: u32 = 48_000;
/// Samples consumed and produced per call (10 ms).
pub const FRAME_SIZE: usize = HOP;
/// STFT delay plus two frames of model lookahead (30 ms).
pub const ALGORITHM_DELAY: usize = dsp::FFT - HOP + 2 * HOP;
/// Spectra kept for deep filtering: two past, the target, and two lookahead frames.
const LOOKAHEAD: usize = 2;
// Drain analysis history, all DF taps and synthesis overlap before sleeping.
const TAIL_FRAMES: usize = DF_ORDER + 1;
const RESUME_FADE_SAMPLES: usize = 2 * FRAME_SIZE;

enum State {
    Running,
    Sleeping,
    Warming { remaining: usize },
}

/// One independent mono stream. Input must be finite, normalized PCM.
/// Construct outside the audio callback; use one instance per channel.
pub struct Denoiser {
    stft: Stft,
    features: Features,
    net: Box<Network>,
    spectra: [[Complex32; BINS]; DF_ORDER], // oldest first; the target is LOOKAHEAD frames old
    enhanced: [Complex32; BINS],
    gains: [f32; NB_ERB],
    coefs: [f32; NB_DF * DF_ORDER * 2],
    attenuation: Ramp,
    recovery: Ramp,
    dry: [f32; ALGORITHM_DELAY],
    dry_position: usize,
    state: State,
    thresholds: [f32; 3],
    zero_frames: usize,
}

#[cfg(feature = "embedded-model")]
impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl Denoiser {
    /// Construct using the embedded model, outside the audio callback.
    #[cfg(feature = "embedded-model")]
    pub fn new() -> Self {
        Self::with_model(Model::embedded())
    }

    /// Construct using shared, validated weights, outside the audio callback.
    pub fn with_model(model: Model) -> Self {
        Self {
            stft: Stft::new(),
            features: Features::new(),
            net: Box::new(Network::new(model)),
            spectra: [[Complex32::default(); BINS]; DF_ORDER],
            enhanced: [Complex32::default(); BINS],
            gains: [0.0; NB_ERB],
            coefs: [0.0; NB_DF * DF_ORDER * 2],
            attenuation: Ramp::new(1.0),
            recovery: Ramp::new(1.0),
            dry: [0.0; ALGORITHM_DELAY],
            dry_position: 0,
            state: State::Running,
            thresholds: [-10.0, 30.0, 20.0],
            zero_frames: 0,
        }
    }

    /// Starts an unrelated stream without allocating; retains thresholds and
    /// settles attenuation at its configured target, discarding any transition.
    pub fn reset(&mut self) {
        self.reset_model();
        self.dry.fill(0.0);
        self.dry_position = 0;
        self.zero_frames = 0;
        self.attenuation.reset(self.attenuation.target);
        self.recovery.reset(1.0);
        self.state = State::Running;
    }

    fn reset_model(&mut self) {
        self.stft.reset();
        self.features.reset();
        self.net.reset();
        self.spectra = [[Complex32::default(); BINS]; DF_ORDER];
    }

    /// Sets `atten-lim-db` immediately, cancelling any parameter transition.
    ///
    /// Call between frames on the processing thread. The next processing call
    /// uses the new limit. Audio delay remains fixed, including at zero dB.
    /// This setter performs no allocations, locks or I/O.
    ///
    /// Pass finite dB values: 0 gives delayed dry audio, larger magnitudes give
    /// stronger suppression, and 100 or more disables the limit (the default).
    pub fn set_attenuation_limit(&mut self, db: f32) {
        self.attenuation.reset(wet_mix(db));
    }

    /// Moves to `db` with a linear, per-output-sample mixture envelope.
    /// Duration rounds up to 48 kHz samples; zero duration sets immediately.
    /// Repeating the same target does not restart a transition. New targets
    /// start at the current mix. No allocation, locking or model rebuild occurs.
    ///
    /// Stable zero dB skips inference. Resuming after bypass or drained digital
    /// silence returns delayed dry audio during 30 ms of model warm-up, then
    /// fades in over 20 ms. Parameter transitions still advance during warm-up.
    pub fn set_attenuation_limit_smooth(&mut self, db: f32, duration: Duration) {
        let samples = (duration.as_secs_f64() * f64::from(SAMPLE_RATE)).ceil() as usize;
        self.attenuation.set_target(wet_mix(db), samples);
    }

    /// Local-SNR thresholds (dB) for skipping work, as in DeepFilterNet: below `min` the frame
    /// is muted, above `max_erb` it passes through, above `max_df` deep filtering is skipped.
    pub fn set_thresholds(&mut self, min: f32, max_erb: f32, max_df: f32) {
        self.thresholds = [min, max_erb, max_df];
    }

    /// Enhances in place, with the same delay and local-SNR result as [`Self::process`].
    pub fn process_frame(&mut self, frame: &mut [f32; FRAME_SIZE]) -> f32 {
        let input = *frame;
        self.process(&input, frame)
    }

    /// Enhances one frame; the output is delayed by [`ALGORITHM_DELAY`] samples.
    /// Returns the estimated local SNR in dB, or -15 when inference is skipped.
    /// Only exact digital silence can trigger idle skipping; quiet nonzero input
    /// still runs the model. Dry audio and parameter envelopes always advance.
    pub fn process(&mut self, input: &[f32; FRAME_SIZE], output: &mut [f32; FRAME_SIZE]) -> f32 {
        let silent = input.iter().all(|&sample| sample == 0.0);
        self.zero_frames = if silent {
            self.zero_frames.saturating_add(1)
        } else {
            0
        };
        if self.attenuation.is_zero() || self.zero_frames > TAIL_FRAMES {
            if !matches!(self.state, State::Sleeping) {
                self.reset_model();
                self.recovery.reset(0.0);
                self.state = State::Sleeping;
            }
        } else if matches!(self.state, State::Sleeping) && self.attenuation.target > 0.0 && !silent
        {
            self.state = State::Warming {
                remaining: ALGORITHM_DELAY / FRAME_SIZE,
            };
        }

        let (lsnr, ready) = match self.state {
            State::Sleeping => (-15.0, false),
            State::Warming { remaining } => {
                let lsnr = self.process_enhanced(input, output);
                self.state = if remaining == 1 {
                    self.recovery.set_target(1.0, RESUME_FADE_SAMPLES);
                    State::Running
                } else {
                    State::Warming {
                        remaining: remaining - 1,
                    }
                };
                (lsnr, false)
            }
            State::Running => (self.process_enhanced(input, output), true),
        };
        // Synthesis is linear: a constant mixture matches spectral attenuation.
        // Mixing aligned samples permits a truly sample-clocked transition with
        // one IFFT. The dry ring keeps moving even while the model is asleep.
        for (&sample, enhanced) in input.iter().zip(output) {
            let dry = std::mem::replace(&mut self.dry[self.dry_position], sample);
            self.dry_position = (self.dry_position + 1) % ALGORITHM_DELAY;
            let strength = self.attenuation.next();
            let wet = if ready {
                strength * self.recovery.next()
            } else {
                0.0
            };
            *enhanced = if wet == 0.0 {
                dry
            } else {
                dry * (1.0 - wet) + *enhanced * wet
            };
        }
        lsnr
    }

    fn process_enhanced(
        &mut self,
        input: &[f32; FRAME_SIZE],
        output: &mut [f32; FRAME_SIZE],
    ) -> f32 {
        self.spectra.copy_within(1.., 0);
        let newest = &mut self.spectra[DF_ORDER - 1];
        self.stft.analyze(input, newest);
        self.features.compute(newest);
        let f = &self.features;
        let lsnr = self.net.encode(&f.erb, &f.re, &f.im);

        let [min, max_erb, max_df] = self.thresholds;
        let target = &self.spectra[DF_ORDER - 1 - LOOKAHEAD];
        let mask = lsnr >= min && lsnr <= max_erb;
        let deep_filter = mask && lsnr <= max_df;
        if mask {
            self.net.erb_decode(&mut self.gains);
            let mut bins = self.enhanced.iter_mut().zip(target);
            for (&n, &g) in f.erb_widths.iter().zip(&self.gains) {
                bins.by_ref().take(n).for_each(|(e, &x)| *e = x * g);
            }
        } else if lsnr < min {
            self.enhanced.fill(Complex32::default());
        } else {
            self.enhanced.copy_from_slice(target);
        }
        if deep_filter {
            self.net.df_decode(&mut self.coefs);
            for (f, e) in self.enhanced[..NB_DF].iter_mut().enumerate() {
                let c = &self.coefs[f * DF_ORDER * 2..];
                *e = (0..DF_ORDER).fold(Complex32::default(), |sum, o| {
                    sum + self.spectra[o][f] * Complex32::new(c[o * 2], c[o * 2 + 1])
                });
            }
        }
        self.stft.synthesize(&mut self.enhanced, output);
        lsnr
    }
}

fn wet_mix(db: f32) -> f32 {
    assert!(db.is_finite(), "attenuation limit must be finite");
    if db.abs() >= 100.0 {
        1.0
    } else {
        1.0 - 10f32.powf(-db.abs() / 20.0)
    }
}
