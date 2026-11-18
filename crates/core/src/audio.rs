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
    #[serde(alias = "left", alias = "right")]
    Average,
}

/// Algorithm applied before gain and monitoring when noise suppression is enabled.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseMethod {
    /// Speech-oriented recurrent neural network.
    #[default]
    Rnnoise,
    /// Adaptive per-frequency subtraction for steady background sounds.
    Spectral,
}

/// Signal envelopes and mean-square energies for one aligned 10 ms block.
/// Measurements are normalized to full scale and taken on either side of the
/// noise filter, ahead of output gain. No PCM samples are retained here.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockLevels {
    /// Largest absolute sample entering the filter.
    pub before_peak: f32,
    /// Largest absolute sample leaving the filter.
    pub after_peak: f32,
    /// Input mean-square energy across both channels.
    pub before_power: f32,
    /// Output mean-square energy across both channels.
    pub after_power: f32,
}

/// Compact pre-gain measurements for the settings window.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SignalHistory {
    /// Pairs of input and filtered peaks, in 1 dB steps above −60 dBFS.
    /// Each pair spans 10 ms, with the earliest pair first.
    pub peaks: Vec<[u8; 2]>,
    /// Energy reduction in dB over the newest half second. Absent for silence.
    pub attenuation_db: Option<f32>,
}

impl SignalHistory {
    /// Summarize up to two seconds of consecutive blocks without retaining PCM.
    pub fn from_blocks<'a>(blocks: impl DoubleEndedIterator<Item = &'a BlockLevels>) -> Self {
        let mut peaks = Vec::with_capacity(200);
        let (mut before, mut after) = (0., 0.);
        let to_scale = |v: f32| (60. + 20. * v.max(0.001).log10()).clamp(0., 60.).round() as u8;
        for (i, b) in blocks.rev().take(200).enumerate() {
            peaks.push([to_scale(b.before_peak), to_scale(b.after_peak)]);
            if i < 50 {
                before += b.before_power;
                after += b.after_power;
            }
        }
        peaks.reverse();
        let count = peaks.len().min(50) as f32;
        let attenuation_db = (count > 0. && before > 1e-8 * count)
            .then(|| 10. * (before / after.max(1e-12 * count)).log10());
        Self {
            peaks,
            attenuation_db,
        }
    }
}

fn levels(frames: &[[f32; 2]; BLOCK]) -> (f32, f32) {
    let (mut peak, mut energy) = (0_f32, 0.);
    for &s in frames.iter().flatten() {
        peak = peak.max(s.abs());
        energy += s * s;
    }
    (peak, energy / (2 * BLOCK) as f32)
}

