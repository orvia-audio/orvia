//! Voice tone controls shared by the live mixer and the microphone setup preview.
use crate::{
    BLOCK, SAMPLE_RATE,
    calibration::{GuideAudio, SampleError, check_cancelled},
};
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
    /// Clear a decayed feedback pair before it can stay subnormal throughout a silent input.
    /// This floor is hundreds of orders of magnitude below the smallest representable PCM sample.
    fn settle(&mut self) {
        const FLOOR: f64 = f64::MIN_POSITIVE * 4096.;
        for pair in &mut self.state {
            if pair[0].abs() < FLOOR && pair[1].abs() < FLOOR {
                *pair = [0.; 2];
            }
        }
    }

    fn frame<const MONO: bool>(&mut self, samples: [f32; 2]) -> [f32; 2] {
        let step = |sample, state: &mut [f64; 2]| {
            let x = f64::from(sample) * self.b;
            let y = x + state[0];
            state[0] = state[1] - 2. * x - self.a1 * y;
            state[1] = x - self.a2 * y;
            y as f32
        };
        let left = step(samples[0], &mut self.state[0]);
        if MONO {
            [left; 2]
        } else {
            [left, step(samples[1], &mut self.state[1])]
        }
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

    /// Whether bounded input will pass through without advancing any gain or filter transition.
    pub(crate) fn is_bypassed(&self) -> bool {
        self.settings == Tone::default()
            && self.previous_filter.is_none()
            && self.gain == 1.
            && self.reduction == 1.
    }

    /// Apply one block of at most 480 normalized stereo frames and return the final peak.
    /// A settled compressor passes through blocks at or below its threshold.
    pub fn process(&mut self, frames: &mut [[f32; 2]]) -> f32 {
        self.process_block::<true>(frames)
    }

    /// Apply up to 480 normalized frames without measuring the playback peak.
    pub fn process_unmetered(&mut self, frames: &mut [[f32; 2]]) {
        self.process_block::<false>(frames);
    }

    pub(crate) fn settings(&self) -> Tone {
        self.settings
    }

    pub(crate) fn process_block<const OUTPUT_PEAK: bool>(
        &mut self,
        frames: &mut [[f32; 2]],
    ) -> f32 {
        self.process_channels::<OUTPUT_PEAK, false>(frames)
    }

    /// The caller selects mono only on routes whose channels are identical for their entire lifetime.
    pub(crate) fn process_channels<const OUTPUT_PEAK: bool, const MONO: bool>(
        &mut self,
        frames: &mut [[f32; 2]],
    ) -> f32 {
        let gain = f32::from(self.settings.gain_percent) / 100.;
        if self.previous_filter.is_none()
            && self.settings.compressor_ratio == 1
            && self.reduction == 1.
            && gain == self.gain
        {
            if self.settings.low_cut_hz != 0 {
                if self.highpass.state == [[0.; 2]; 2] && frames.iter().flatten().all(|&s| s == 0.)
                {
                    return 0.;
                }
                let mut peak = 0_f32;
                for frame in frames {
                    *frame = self
                        .highpass
                        .frame::<MONO>(*frame)
                        .map(|s| (s * gain).clamp(-1., 1.));
                    if OUTPUT_PEAK {
                        peak = peak.max(frame[0].abs()).max(frame[1].abs());
                    }
                }
                self.highpass.settle();
                return peak;
            }
            let mut peak = 0_f32;
            for s in frames.iter_mut().flatten() {
                *s = (*s * gain).clamp(-1., 1.);
                if OUTPUT_PEAK {
                    peak = peak.max(s.abs());
                }
            }
            return peak;
        }
        let peak = self.prepare_compressor_input::<MONO>(frames, gain);
        let threshold = 0.25;
        // An earlier loud block may still be releasing even after the input falls below threshold.
        if peak <= threshold && self.reduction == 1. {
            return if OUTPUT_PEAK { peak } else { 0. };
        }
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
        let count = frames.len().max(1) as f32;
        for (i, sample) in frames.iter_mut().enumerate() {
            if self.settings.compressor_ratio == 1 {
                self.reduction =
                    previous_reduction + (1. - previous_reduction) * (i + 1) as f32 / count;
            } else {
                self.reduction += step * (target - self.reduction);
            }
            for s in sample {
                *s = (*s * self.reduction).clamp(-1., 1.);
                if OUTPUT_PEAK {
                    output = output.max(s.abs());
                }
            }
        }
        if (1. - self.reduction).abs() < 0.0003 && target == 1. {
            self.reduction = 1.;
        }
        output
    }

    /// Apply low-cut and gain transitions and measure the peak that drives both channels.
    fn prepare_compressor_input<const MONO: bool>(
        &mut self,
        frames: &mut [[f32; 2]],
        gain: f32,
    ) -> f32 {
        if self.previous_filter.is_none()
            && self.settings.low_cut_hz == 0
            && self.gain == 1.
            && gain == 1.
        {
            let mut left = 0_f32;
            let mut right = 0_f32;
            for &[l, r] in frames.iter() {
                left = left.max(l.abs());
                right = right.max(r.abs());
            }
            return left.max(right);
        }
        let mut peak = 0_f32;
        let count = frames.len().max(1) as f32;
        for (i, frame) in frames.iter_mut().enumerate() {
            let mix = (i + 1) as f32 / count;
            let dry = *frame;
            if self.settings.low_cut_hz != 0 {
                *frame = self.highpass.frame::<MONO>(dry);
            }
            if let Some(previous) = &mut self.previous_filter {
                let before = previous.frame::<MONO>(dry);
                for (s, old) in frame.iter_mut().zip(before) {
                    *s = old + (*s - old) * mix;
                }
            }
            for s in frame {
                *s *= self.gain + (gain - self.gain) * mix;
                peak = peak.max(s.abs());
            }
        }
        self.highpass.settle();
        self.previous_filter = None;
        self.gain = gain;
        peak
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

fn filtered_power(filter: &mut HighPass, frames: &[[f32; 2]]) -> f64 {
    frames
        .iter()
        .flat_map(|&frame| filter.frame::<false>(frame))
        .map(|s| f64::from(s).powi(2))
        .sum::<f64>()
        / (2 * frames.len()).max(1) as f64
}

/// Compare speech energy above the room floor in 100 ms windows. A steady rumble
/// can continue under the reading, so its removal should not count as lost voice.
fn preserves_speech(dry: &[f64], wet: &[f64]) -> bool {
    let tail_power = |levels: &[f64]| {
        let tail = &levels[levels.len().saturating_sub(30)..];
        tail.iter().sum::<f64>() / tail.len() as f64
    };
    let dry_floor = tail_power(dry);
    let wet_floor = tail_power(wet);
    let strongest = dry.iter().copied().fold(0., f64::max);
    let (mut observed, mut preserved) = (0, 0);
    for (before, after) in dry.chunks(10).zip(wet.chunks(10)) {
        let count = before.len() as f64;
        let before = before.iter().sum::<f64>() / count - dry_floor;
        if before <= dry_floor.max(strongest * 0.025).max(1e-8) {
            continue;
        }
        let after = after.iter().sum::<f64>() / count - wet_floor;
        observed += 1;
        if 10. * (before / after.max(1e-15)).log10() <= 1. {
            preserved += 1;
        }
    }
    // The zero-based 90th percentile is within the limit iff enough windows pass it.
    preserved > observed * 9 / 10
}

/// Recommend tone controls from separate virtual-mic noise and speech takes at 48 kHz.
/// The reading should end with at least 300 ms of room sound.
/// Chooses the lowest effective cutoff that preserves speech, then estimates gain and peak control.
///
/// # Errors
///
/// Returns a sample error for a short, non-finite, clipped, or inaudible recording.
pub fn recommend(room: &[[f32; 2]], voice: &[[f32; 2]]) -> Result<Tone, SampleError> {
    recommend_from(room, voice, &|| false)
}

/// Estimate the tone from a seekable reading, checking for cancellation between blocks.
///
/// # Errors
///
/// Returns a sample error for cancelled, unreadable, clipped, or unusable recordings.
pub fn recommend_from<S: GuideAudio + ?Sized>(
    room: &[[f32; 2]],
    voice: &S,
    cancelled: &dyn Fn() -> bool,
) -> Result<Tone, SampleError> {
    let len = voice.frame_count();
    if room.len() < BLOCK * 50 || len < BLOCK * 20 {
        return Err(SampleError::Incomplete);
    }
    let mut room_sum = 0.;
    for block in room.chunks(BLOCK) {
        check_cancelled(cancelled)?;
        for &s in block.iter().flatten() {
            if !s.is_finite() {
                return Err(SampleError::Incomplete);
            }
            room_sum += f64::from(s).powi(2);
        }
    }
    let noise = room_sum / (2 * room.len()) as f64;
    let read = |i: usize, storage: &mut [[f32; 2]; BLOCK]| {
        check_cancelled(cancelled)?;
        let start = i * BLOCK;
        let count = (len - start).min(BLOCK);
        voice.read_frames(start, &mut storage[..count])?;
        Ok::<usize, SampleError>(count)
    };
    let mut storage = [[0.; 2]; BLOCK];
    let mut levels = Vec::with_capacity(len / BLOCK);
    let mut clipped = 0;
    let mut peak_power = 0_f64;
    for i in 0..len.div_ceil(BLOCK) {
        let n = read(i, &mut storage)?;
        let mut sum = 0.;
        for &[l, r] in &storage[..n] {
            if !l.is_finite() || !r.is_finite() {
                return Err(SampleError::Incomplete);
            }
            if l.abs() >= 0.999 || r.abs() >= 0.999 {
                clipped += 1;
            }
            sum += f64::from(l).powi(2);
            sum += f64::from(r).powi(2);
        }
        if n == BLOCK {
            let power = sum / (2 * BLOCK) as f64;
            peak_power = peak_power.max(power);
            levels.push(power);
        }
    }
    if clipped > BLOCK {
        return Err(SampleError::Clipped);
    }
    // The reading includes quiet lead-in and trailing silence from the endpoint detector.
    let mut quiet: Vec<_> = levels.iter().rev().take(30).copied().collect();
    quiet.sort_unstable_by(f64::total_cmp);
    let speech_floor = (quiet[quiet.len() / 4] * 2.)
        .max(peak_power * 0.001)
        .max(1e-8);
    let active_count = levels.iter().filter(|&&p| p > speech_floor).count();
    if active_count < 15 {
        return Err(SampleError::UnclearSpeech);
    }
    let mut hz = 0;
    let mut best = 0.;
    let mut selected_levels = None;
    if noise > 1e-8 {
        for cutoff in [50, 65, 80, 100, 125, 150] {
            let room_power = filtered_power(&mut HighPass::new(cutoff), room);
            let reduction = 10. * (noise / room_power.max(1e-15)).log10();
            if reduction < 3. || reduction <= best + 0.5 {
                continue;
            }
            let mut filter = HighPass::new(cutoff);
            let mut filtered_levels = Vec::with_capacity(levels.len());
            for i in 0..levels.len() {
                read(i, &mut storage)?;
                filtered_levels.push(filtered_power(&mut filter, &storage));
            }
            if preserves_speech(&levels, &filtered_levels) {
                selected_levels = Some(filtered_levels);
                hz = cutoff;
                best = reduction;
                if reduction >= 6. {
                    break;
                }
            }
        }
    }
    // Aim for ordinary speech near -20 dBFS RMS, with at least 1 dB of headroom.
    // Only the median survives into the compressor trials.
    let median_power = {
        let filtered = selected_levels.as_deref().unwrap_or(&levels);
        let mut speech = Vec::with_capacity(active_count);
        speech.extend(
            levels
                .iter()
                .zip(filtered)
                .filter_map(|(&dry, &wet)| (dry > speech_floor).then_some(wet)),
        );
        let rank = speech.len() / 2;
        *speech.select_nth_unstable_by(rank, f64::total_cmp).1
    };
    drop(selected_levels);
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
        let mut preview = ToneProcessor::new(tone);
        let mut too_loud = 0;
        let allowed_loud = active_count - (active_count * 9 / 10 + 1);
        let mut acceptable = true;
        for i in 0..len.div_ceil(BLOCK) {
            let n = read(i, &mut storage)?;
            let block = &mut storage[..n];
            if preview.process(block) > 0.89 {
                acceptable = false;
                break;
            }
            if ratio < 4
                && levels.get(i).is_some_and(|&p| p > speech_floor)
                && power(block) > loud_power_limit
            {
                too_loud += 1;
                if too_loud > allowed_loud {
                    acceptable = false;
                    break;
                }
            }
        }
        if acceptable {
            return Ok(tone);
        }
    }
    let mut filter = HighPass::new(hz);
    let mut peak = 0_f32;
    for i in 0..len.div_ceil(BLOCK) {
        let n = read(i, &mut storage)?;
        for &frame in &storage[..n] {
            let samples = if hz == 0 {
                frame
            } else {
                filter.frame::<false>(frame)
            };
            peak = peak.max(samples[0].abs()).max(samples[1].abs());
        }
    }
    tone.gain_percent = tone.gain_percent.min((89. / peak.max(0.01)).floor() as u16);
    Ok(tone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_filter_matches_stereo_across_control_changes() {
        let mut mono = ToneProcessor::new(Tone::default());
        let mut stereo = ToneProcessor::new(Tone::default());
        for block in 0..240 {
            let tone = Tone {
                low_cut_hz: [0, 50, 125, 0][block / 13 % 4],
                gain_percent: [100, 150, 70][block / 19 % 3],
                compressor_ratio: [1, 3][block / 23 % 2],
            };
            mono.configure(tone);
            stereo.configure(tone);
            let mut a = std::array::from_fn::<_, BLOCK, _>(|i| {
                [((block * BLOCK + i) as f32 * 0.015).sin() * 0.45; 2]
            });
            let mut b = a;
            assert_eq!(
                mono.process_channels::<true, true>(&mut a).to_bits(),
                stereo.process(&mut b).to_bits()
            );
            assert!(
                a.iter()
                    .flatten()
                    .zip(b.iter().flatten())
                    .all(|(l, r)| l.to_bits() == r.to_bits())
            );
        }
    }

    #[test]
    fn quiet_blocks_pass_through_only_after_compressor_release() {
        let mut processor = ToneProcessor::new(Tone {
            compressor_ratio: 2,
            ..Tone::default()
        });
        let quiet = [[-0.0, -0.25]; BLOCK];
        let mut block = quiet;
        assert_eq!(processor.process(&mut block), 0.25);
        assert_eq!(
            block.map(|f| f.map(f32::to_bits)),
            quiet.map(|f| f.map(f32::to_bits))
        );

        let mut loud = [[0.125, 1.0]; BLOCK];
        processor.process_block::<false>(&mut loud);
        assert!(loud[BLOCK - 1][0] < 0.125);
        block = quiet;
        assert!(processor.process(&mut block) < 0.25);
        assert!(block[BLOCK - 1][1].abs() > block[0][1].abs());

        for _ in 0..160 {
            block = quiet;
            processor.process_block::<false>(&mut block);
        }
        assert_eq!(
            block.map(|f| f.map(f32::to_bits)),
            quiet.map(|f| f.map(f32::to_bits))
        );
    }

    #[test]
    fn silent_high_pass_feedback_settles_without_changing_pcm_on_resume() {
        for hz in [1, 50, 150] {
            let mut reference = HighPass::new(hz);
            let mut live = HighPass::new(hz);
            // Both channels have decayed well below f32 PCM. One still receives quiet input.
            reference.state = [[1e-307, -2e-307], [1e-6, -2e-6]];
            live.state = reference.state;
            live.settle();
            assert_eq!(live.state[0], [0.; 2]);
            assert_eq!(live.state[1], reference.state[1]);
            for i in 0..2 * BLOCK {
                let input = [if i >= BLOCK { 1e-15 } else { 0. }, -0.002];
                assert_eq!(reference.frame::<false>(input), live.frame::<false>(input));
            }
        }
    }

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
        assert!(
            filtered_power(&mut HighPass::new(tone.low_cut_hz), &rumble) < 0.5 * power(&rumble)
        );
        assert!(
            filtered_power(&mut HighPass::new(tone.low_cut_hz), &speech) > 0.79 * power(&speech)
        );
        assert!((135..=155).contains(&tone.gain_percent));
        assert_eq!(tone.compressor_ratio, 1);
        assert_eq!(recommend(&rumble, &phrase(52.)).unwrap().low_cut_hz, 0);
    }

    #[test]
    fn distinguishes_steady_rumble_under_the_reading_from_the_voice() {
        let rumble = room(30.);
        let mut voice = phrase(180.);
        for (i, frame) in voice.iter_mut().enumerate() {
            for sample in frame {
                *sample += sine(30., 0.07, i);
            }
        }
        let tone = recommend(&rumble, &voice).unwrap();
        assert!((50..=100).contains(&tone.low_cut_hz), "{tone:?}");
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
