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
use net::{DF_ORDER, NB_DF, NB_ERB, Network, RecurrentStage};
use ramp::Ramp;
use realfft::num_complex::Complex32;
use std::time::Duration;

/// Input and output sample rate in Hz.
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples consumed and produced per call (10 ms).
pub const FRAME_SIZE: usize = HOP;
/// STFT delay plus two frames of model lookahead (30 ms).
pub const ALGORITHM_DELAY: usize = dsp::FFT - HOP + LOOKAHEAD * HOP;
const LOOKAHEAD: usize = 2;
const FULL_FRAMES: usize = LOOKAHEAD + 1;
const LOW_BAND_FRAMES: usize = DF_ORDER - LOOKAHEAD;
const _: () = assert!(ALGORITHM_DELAY.is_multiple_of(FRAME_SIZE));
// Drain analysis history, all DF taps and synthesis overlap before sleeping.
const TAIL_FRAMES: usize = DF_ORDER + 1;
const RESUME_FADE_SAMPLES: usize = 2 * FRAME_SIZE;

#[derive(Clone, Copy)]
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
    // The target becomes synthesis scratch after its original low bins have been saved.
    spectra: [[Complex32; BINS]; FULL_FRAMES],
    // Original low bins of the two past frames and the target, oldest first.
    low_history: [[Complex32; NB_DF]; LOW_BAND_FRAMES],
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
            spectra: [[Complex32::default(); BINS]; FULL_FRAMES],
            low_history: [[Complex32::default(); NB_DF]; LOW_BAND_FRAMES],
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

    /// Copy the signal history and controls of a stream using the same model.
    ///
    /// Call between complete frames to split two previously identical channels.
    /// The destination reuses its FFT plans, scratch buffers and model weights.
    pub fn copy_stream_from(&mut self, source: &Self) {
        self.stft.copy_history_from(&source.stft);
        self.features.copy_history_from(&source.features);
        self.net.copy_history_from(&source.net);
        // Target/synthesis scratch is overwritten on the next analysis.
        self.spectra[1..].copy_from_slice(&source.spectra[1..]);
        self.low_history = source.low_history;
        self.attenuation = source.attenuation;
        self.recovery = source.recovery;
        self.dry = source.dry;
        self.dry_position = source.dry_position;
        self.state = source.state;
        self.thresholds = source.thresholds;
        self.zero_frames = source.zero_frames;
    }

    fn reset_model(&mut self) {
        self.stft.reset();
        self.features.reset();
        self.net.reset();
        self.spectra = [[Complex32::default(); BINS]; FULL_FRAMES];
        self.low_history = [[Complex32::default(); NB_DF]; LOW_BAND_FRAMES];
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
        self.mix_dry(input, output, ready);
        lsnr
    }

    fn mix_dry(&mut self, input: &[f32; FRAME_SIZE], output: &mut [f32; FRAME_SIZE], ready: bool) {
        let dry = &mut self.dry[self.dry_position..][..FRAME_SIZE];
        self.dry_position = (self.dry_position + FRAME_SIZE) % ALGORITHM_DELAY;
        let recovery = if ready {
            self.recovery.settled()
        } else {
            Some(0.0)
        };
        if let (Some(strength), Some(recovery)) = (self.attenuation.settled(), recovery) {
            let wet = strength * recovery;
            if wet == 0.0 {
                output.copy_from_slice(dry);
            } else {
                for (&dry, enhanced) in dry.iter().zip(output) {
                    *enhanced = dry * (1.0 - wet) + *enhanced * wet;
                }
            }
        } else {
            for (&dry, enhanced) in dry.iter().zip(output) {
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
        }
        dry.copy_from_slice(input);
    }

    fn process_enhanced(
        &mut self,
        input: &[f32; FRAME_SIZE],
        output: &mut [f32; FRAME_SIZE],
    ) -> f32 {
        self.analyze(input);
        let f = &self.features;
        let lsnr = self.net.encode(&f.erb, &f.re, &f.im);
        if self.needs_mask(lsnr) {
            self.net.erb_decode(&mut self.gains);
        }
        self.apply_mask(lsnr);
        if self.needs_deep_filter(lsnr) {
            self.net.df_decode(&mut self.coefs);
            self.apply_deep_filter();
        }
        self.stft.synthesize(&mut self.spectra[0], output);
        lsnr
    }

    fn analyze(&mut self, input: &[f32; FRAME_SIZE]) {
        self.spectra.copy_within(1.., 0);
        // Preserve the original low bins even when this frame skips the deep filter.
        self.low_history.copy_within(1.., 0);
        self.low_history[LOW_BAND_FRAMES - 1].copy_from_slice(&self.spectra[0][..NB_DF]);
        let newest = &mut self.spectra[FULL_FRAMES - 1];
        self.stft.analyze(input, newest);
        self.features.compute(newest);
    }

    fn needs_mask(&self, snr: f32) -> bool {
        snr >= self.thresholds[0] && snr <= self.thresholds[1]
    }
    fn needs_deep_filter(&self, snr: f32) -> bool {
        self.needs_mask(snr) && snr <= self.thresholds[2]
    }

    fn apply_mask(&mut self, lsnr: f32) {
        if self.needs_mask(lsnr) {
            let mut bins = self.spectra[0].iter_mut();
            for (&width, &gain) in self.features.erb_widths.iter().zip(&self.gains) {
                bins.by_ref().take(width).for_each(|bin| *bin *= gain);
            }
        } else if lsnr < self.thresholds[0] {
            self.spectra[0].fill(Complex32::default());
        }
    }

    fn apply_deep_filter(&mut self) {
        let [target, next, newest] = &mut self.spectra;
        let [oldest, previous, original] = &self.low_history;
        let taps: [&[Complex32]; DF_ORDER] =
            [oldest, previous, original, &next[..NB_DF], &newest[..NB_DF]];
        for (f, bin) in target[..NB_DF].iter_mut().enumerate() {
            let c = &self.coefs[f * DF_ORDER * 2..];
            *bin = (0..DF_ORDER).fold(Complex32::default(), |sum, o| {
                sum + taps[o][f] * Complex32::new(c[o * 2], c[o * 2 + 1])
            });
        }
    }

    /// Process two independent channels. Shared matrix reads preserve separate recurrent states.
    /// Both arguments must be distinct uninterrupted mono streams. Warm-up and sleep follow
    /// the same sample-clocked transitions as individual calls to [`Self::process`].
    pub fn process_pair(
        left: &mut Self,
        right: &mut Self,
        inputs: &[[f32; FRAME_SIZE]; 2],
        outputs: &mut [[f32; FRAME_SIZE]; 2],
    ) {
        let running = |channel: &Self, input: &[f32; FRAME_SIZE]| {
            matches!(channel.state, State::Running)
                && !channel.attenuation.is_zero()
                && input.iter().any(|&v| v != 0.)
        };
        if !left.net.can_batch_with(&right.net)
            || !running(left, &inputs[0])
            || !running(right, &inputs[1])
        {
            left.process(&inputs[0], &mut outputs[0]);
            right.process(&inputs[1], &mut outputs[1]);
            return;
        }
        for (channel, input) in [&mut *left, &mut *right].into_iter().zip(inputs) {
            channel.zero_frames = 0;
            channel.analyze(input);
            let f = &channel.features;
            channel.net.encode_features(&f.erb, &f.re, &f.im);
        }
        left.net
            .recurrent_pair(&mut right.net, RecurrentStage::Encoder);
        let snr = [left.net.encoded_snr(), right.net.encoded_snr()];
        if left.needs_mask(snr[0]) && right.needs_mask(snr[1]) {
            left.net.prepare_erb();
            right.net.prepare_erb();
            left.net.recurrent_pair(&mut right.net, RecurrentStage::Erb);
            left.net.finish_erb(&mut left.gains);
            right.net.finish_erb(&mut right.gains);
        } else {
            for (channel, snr) in [&mut *left, &mut *right].into_iter().zip(snr) {
                if channel.needs_mask(snr) {
                    channel.net.erb_decode(&mut channel.gains);
                }
            }
        }
        left.apply_mask(snr[0]);
        right.apply_mask(snr[1]);
        if left.needs_deep_filter(snr[0]) && right.needs_deep_filter(snr[1]) {
            left.net.prepare_df();
            right.net.prepare_df();
            left.net.recurrent_pair(&mut right.net, RecurrentStage::Df);
            left.net.finish_df(&mut left.coefs);
            right.net.finish_df(&mut right.coefs);
        } else {
            for (channel, snr) in [&mut *left, &mut *right].into_iter().zip(snr) {
                if channel.needs_deep_filter(snr) {
                    channel.net.df_decode(&mut channel.coefs);
                }
            }
        }
        for (((channel, snr), input), output) in
            [left, right].into_iter().zip(snr).zip(inputs).zip(outputs)
        {
            if channel.needs_deep_filter(snr) {
                channel.apply_deep_filter();
            }
            channel.stft.synthesize(&mut channel.spectra[0], output);
            channel.mix_dry(input, output, true);
        }
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

#[cfg(all(test, feature = "embedded-model"))]
mod tests {
    use super::*;

    #[test]
    fn each_deep_filter_tap_reads_original_bins_after_in_place_synthesis() {
        let mut denoiser = Denoiser::new();
        let mut reference = Stft::new();
        let mut full_history = [[Complex32::default(); BINS]; DF_ORDER];
        denoiser.gains.fill(0.25);
        for frame in 0..4 * DF_ORDER {
            let input = std::array::from_fn(|i| ((frame * FRAME_SIZE + i) as f32 * 0.071).sin());
            full_history.copy_within(1.., 0);
            reference.analyze(&input, &mut full_history[DF_ORDER - 1]);
            denoiser.analyze(&input);
            assert_eq!(denoiser.spectra[0], full_history[DF_ORDER - 1 - LOOKAHEAD]);

            // Alternate pass-through, muting and deep filtering. Each FIR tap is selected in turn.
            let snr = [50.0, -15.0, 0.0][frame % 3];
            denoiser.apply_mask(snr);
            if denoiser.needs_deep_filter(snr) {
                let tap = frame % DF_ORDER;
                for coefs in denoiser.coefs.chunks_exact_mut(2 * DF_ORDER) {
                    coefs.fill(0.0);
                    coefs[2 * tap] = 1.0;
                }
                denoiser.apply_deep_filter();
                assert_eq!(denoiser.spectra[0][..NB_DF], full_history[tap][..NB_DF]);
            }
            denoiser
                .stft
                .synthesize(&mut denoiser.spectra[0], &mut [0.0; FRAME_SIZE]);
        }
    }

    #[test]
    fn paired_streams_match_independent_filters_through_stage_changes_and_silence() {
        let mut pair = [Denoiser::new(), Denoiser::new()];
        let mut reference = [Denoiser::new(), Denoiser::new()];
        for n in 0..90 {
            for filters in [&mut pair, &mut reference] {
                match n {
                    0 | 67 => {
                        for channel in filters {
                            channel.set_thresholds(-100., 100., 100.);
                        }
                    }
                    13 => filters[1].set_thresholds(-100., 100., -100.),
                    29 => filters[0].set_thresholds(100., 100., 100.),
                    45 => filters[1].set_attenuation_limit_smooth(0., Duration::from_millis(15)),
                    53 => filters[1].set_attenuation_limit_smooth(12., Duration::from_millis(25)),
                    _ => {}
                }
            }
            let input = std::array::from_fn(|ch| {
                std::array::from_fn(|i| {
                    let phase = (n * FRAME_SIZE + i) as f32 * [0.02, 0.057][ch];
                    if ch == 0 && (56..69).contains(&n) {
                        0.
                    } else {
                        phase.sin() * 0.25
                    }
                })
            });
            let mut actual = [[0.; FRAME_SIZE]; 2];
            let [left, right] = &mut pair;
            Denoiser::process_pair(left, right, &input, &mut actual);
            for (ch, denoiser) in reference.iter_mut().enumerate() {
                let mut expected = [0.; FRAME_SIZE];
                denoiser.process(&input[ch], &mut expected);
                assert_eq!(
                    actual[ch].map(f32::to_bits),
                    expected.map(f32::to_bits),
                    "block {n} channel {ch}"
                );
            }
        }
    }
}