enum Filter {
    Rnnoise(Box<DenoiseState<'static>>),
    Spectral(Box<crate::spectral::Spectral>),
}

/// Stateful block processor for one uninterrupted segment of microphone input.
///
/// Construct a fresh processor after a discontinuity to discard model history.
/// Mono sources duplicate each sample into both channels. See the [crate example](crate).
pub struct Processor {
    settings: crate::Settings,
    filters: Vec<Filter>,
    previous_input: (f32, f32),
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
            (0..channels)
                .map(|_| match settings.noise_method {
                    NoiseMethod::Rnnoise => Filter::Rnnoise(DenoiseState::new()),
                    NoiseMethod::Spectral => {
                        Filter::Spectral(Box::new(crate::spectral::Spectral::new()))
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            settings: settings.clone(),
            filters,
            previous_input: (0., 0.),
        }
    }

    /// Process normalized stereo samples in place and return the absolute output peak.
    ///
    /// Non-finite input becomes silence. Gain is limited to 200% and the final
    /// signal is clipped to full scale. A muted block is entirely silent.
    pub fn process(&mut self, frames: &mut [[f32; 2]; BLOCK]) -> f32 {
        self.process_with_levels(frames).0
    }

    /// Change linear output gain without clearing filter history. 100 means unity.
    pub fn set_gain(&mut self, percent: u16) {
        self.settings.gain_percent = percent.min(200);
    }

    /// Process one block and return its output peak and pre-gain filter measurements.
    /// Both filters use overlapping windows. The input envelope is delayed by one
    /// block to compare the same interval on both sides of the filter.
    pub fn process_with_levels(&mut self, frames: &mut [[f32; 2]; BLOCK]) -> (f32, BlockLevels) {
        if self.settings.muted {
            frames.fill([0.; 2]);
            return (0., BlockLevels::default());
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
            };
        }
        let input_levels = levels(frames);
        for (channel, filter) in self.filters.iter_mut().enumerate() {
            let mut output = [0.; BLOCK];
            match filter {
                Filter::Rnnoise(f) => {
                    let input = std::array::from_fn::<_, BLOCK, _>(|i| frames[i][channel] * 32768.);
                    f.process_frame(&mut output, &input);
                    for s in &mut output {
                        *s /= 32768.;
                    }
                }
                Filter::Spectral(f) => {
                    f.process(&std::array::from_fn(|i| frames[i][channel]), &mut output)
                }
            }
            for (frame, sample) in frames.iter_mut().zip(output) {
                frame[channel] = sample;
                if self.settings.mixing != Mixing::Stereo {
                    frame[1] = frame[0];
                }
            }
        }
        let (before, after) = if self.filters.is_empty() {
            (input_levels, input_levels)
        } else {
            (
                std::mem::replace(&mut self.previous_input, input_levels),
                levels(frames),
            )
        };
        let gain = f32::from(self.settings.gain_percent.min(200)) / 100.;
        let mut peak = 0_f32;
        for sample in frames.iter_mut().flatten() {
            *sample = (*sample * gain).clamp(-1., 1.);
            peak = peak.max(sample.abs());
        }
        (
            peak,
            BlockLevels {
                before_peak: before.0,
                before_power: before.1,
                after_peak: after.0,
                after_power: after.1,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;

    #[test]
    fn maps_channels_then_applies_gain() {
        for (mixing, expected) in [(Mixing::Stereo, [1., -0.5]), (Mixing::Average, [0.5; 2])] {
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
        for method in [NoiseMethod::Rnnoise, NoiseMethod::Spectral] {
            let mut processor = Processor::new(&Settings {
                suppress_noise: true,
                noise_method: method,
                mixing: Mixing::Average,
                ..Default::default()
            });
            let mut rng = 17_u32;
            let (mut original, mut filtered) = (0.0, 0.0);
            for block in 0..250 {
                let mut frames: [[f32; 2]; BLOCK] = std::array::from_fn(|_| {
                    rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    [0.05 * (rng as i32 as f32 / i32::MAX as f32); 2]
                });
                let energy = |frames: &[[f32; 2]; BLOCK]| {
                    frames.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>()
                };
                let input = energy(&frames);
                processor.process(&mut frames);
                if block >= 150 {
                    original += input;
                    filtered += energy(&frames);
                }
            }
            assert!(
                filtered < original * 0.5,
                "{method:?}: filtered {filtered} of {original}"
            );
        }
    }

    #[test]
    fn spectral_filter_preserves_the_onset_of_a_new_tone() {
        let mut p = Processor::new(&Settings {
            suppress_noise: true,
            noise_method: NoiseMethod::Spectral,
            mixing: Mixing::Average,
            ..Default::default()
        });
        for _ in 0..150 {
            p.process(&mut [[0.; 2]; BLOCK]);
        }
        let tone =
            std::array::from_fn(|i| [(i as f32 * std::f32::consts::TAU / 48.).sin() * 0.4; 2]);
        let mut first = tone;
        p.process(&mut first);
        let mut next = tone;
        let (_, levels) = p.process_with_levels(&mut next);
        assert!((levels.before_peak - 0.4).abs() < 1e-5);
        assert!((levels.after_power - levels.before_power).abs() < 1e-4);
        assert!(
            next.iter()
                .zip(tone)
                .all(|(a, b)| (a[0] - b[0]).abs() < 1e-4 && a[0] == a[1])
        );
    }

    #[test]
    fn output_gain_does_not_change_the_filter_measurement() {
        let mut p = Processor::new(&Settings::default());
        for gain in [0, 25, 100, 200] {
            p.set_gain(gain);
            let (peak, b) = p.process_with_levels(&mut [[0.25, -0.25]; BLOCK]);
            assert_eq!(peak, 0.25 * f32::from(gain) / 100.);
            assert_eq!(b.before_power, 0.0625);
            assert_eq!(b.after_power, b.before_power);
        }
    }

    #[test]
    fn history_shares_a_scale_and_compares_the_latest_half_second() {
        let mut blocks = vec![
            BlockLevels {
                before_peak: 1.,
                after_peak: 0.1,
                before_power: 1.,
                after_power: 1.
            };
            210
        ];
        for b in &mut blocks[160..] {
            b.after_power = 0.25;
        }
        let signal = SignalHistory::from_blocks(blocks.iter());
        assert_eq!(signal.peaks, vec![[60, 40]; 200]);
        assert!((signal.attenuation_db.unwrap() - 6.0206).abs() < 0.001);
        assert!(
            SignalHistory::from_blocks([BlockLevels::default()].iter())
                .attenuation_db
                .is_none()
        );
        assert!(
            SignalHistory::from_blocks([].iter())
                .attenuation_db
                .is_none()
        );
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
