use nnnoiseless::DenoiseState;
use serde::{Deserialize, Serialize};

/// Sample frames per second used by the processor before output rate conversion.
pub const SAMPLE_RATE: u32 = 48_000;

/// Stereo frames in one 10 ms processing block.
pub const BLOCK: usize = DenoiseState::FRAME_SIZE;

/// Channel mapping applied before gain and noise suppression.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mixing {
    /// Keep left and right independent.
    #[default]
    Stereo,
    /// Send the arithmetic mean of left and right to both channels.
    Average,
    /// Send the left input to both channels.
    Left,
    /// Send the right input to both channels.
    Right,
}

/// Stateful block processor for one uninterrupted segment of microphone input.
///
/// Construct a fresh processor after a discontinuity to discard model history.
/// Mono sources duplicate each sample into both channels. See the [crate example](crate).
pub struct Processor {
    settings: crate::Settings,
    filters: Vec<Box<DenoiseState<'static>>>,
}

impl Processor {
    /// Allocate one denoiser per independent channel when suppression is enabled.
    /// Call on the control thread before starting the audio callback.
    pub fn new(settings: &crate::Settings) -> Self {
        let channels = if settings.mixing == Mixing::Stereo {
            2
        } else {
            1
        };
        let filters = if settings.suppress_noise && !settings.muted {
            (0..channels).map(|_| DenoiseState::new()).collect()
        } else {
            Vec::new()
        };
        Self {
            settings: settings.clone(),
            filters,
        }
    }

    /// Process normalized stereo samples in place and return the absolute output peak.
    ///
    /// Non-finite input becomes silence. Gain is limited to 200% and the final
    /// signal is clipped to full scale. A muted block is entirely silent.
    pub fn process(&mut self, frames: &mut [[f32; 2]; BLOCK]) -> f32 {
        if self.settings.muted {
            frames.fill([0.; 2]);
            return 0.;
        }
        for frame in frames.iter_mut() {
            for sample in frame.iter_mut() {
                *sample = if sample.is_finite() {
                    sample.clamp(-1., 1.)
                } else {
                    0.
                };
            }
            *frame = match self.settings.mixing {
                Mixing::Stereo => *frame,
                Mixing::Average => [(frame[0] + frame[1]) * 0.5; 2],
                Mixing::Left => [frame[0]; 2],
                Mixing::Right => [frame[1]; 2],
            };
        }
        for (channel, filter) in self.filters.iter_mut().enumerate() {
            // RNNoise takes signed 16-bit PCM magnitudes represented as floats.
            let input: [f32; BLOCK] = std::array::from_fn(|i| frames[i][channel] * 32768.);
            let mut output = [0.; BLOCK];
            filter.process_frame(&mut output, &input);
            for (frame, sample) in frames.iter_mut().zip(output) {
                frame[channel] = sample / 32768.;
                if self.settings.mixing != Mixing::Stereo {
                    frame[1] = frame[0];
                }
            }
        }
        let gain = f32::from(self.settings.gain_percent.min(200)) / 100.;
        let mut peak = 0_f32;
        for sample in frames.iter_mut().flatten() {
            *sample = (*sample * gain).clamp(-1., 1.);
            peak = peak.max(sample.abs());
        }
        peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;

    #[test]
    fn maps_channels_then_applies_gain() {
        for (mixing, expected) in [
            (Mixing::Stereo, [1., -0.5]),
            (Mixing::Average, [0.5; 2]),
            (Mixing::Left, [1.; 2]),
            (Mixing::Right, [-0.5; 2]),
        ] {
            let mut frames = [[0.75, -0.25]; BLOCK];
            let mut processor = Processor::new(&Settings {
                mixing,
                gain_percent: 200,
                ..Default::default()
            });
            processor.process(&mut frames);
            assert!(frames.iter().all(|frame| *frame == expected));
        }
    }

    #[test]
    fn suppression_reduces_stationary_noise() {
        let mut processor = Processor::new(&Settings {
            suppress_noise: true,
            mixing: Mixing::Left,
            ..Default::default()
        });
        let mut rng = 17_u32;
        let (mut original, mut filtered) = (0.0, 0.0);
        for block in 0..100 {
            let mut frames: [[f32; 2]; BLOCK] = std::array::from_fn(|_| {
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                [0.05 * (rng as i32 as f32 / i32::MAX as f32); 2]
            });
            let energy = |frames: &[[f32; 2]; BLOCK]| {
                frames.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>()
            };
            let input = energy(&frames);
            processor.process(&mut frames);
            if block >= 50 {
                original += input;
                filtered += energy(&frames);
            }
        }
        assert!(filtered < original * 0.5);
    }

    #[test]
    fn mute_and_invalid_input_are_silent() {
        for muted in [false, true] {
            let mut processor = Processor::new(&Settings {
                muted,
                suppress_noise: true,
                ..Default::default()
            });
            let mut frames = [if muted {
                [1.; 2]
            } else {
                [f32::NAN, f32::INFINITY]
            }; BLOCK];
            assert_eq!(processor.process(&mut frames), 0.);
            assert_eq!(frames, [[0.; 2]; BLOCK]);
        }
    }
}
