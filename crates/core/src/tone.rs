//! Voice tone controls shared by the live mixer and the microphone setup preview.
use crate::{BLOCK, SAMPLE_RATE, calibration::SampleError};
use serde::{Deserialize, Serialize};

/// Output processing applied after the selected noise filter. Defaults to unity bypass.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Tone {
    /// High-pass cutoff in Hz, from 0 (bypass) to 150.
    pub low_cut_hz: u16,
    /// Linear level in percent, from 0 to 200.
    pub gain_percent: u16,
    /// Compression ratio above the fixed threshold. One bypasses compression.
    pub compressor_ratio: u16,
}

impl Default for Tone {
    fn default() -> Self {
        Self {
            low_cut_hz: 0,
            gain_percent: 100,
            compressor_ratio: 1,
        }
    }
}

impl Tone {
    /// Return settings within the live processor's supported ranges.
    pub fn bounded(self) -> Self {
        Self {
            low_cut_hz: self.low_cut_hz.min(150),
            gain_percent: self.gain_percent.min(200),
            compressor_ratio: self.compressor_ratio.clamp(1, 4),
        }
    }
}

/// Two-pole Butterworth high-pass with independent stereo histories at 48 kHz.
struct HighPass {
    b: f64,
    a1: f64,
    a2: f64,
    state: [[f64; 2]; 2],
}

impl HighPass {
    fn new(hz: u16) -> Self {
        let k = (std::f64::consts::PI * f64::from(hz) / f64::from(SAMPLE_RATE)).tan();
        let q = std::f64::consts::SQRT_2 * k;
        let b = 1. / (1. + q + k * k);
        Self {
            b,
            a1: 2. * (k * k - 1.) * b,
            a2: (1. - q + k * k) * b,
            state: [[0.; 2]; 2],
        }
    }
    fn frame(&mut self, mut samples: [f32; 2]) -> [f32; 2] {
        for (sample, state) in samples.iter_mut().zip(&mut self.state) {
            let x = f64::from(*sample) * self.b;
            let y = x + state[0];
            state[0] = state[1] - 2. * x - self.a1 * y;
            state[1] = x - self.a2 * y;
            *sample = y as f32;
        }
        samples
    }
}

/// Stereo-linked tone processing at 48 kHz. Uses the engine's existing block and no look-ahead buffer.
pub struct ToneProcessor {
    settings: Tone,
    highpass: HighPass,
    previous_filter: Option<HighPass>,
    gain: f32,
    reduction: f32,
}

impl ToneProcessor {
    /// Start with settled output gain and an empty filter history.
    pub fn new(settings: Tone) -> Self {
        let settings = settings.bounded();
        Self {
            settings,
            highpass: HighPass::new(settings.low_cut_hz),
            previous_filter: None,
            gain: f32::from(settings.gain_percent) / 100.,
            reduction: 1.,
        }
    }
    /// Change controls between blocks without interrupting the denoiser or audio streams.
    pub fn configure(&mut self, settings: Tone) {
        let settings = settings.bounded();
        if settings.low_cut_hz != self.settings.low_cut_hz {
            self.previous_filter = Some(std::mem::replace(
                &mut self.highpass,
                HighPass::new(settings.low_cut_hz),
            ));
        }
        self.settings = settings;
    }
    /// Apply one block of at most 480 normalized stereo frames and return the final peak.
    pub fn process(&mut self, frames: &mut [[f32; 2]]) -> f32 {
        let count = frames.len().max(1) as f32;
        let gain = f32::from(self.settings.gain_percent) / 100.;
        if self.settings.low_cut_hz == 0
            && self.previous_filter.is_none()
            && self.settings.compressor_ratio == 1
            && self.reduction == 1.
            && gain == self.gain
        {
            let mut peak = 0_f32;
            for s in frames.iter_mut().flatten() {
                *s = (*s * gain).clamp(-1., 1.);
                peak = peak.max(s.abs());
            }
            return peak;
        }
        let mut peak = 0_f32;
        for (i, frame) in frames.iter_mut().enumerate() {
            let mix = (i + 1) as f32 / count;
            let dry = *frame;
            if self.settings.low_cut_hz != 0 {
                *frame = self.highpass.frame(dry);
            }
            if let Some(previous) = &mut self.previous_filter {
                let before = previous.frame(dry);
                for (s, old) in frame.iter_mut().zip(before) {
                    *s = old + (*s - old) * mix;
                }
            }
            for s in frame {
                *s *= self.gain + (gain - self.gain) * mix;
                peak = peak.max(s.abs());
            }
        }
        self.previous_filter = None;
        self.gain = gain;
        let threshold = 0.25;
        let target = if self.settings.compressor_ratio > 1 && peak > threshold {
            (peak / threshold).powf(1. / f32::from(self.settings.compressor_ratio) - 1.)
        } else {
            1.
        };
        // A block peak drives both channels. Release takes about 150 ms at the engine block size.
        let step = if target < self.reduction {
            0.0138
        } else {
            0.000139
        };
        let mut output = 0_f32;
        let previous_reduction = self.reduction;
        for (i, sample) in frames.iter_mut().enumerate() {
            if self.settings.compressor_ratio == 1 {
                self.reduction =
                    previous_reduction + (1. - previous_reduction) * (i + 1) as f32 / count;
            } else {
                self.reduction += step * (target - self.reduction);
            }
            for s in sample {
                *s = (*s * self.reduction).clamp(-1., 1.);
                output = output.max(s.abs());
            }
        }
        if (1. - self.reduction).abs() < 0.0003 && target == 1. {
            self.reduction = 1.;
        }
        output
    }
}

