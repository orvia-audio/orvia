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
    /// DeepFilterNet3 speech enhancement with an embedded CPU model.
    #[default]
    #[serde(rename = "deepfilternet3")]
    DeepFilterNet3,
    /// Speech-oriented recurrent neural network.
    Rnnoise,
    /// WebRTC speech-oriented suppression at the high suppression level.
    #[serde(rename = "webrtc", alias = "spectral")]
    WebRtc,
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
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
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
    WebRtc(Box<sonora::NoiseChannel>),
    DeepFilterNet3(Box<dfn3_rs::Denoiser>),
}

/// Stateful block processor for one uninterrupted segment of microphone input.
///
/// Construct a fresh processor after a discontinuity to discard model history.
/// Mono sources duplicate each sample into both channels. See the [crate example](crate).
pub struct Processor {
    settings: crate::Settings,
    filters: Vec<Filter>,
    linked_stereo: bool,
    tone: crate::tone::ToneProcessor,
    delayed_levels: std::collections::VecDeque<(f32, f32)>,
}

impl Processor {
    /// Allocate one denoiser per independent channel when suppression is enabled.
    /// Call on the control thread before starting the audio callback.
    pub fn new(settings: &crate::Settings) -> Self {
        Self::for_input(settings, 2)
    }

    /// Construct a processor for a negotiated capture format. A single-channel
    /// source must supply duplicated stereo frames and needs only one filter history.
    /// Multichannel sources use the first two channels and the selected mixing mode.
    pub fn for_input(settings: &crate::Settings, input_channels: u16) -> Self {
        let mut settings = settings.clone();
        if input_channels == 1 {
            settings.mixing = Mixing::Average;
        }
        let channels = if settings.mixing == Mixing::Stereo {
            2
        } else {
            1
        };
        let filters = if settings.suppress_noise {
            (0..channels)
                .map(|_| match settings.noise_method {
                    NoiseMethod::Rnnoise => Filter::Rnnoise(DenoiseState::new()),
                    NoiseMethod::DeepFilterNet3 => {
                        let mut filter = Box::new(dfn3_rs::Denoiser::new());
                        filter.set_attenuation_limit(f32::from(settings.deep_filter_limit_db));
                        Filter::DeepFilterNet3(filter)
                    }
                    NoiseMethod::WebRtc => Filter::WebRtc(Box::default()),
                })
                .collect()
        } else {
            Vec::new()
        };
        let delay_blocks = match settings.active_noise_method() {
            None => 0,
            Some(NoiseMethod::DeepFilterNet3) => dfn3_rs::ALGORITHM_DELAY / BLOCK,
            Some(NoiseMethod::WebRtc | NoiseMethod::Rnnoise) => 1,
        };
        Self {
            tone: crate::tone::ToneProcessor::new(settings.tone()),
            linked_stereo: channels == 2 && settings.suppress_noise,
            settings,
            filters,
            delayed_levels: vec![(0., 0.); delay_blocks].into(),
        }
    }

    /// Process normalized stereo samples in place and return the absolute output peak.
    ///
    /// Non-finite input becomes silence. Gain is limited to 200% and the final
    /// signal is clipped to full scale.
    pub fn process(&mut self, frames: &mut [[f32; 2]; BLOCK]) -> f32 {
        self.process_block::<false, true>(frames).0
    }

    /// Process audio without computing an output peak. Filter state and delayed input
    /// levels stay aligned when a mixer begins observing the following block.
    pub fn process_unmetered(&mut self, frames: &mut [[f32; 2]; BLOCK]) {
        self.process_block::<false, false>(frames);
    }

    /// Change linear output gain without clearing filter history. 100 means unity.
    pub fn set_gain(&mut self, percent: u16) {
        self.set_tone(crate::Tone {
            gain_percent: percent,
            ..self.settings.tone()
        });
    }

    /// Update output processing at a block boundary while preserving noise filter history.
    pub fn set_tone(&mut self, tone: crate::Tone) {
        self.settings.set_tone(tone);
        self.tone.configure(tone);
    }

    /// Ramp DeepFilterNet3 to a new attenuation limit over 20 ms while preserving
    /// its history and delay. Call between blocks. Other filters ignore the limit.
    pub fn set_deep_filter_limit(&mut self, db: u16) {
        if db == self.settings.deep_filter_limit_db {
            return;
        }
        self.settings.deep_filter_limit_db = db;
        for filter in &mut self.filters {
            if let Filter::DeepFilterNet3(denoiser) = filter {
                denoiser.set_attenuation_limit_smooth(
                    f32::from(db),
                    std::time::Duration::from_millis(20),
                );
            }
        }
    }