fn power(frames: &[[f32; 2]]) -> f64 {
    frames
        .iter()
        .flatten()
        .map(|&s| f64::from(s).powi(2))
        .sum::<f64>()
        / (2 * frames.len()).max(1) as f64
}

fn filtered(frames: &[[f32; 2]], hz: u16) -> Vec<[f32; 2]> {
    let mut filter = HighPass::new(hz);
    frames.iter().map(|&f| filter.frame(f)).collect()
}

/// Recommend tone controls from separate virtual-mic noise and speech takes at 48 kHz.
/// Chooses the lowest effective cutoff that preserves speech, then estimates gain and peak control.
///
/// # Errors
///
/// Returns a sample error for a short, non-finite, clipped, or inaudible recording.
pub fn recommend(room: &[[f32; 2]], voice: &[[f32; 2]]) -> Result<Tone, SampleError> {
    if room.len() < BLOCK * 50
        || voice.len() < BLOCK * 20
        || room.iter().chain(voice).flatten().any(|s| !s.is_finite())
    {
        return Err(SampleError::Incomplete);
    }
    if voice
        .iter()
        .filter(|f| f.iter().any(|v| v.abs() >= 0.999))
        .count()
        > BLOCK
    {
        return Err(SampleError::Clipped);
    }
    let levels: Vec<_> = voice.chunks_exact(BLOCK).map(power).collect();
    let peak_power = levels.iter().copied().fold(0., f64::max);
    // The reading includes quiet lead-in and trailing silence from the endpoint detector.
    let mut sorted = levels.clone();
    sorted.sort_unstable_by(f64::total_cmp);
    let speech_floor = (sorted[sorted.len() / 10] * 2.)
        .max(peak_power * 0.001)
        .max(1e-8);
    let active: Vec<_> = levels
        .iter()
        .enumerate()
        .filter_map(|(i, &p)| (p > speech_floor).then_some(i))
        .collect();
    if active.len() < 15 {
        return Err(SampleError::UnclearSpeech);
    }
    let noise = power(room);
    let mut hz = 0;
    let mut best = 0.;
    if noise > 1e-8 {
        for cutoff in [50, 65, 80, 100, 125, 150] {
            let reduction = 10. * (noise / power(&filtered(room, cutoff)).max(1e-15)).log10();
            let candidate = filtered(voice, cutoff);
            let mut losses: Vec<_> = active
                .iter()
                .map(|&i| {
                    10. * (levels[i] / power(&candidate[i * BLOCK..(i + 1) * BLOCK]).max(1e-15))
                        .log10()
                })
                .collect();
            losses.sort_unstable_by(f64::total_cmp);
            if losses[losses.len() * 9 / 10] <= 1. && reduction >= 3. && reduction > best + 0.5 {
                hz = cutoff;
                best = reduction;
                if reduction >= 6. {
                    break;
                }
            }
        }
    }
    let voice = filtered(voice, hz);
    let mut speech: Vec<_> = active
        .iter()
        .map(|&i| power(&voice[i * BLOCK..(i + 1) * BLOCK]))
        .collect();
    speech.sort_unstable_by(f64::total_cmp);
    // Aim for ordinary speech near -20 dBFS RMS, with at least 1 dB of headroom.
    let median_power = speech[speech.len() / 2];
    let gain = (0.1 / median_power.sqrt().max(1e-6)).clamp(0.01, 2.);
    // Allow loud syllables 8 dB above ordinary speech. Below the compressor knee,
    // gain alone should preserve the speaker's dynamics.
    let loud_power_limit = (median_power * gain * gain * 10_f64.powf(0.8)).max(0.25_f64.powi(2));
    let mut tone = Tone {
        low_cut_hz: hz,
        gain_percent: (gain * 100.).round() as u16,
        compressor_ratio: 1,
    };
    for ratio in 1..=4 {
        tone.compressor_ratio = ratio;
        let mut preview = ToneProcessor::new(Tone {
            low_cut_hz: 0,
            ..tone
        });
        let mut peak = 0_f32;
        let mut output_powers = Vec::with_capacity(levels.len());
        for block in voice.chunks(BLOCK) {
            let mut buffer = [[0.; 2]; BLOCK];
            buffer[..block.len()].copy_from_slice(block);
            peak = peak.max(preview.process(&mut buffer[..block.len()]));
            output_powers.push(power(&buffer[..block.len()]));
        }
        let mut voiced: Vec<_> = active.iter().map(|&i| output_powers[i]).collect();
        voiced.sort_unstable_by(f64::total_cmp);
        if peak <= 0.89 && (voiced[voiced.len() * 9 / 10] <= loud_power_limit || ratio == 4) {
            return Ok(tone);
        }
    }
    let peak = voice.iter().flatten().fold(0_f32, |p, s| p.max(s.abs()));
    tone.gain_percent = tone.gain_percent.min((89. / peak.max(0.01)).floor() as u16);
    Ok(tone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amplitude: f32, i: usize) -> f32 {
        amplitude * (std::f32::consts::TAU * hz * i as f32 / SAMPLE_RATE as f32).sin()
    }

    fn room(hz: f32) -> Vec<[f32; 2]> {
        (0..BLOCK * 100).map(|i| [sine(hz, 0.035, i); 2]).collect()
    }

    fn phrase(hz: f32) -> Vec<[f32; 2]> {
        (0..BLOCK * 130)
            .map(|i| {
                let amplitude = if (20..95).contains(&(i / BLOCK)) {
                    0.1
                } else {
                    0.
                };
                [sine(hz, amplitude, i); 2]
            })
            .collect()
    }

    #[test]
    fn cuts_rumble_and_preserves_a_low_voice() {
        let rumble = room(30.);
        let speech = phrase(155.);
        let tone = recommend(&rumble, &speech).unwrap();
        assert!((50..=100).contains(&tone.low_cut_hz), "{tone:?}");
        assert!(power(&filtered(&rumble, tone.low_cut_hz)) < 0.5 * power(&rumble));
        assert!(power(&filtered(&speech, tone.low_cut_hz)) > 0.79 * power(&speech));
        assert!((135..=155).contains(&tone.gain_percent));
        assert_eq!(tone.compressor_ratio, 1);
        assert_eq!(recommend(&rumble, &phrase(52.)).unwrap().low_cut_hz, 0);
    }

    #[test]
    fn leaves_unrelated_noise_and_rejects_silence_and_clipping() {
        let noise = room(860.);
        assert_eq!(recommend(&noise, &phrase(180.)).unwrap().low_cut_hz, 0);
        assert_eq!(recommend(&noise, &noise), Err(SampleError::UnclearSpeech));
        assert_eq!(
            recommend(&noise, &vec![[0.; 2]; BLOCK * 100]),
            Err(SampleError::UnclearSpeech)
        );
        assert_eq!(
            recommend(&noise, &vec![[1.; 2]; BLOCK * 100]),
            Err(SampleError::Clipped)
        );
    }

    #[test]
    fn levels_loud_syllables_even_when_they_do_not_clip() {
        let mut speech = phrase(200.);
        for (i, frame) in speech
            .iter_mut()
            .enumerate()
            .skip(BLOCK * 55)
            .take(BLOCK * 22)
        {
            *frame = [sine(200., 0.48, i); 2];
        }
        // At the suggested gain the loud syllable stays below -1 dBFS even without compression.
        let tone = recommend(&vec![[0.; 2]; BLOCK * 60], &speech).unwrap();
        assert!(0.48 * f32::from(tone.gain_percent) / 100. < 0.89);
        assert!(tone.compressor_ratio > 1);
    }

    #[test]
    fn selects_compression_for_loud_syllables_and_preserves_stereo_balance() {
        let mut speech = phrase(200.);
        for (i, frame) in speech
            .iter_mut()
            .enumerate()
            .skip(BLOCK * 65)
            .take(BLOCK * 8)
        {
            let x = sine(200., 0.76, i);
            *frame = [x, x * 0.25];
        }
        let tone = recommend(&vec![[0.; 2]; BLOCK * 60], &speech).unwrap();
        assert!(tone.compressor_ratio > 1);
        let mut processor = ToneProcessor::new(tone);
        for block in speech.chunks_mut(BLOCK) {
            assert!(processor.process(block) <= 0.891);
        }
        assert!(
            speech[BLOCK * 65..BLOCK * 73]
                .iter()
                .all(|f| (f[0] * 0.25 - f[1]).abs() < 1e-6)
        );
    }
}