    /// Process one block and return its output peak and pre-gain filter measurements.
    /// The input envelope follows each filter's delay to the nearest 10 ms.
    pub fn process_with_levels(&mut self, frames: &mut [[f32; 2]; BLOCK]) -> (f32, BlockLevels) {
        self.process_block::<true, true>(frames)
    }

    fn process_block<const METER: bool, const OUTPUT_PEAK: bool>(
        &mut self,
        frames: &mut [[f32; 2]; BLOCK],
    ) -> (f32, BlockLevels) {
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
        // Keep the delayed input aligned if metering resumes on the next block.
        let input_levels = if METER || !self.delayed_levels.is_empty() {
            levels(frames)
        } else {
            (0., 0.)
        };
        self.suppress(frames);
        let (before, after) = if let Some(before) = self.delayed_levels.pop_front() {
            self.delayed_levels.push_back(input_levels);
            (before, if METER { levels(frames) } else { (0., 0.) })
        } else {
            (input_levels, input_levels)
        };
        // The mixer already bounded the input. With no active effects its output peak is the input peak.
        let peak = if self.filters.is_empty() && self.tone.is_bypassed() {
            if OUTPUT_PEAK && !METER {
                frames
                    .iter()
                    .flatten()
                    .fold(0_f32, |peak, s| peak.max(s.abs()))
            } else {
                input_levels.0
            }
        } else {
            self.tone.process_block::<OUTPUT_PEAK>(frames)
        };
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

    fn suppress(&mut self, frames: &mut [[f32; 2]; BLOCK]) {
        // Identical stereo input has identical recurrent history until the first differing sample.
        // Once split, equal samples alone cannot establish that the two histories match again.
        if self.linked_stereo && frames.iter().any(|[l, r]| l.to_bits() != r.to_bits()) {
            match self.filters.as_mut_slice() {
                [Filter::Rnnoise(left), Filter::Rnnoise(right)] => right.copy_stream_from(left),
                [Filter::WebRtc(left), Filter::WebRtc(right)] => right.split_from(left),
                [Filter::DeepFilterNet3(left), Filter::DeepFilterNet3(right)] => {
                    right.copy_stream_from(left)
                }
                _ => unreachable!("linked channels always use the same filter"),
            }
            self.linked_stereo = false;
        }
        if !self.linked_stereo
            && let [Filter::DeepFilterNet3(left), Filter::DeepFilterNet3(right)] =
                self.filters.as_mut_slice()
        {
            let inputs = std::array::from_fn(|ch| std::array::from_fn(|i| frames[i][ch]));
            let mut outputs = [[0.; BLOCK]; 2];
            dfn3_rs::Denoiser::process_pair(left, right, &inputs, &mut outputs);
            for (i, frame) in frames.iter_mut().enumerate() {
                *frame = [outputs[0][i], outputs[1][i]];
            }
            return;
        }
        let mirror = self.linked_stereo || self.settings.mixing != Mixing::Stereo;
        for (channel, filter) in self
            .filters
            .iter_mut()
            .take(if mirror { 1 } else { 2 })
            .enumerate()
        {
            let mut output = [0.; BLOCK];
            match filter {
                Filter::Rnnoise(f) => {
                    let input = std::array::from_fn::<_, BLOCK, _>(|i| frames[i][channel] * 32768.);
                    f.process_frame(&mut output, &input);
                    for s in &mut output {
                        *s /= 32768.;
                    }
                }
                Filter::DeepFilterNet3(f) => {
                    let input = std::array::from_fn(|i| frames[i][channel]);
                    f.process(&input, &mut output);
                }
                Filter::WebRtc(f) => {
                    let input: [f32; BLOCK] = std::array::from_fn(|i| frames[i][channel]);
                    f.process(&input, &mut output);
                }
            }
            for (frame, sample) in frames.iter_mut().zip(output) {
                frame[channel] = sample;
                if mirror {
                    frame[1] = frame[0];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Settings;

    #[test]
    fn mono_devices_preserve_stereo_samples_and_meter_history_when_observation_resumes() {
        for method in [
            NoiseMethod::DeepFilterNet3,
            NoiseMethod::Rnnoise,
            NoiseMethod::WebRtc,
        ] {
            let settings = Settings {
                noise_method: method,
                low_cut_hz: 80,
                deep_filter_limit_db: 12,
                ..Default::default()
            };
            let mut mono = Processor::for_input(&settings, 1);
            let mut reference = Processor::new(&settings);
            assert_eq!(mono.filters.len(), 1);
            for index in 0..30 {
                let mut frames =
                    std::array::from_fn(|i| [((index * BLOCK + i) as f32 * 0.03).sin() * 0.08; 2]);
                let mut expected = frames;
                let (peak, history) = reference.process_with_levels(&mut expected);
                if index < 10 {
                    mono.process_unmetered(&mut frames);
                } else if index < 20 {
                    assert_eq!(mono.process(&mut frames), peak);
                } else {
                    let (output, levels) = mono.process_with_levels(&mut frames);
                    assert_eq!(output, peak);
                    assert_eq!(
                        (levels.before_power, levels.after_power),
                        (history.before_power, history.after_power)
                    );
                }
                assert_eq!(frames, expected);
            }
        }
    }

    #[test]
    fn identical_stereo_can_split_during_a_ramp_and_resume_independent_speech() {
        for split in [0_usize, 11, 20, 31, 45] {
            let settings = Settings {
                deep_filter_limit_db: 12,
                ..Settings::default()
            };
            let mut linked = Processor::new(&settings);
            let mut independent = Processor::new(&settings);
            independent.linked_stereo = false;
            for n in 0..64 {
                if n == split.saturating_sub(1_usize) {
                    linked.set_deep_filter_limit(35);
                    independent.set_deep_filter_limit(35);
                }
                let mut frame = std::array::from_fn(|i| {
                    let left = if (10..28).contains(&n) {
                        0.
                    } else {
                        ((n * BLOCK + i) as f32 * 0.055).sin() * 0.1
                    };
                    let right = if n == split && i == BLOCK - 1 {
                        left + 0.002
                    } else {
                        left
                    };
                    [left, right]
                });
                let mut expected = frame;
                independent.process_unmetered(&mut expected);
                linked.process_unmetered(&mut frame);
                assert_eq!(linked.linked_stereo, n < split);
                for (i, (actual, expected)) in frame.iter().zip(expected).enumerate() {
                    assert_eq!(*actual, expected, "split at {split}, block {n}, sample {i}");
                }
            }
        }
    }

    #[test]
    fn speech_filters_preserve_independent_channel_histories_on_split() {
        for method in [NoiseMethod::Rnnoise, NoiseMethod::WebRtc] {
            for split in [0, 47, 153] {
                let settings = Settings {
                    noise_method: method,
                    ..Settings::default()
                };
                let mut linked = Processor::new(&settings);
                let mut separate = Processor::new(&settings);
                separate.linked_stereo = false;
                for n in 0..split + 28 {
                    let mut block = std::array::from_fn(|i| {
                        let phase = (n * BLOCK + i) as f32;
                        let left = match n % 53 {
                            0..=12 => 0.,
                            13..=14 => {
                                if i % 7 == 0 {
                                    1.
                                } else {
                                    -1.
                                }
                            }
                            _ => (0.34 * phase).sin() * (0.007 * phase).cos() * 0.3,
                        };
                        let right = if n == split && i == BLOCK - 1 {
                            0.93
                        } else if n > split && n % 3 == 0 {
                            -left
                        } else {
                            left
                        };
                        [left, right]
                    });
                    let mut reference = block;
                    linked.process_unmetered(&mut block);
                    separate.process_unmetered(&mut reference);
                    for (i, (a, b)) in block.iter().zip(reference).enumerate() {
                        assert_eq!(
                            a.map(f32::to_bits),
                            b.map(f32::to_bits),
                            "{method:?}, split {split}, block {n}, sample {i}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rnnoise_preserves_silence_tails_and_resumed_speech() {
        let mut reference = nnnoiseless_reference::DenoiseState::new();
        let mut actual = DenoiseState::new();
        let mut right = DenoiseState::new();
        for block in 0..385 {
            let input: [f32; BLOCK] = std::array::from_fn(|i| match block {
                0..=10 | 31..=200 | 202..=350 => 0.0,
                201 => f32::from_bits(17),
                351..=360 => -0.0,
                _ => ((block * BLOCK + i) as f32 * 0.173).sin() * 32000.0,
            });
            if block == 194 {
                right.copy_stream_from(&actual);
            }
            let mut expected = [0.0; BLOCK];
            let expected_vad = reference.process_frame(&mut expected, &input).to_bits();
            let check = |channel: &mut DenoiseState<'_>| {
                let mut out = [0.0; BLOCK];
                assert_eq!(
                    channel.process_frame(&mut out, &input).to_bits(),
                    expected_vad
                );
                assert_eq!(
                    out.map(f32::to_bits),
                    expected.map(f32::to_bits),
                    "block {block}"
                );
            };
            check(&mut actual);
            if block >= 194 {
                check(&mut right);
            }
        }
    }

    #[test]
    fn maps_channels_then_applies_gain() {
        for (mixing, expected) in [(Mixing::Stereo, [1., -0.5]), (Mixing::Average, [0.5; 2])] {
            let mut frames = [[0.75, -0.25]; BLOCK];
            let mut processor = Processor::new(&Settings {
                mixing,
                gain_percent: 200,
                suppress_noise: false,
                ..Default::default()
            });
            processor.process(&mut frames);
            assert!(frames.iter().all(|frame| *frame == expected));
        }
    }

    #[test]
    fn bypass_matches_the_tone_processor_across_control_changes() {
        let neutral = crate::Tone::default();
        let active = crate::Tone {
            low_cut_hz: 80,
            gain_percent: 75,
            compressor_ratio: 3,
        };
        for mixing in [Mixing::Stereo, Mixing::Average] {
            let mut mixer = Processor::new(&Settings {
                mixing,
                suppress_noise: false,
                ..Settings::default()
            });
            let mut tone = crate::tone::ToneProcessor::new(neutral);
            for (n, controls) in [neutral, active, active, neutral, neutral, neutral, neutral]
                .into_iter()
                .enumerate()
            {
                mixer.set_tone(controls);
                tone.configure(controls);
                let mut actual = std::array::from_fn(|i| match i % 4 {
                    0 => [-0., f32::from_bits(11)],
                    1 => [f32::NAN, f32::INFINITY],
                    2 => [1.3, -0.45],
                    _ => [-1.7, 0.82],
                });
                let mut expected = actual.map(|frame| {
                    let [l, r] = frame.map(|s| if s.is_finite() { s.clamp(-1., 1.) } else { 0. });
                    if mixing == Mixing::Average {
                        [(l + r) * 0.5; 2]
                    } else {
                        [l, r]
                    }
                });
                let input = levels(&expected);
                let peak = tone.process(&mut expected);
                match n % 3 {
                    0 => assert_eq!(mixer.process(&mut actual), peak),
                    1 => mixer.process_unmetered(&mut actual),
                    _ => {
                        let (output, b) = mixer.process_with_levels(&mut actual);
                        assert_eq!(output, peak);
                        assert_eq!((b.before_peak, b.before_power), input);
                        assert_eq!((b.after_peak, b.after_power), input);
                    }
                }
                assert_eq!(
                    actual.map(|f| f.map(f32::to_bits)),
                    expected.map(|f| f.map(f32::to_bits))
                );
            }
        }
    }

    #[test]
    fn live_limit_changes_preserve_both_channels_and_the_model_history() {
        let mut processor = Processor::new(&Settings {
            deep_filter_limit_db: 12,
            ..Settings::default()
        });
        let mut reference: [Box<dfn3_rs::Denoiser>; 2] = std::array::from_fn(|_| {
            let mut d = Box::<dfn3_rs::Denoiser>::default();
            d.set_attenuation_limit(12.);
            d
        });
        for block in 0..36 {
            let limit = match block {
                10 => Some(0),
                18 => Some(12),
                26 => Some(Settings::UNLIMITED_ATTENUATION_DB),
                _ => None,
            };
            if let Some(db) = limit {
                processor.set_deep_filter_limit(db);
                for filter in &mut reference {
                    filter.set_attenuation_limit_smooth(
                        f32::from(db),
                        std::time::Duration::from_millis(20),
                    );
                }
            }
            let mut frames = std::array::from_fn(|i| {
                let phase = (block * BLOCK + i) as f32 * 0.04;
                [0.08 * phase.sin(), 0.02 * (phase * 1.5).cos()]
            });
            let input = frames;
            processor.process(&mut frames);
            for (ch, filter) in reference.iter_mut().enumerate() {
                let mut expected = [0.; BLOCK];
                filter.process(&input.map(|f| f[ch]), &mut expected);
                assert_eq!(frames.map(|f| f[ch]), expected);
            }
        }
    }

    #[test]
    fn suppression_reduces_stationary_noise() {
        for method in [
            NoiseMethod::Rnnoise,
            NoiseMethod::WebRtc,
            NoiseMethod::DeepFilterNet3,
        ] {
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
    fn webrtc_preserves_a_tone_across_mono_outputs() {
        let mut p = Processor::new(&Settings {
            suppress_noise: true,
            noise_method: NoiseMethod::WebRtc,
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
        assert!(levels.after_power > levels.before_power * 0.1);
        assert!(next.iter().all(|[left, right]| left == right));
    }

    #[test]
    fn output_gain_does_not_change_the_filter_measurement() {
        let mut p = Processor::new(&Settings {
            suppress_noise: false,
            ..Default::default()
        });
        for gain in [0, 25, 100, 200] {
            p.set_gain(gain);
            p.process(&mut [[0.25, -0.25]; BLOCK]); // Allow one block for the gain ramp.
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
    fn invalid_input_is_silent() {
        let mut processor = Processor::new(&Settings {
            suppress_noise: true,
            ..Default::default()
        });
        let mut frames = [[f32::NAN, f32::INFINITY]; BLOCK];
        assert_eq!(processor.process(&mut frames), 0.);
        assert_eq!(frames, [[0.; 2]; BLOCK]);
    }
}
