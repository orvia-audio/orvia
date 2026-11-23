//! Estimate a DeepFilterNet3 limit from two spoken phrases and the room sound between them.
use crate::{BLOCK, Mixing, SAMPLE_RATE, Settings};
use std::ops::Range;

/// Estimated energy changes in the user's recorded test, before output gain.
#[derive(Clone, Copy, Debug)]
pub struct Recommendation {
    /// Suggested DeepFilterNet3 attenuation limit, in dB.
    pub limit_db: u16,
    /// Largest measured loss among the active 50 ms windows at the start of each phrase.
    pub onset_loss_db: f32,
    /// Reduction in the average energy of the room-only intervals, in dB.
    pub room_reduction_db: f32,
}

/// Reasons the guided sample cannot support a recommendation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleError {
    /// The full guide was not captured, or the samples were invalid.
    Incomplete,
    /// One or both speech intervals did not rise sufficiently above the room level.
    UnclearSpeech,
    /// A sustained part of the raw microphone recording reached full scale.
    Clipped,
}

/// Random-access stereo PCM for guided setup. Implementations can keep long readings on disk.
pub trait GuideAudio {
    /// Total number of normalized 48 kHz stereo frames in the completed recording.
    fn frame_count(&self) -> usize;

    /// Fill the destination starting at the given frame, preserving the captured sample bits.
    ///
    /// # Errors
    ///
    /// Returns a sample error if the requested frames cannot be read in full.
    fn read_frames(&self, start: usize, out: &mut [[f32; 2]]) -> Result<(), SampleError>;
}

impl GuideAudio for [[f32; 2]] {
    fn frame_count(&self) -> usize {
        self.len()
    }

    fn read_frames(&self, start: usize, out: &mut [[f32; 2]]) -> Result<(), SampleError> {
        let frames = self
            .get(start..)
            .and_then(|tail| tail.get(..out.len()))
            .ok_or(SampleError::Incomplete)?;
        out.copy_from_slice(frames);
        Ok(())
    }
}

pub(crate) fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), SampleError> {
    if cancelled() {
        Err(SampleError::Incomplete)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct Energy {
    dry: f64,
    wet: f64,
    cross: f64,
}

impl Energy {
    fn add(&mut self, block: Self) {
        self.dry += block.dry;
        self.wet += block.wet;
        self.cross += block.cross;
    }

    #[cfg(test)]
    fn sum(blocks: &[Self]) -> Self {
        blocks.iter().fold(Self::default(), |mut sum, &block| {
            sum.add(block);
            sum
        })
    }

    fn reduction(self, dry_mix: f64) -> f32 {
        let wet_mix = 1. - dry_mix;
        let after = dry_mix * dry_mix * self.dry
            + wet_mix * wet_mix * self.wet
            + 2. * dry_mix * wet_mix * self.cross;
        (10. * (self.dry.max(1e-16) / after.max(1e-16)).log10()) as f32
    }
}

fn dry_mix(limit: u16) -> f64 {
    if limit == Settings::UNLIMITED_ATTENUATION_DB {
        0.
    } else {
        10_f64.powf(-f64::from(limit) / 20.)
    }
}

/// Locate the active 50 ms windows using the input alone, before running the model.
fn onset_windows(
    blocks: &[f64],
    room_range: Range<usize>,
    phrases: [Range<usize>; 2],
) -> Result<Vec<Range<usize>>, SampleError> {
    fn percentile90(blocks: &[f64]) -> f64 {
        let mut powers: Vec<_> = blocks.to_vec();
        let rank = powers.len() * 9 / 10;
        *powers.select_nth_unstable_by(rank, f64::total_cmp).1
    }
    let room = &blocks[room_range];
    if room.is_empty() {
        return Err(SampleError::Incomplete);
    }
    let floor = (percentile90(room) * 4.).max(1e-8);
    let mut onsets = Vec::with_capacity(6);
    for phrase in phrases {
        let speech = &blocks[phrase.clone()];
        if speech.len() < 20 {
            return Err(SampleError::UnclearSpeech);
        }
        let threshold = floor.max(percentile90(speech) * 0.025);
        if speech.iter().filter(|b| **b > threshold).take(20).count() < 20 {
            return Err(SampleError::UnclearSpeech);
        }
        let first = speech
            .windows(4)
            .position(|w| w[0] > threshold && w.iter().filter(|b| **b > threshold).count() >= 3)
            .ok_or(SampleError::UnclearSpeech)?;
        if first + 15 > speech.len() {
            return Err(SampleError::UnclearSpeech);
        }
        let start = phrase.start + first;
        let previous = onsets.len();
        for i in (start..start + 15).step_by(5) {
            if blocks[i..i + 5].iter().sum::<f64>() > 5. * floor {
                onsets.push(i..i + 5);
            }
        }
        if onsets.len() == previous {
            return Err(SampleError::UnclearSpeech);
        }
    }
    Ok(onsets)
}

fn choose(room: Energy, onsets: &[Energy]) -> Recommendation {
    // Preserve each onset within 6 dB. Prefer the weakest limit that delivers
    // 9 dB of room reduction, or comes within half a dB of the best achievable result.
    let candidates: Vec<_> = (0..=Settings::UNLIMITED_ATTENUATION_DB)
        .map(|limit_db| {
            let mix = dry_mix(limit_db);
            Recommendation {
                limit_db,
                onset_loss_db: onsets.iter().map(|b| b.reduction(mix)).fold(0., f32::max),
                room_reduction_db: room.reduction(mix),
            }
        })
        .filter(|r| r.onset_loss_db <= 6.01)
        .collect();
    let best = candidates
        .iter()
        .map(|r| r.room_reduction_db)
        .fold(0., f32::max);
    let target = 9_f32.min(best - 0.5);
    // Zero dB always preserves the entire input.
    candidates
        .iter()
        .copied()
        .find(|r| r.room_reduction_db >= target)
        .unwrap_or(candidates[0])
}

/// Estimate a suppression limit from a complete 48 kHz stereo guide.
///
/// Run on a worker thread. Independent channels use separate filter histories in stereo.
/// Compensation for the model's look-ahead aligns raw and enhanced speech for comparison.
/// Output gain is applied after suppression and does not affect this estimate.
///
/// # Errors
///
/// Returns a [`SampleError`] if the input is incomplete, clipped or lacks two clear phrases.
pub fn recommend(
    room: &[[f32; 2]],
    phrases: [&[[f32; 2]]; 2],
    mixing: Mixing,
) -> Result<Recommendation, SampleError> {
    recommend_from(room, phrases, mixing, &|| false)
}

/// Estimate a suppression limit from seekable readings, checking for cancellation between blocks.
///
/// # Errors
///
/// Returns a sample error for unreadable, cancelled, clipped, or incomplete guides.
pub fn recommend_from<S: GuideAudio + ?Sized>(
    room: &[[f32; 2]],
    phrases: [&S; 2],
    mixing: Mixing,
    cancelled: &dyn Fn() -> bool,
) -> Result<Recommendation, SampleError> {
    let room_blocks = room.len() / BLOCK;
    let first = room_blocks..room_blocks + phrases[0].frame_count() / BLOCK;
    let start = first.end + room_blocks;
    let second = start..start + phrases[1].frame_count() / BLOCK;
    let total = second.end;
    let read = |i: usize, buf: &mut [[f32; 2]; BLOCK]| {
        check_cancelled(cancelled)?;
        if first.contains(&i) {
            phrases[0].read_frames((i - first.start) * BLOCK, buf)
        } else if second.contains(&i) {
            phrases[1].read_frames((i - second.start) * BLOCK, buf)
        } else {
            let room_index = if i < room_blocks { i } else { i - first.end };
            room.read_frames(room_index * BLOCK, buf)
        }
    };
    let channels = if mixing == Mixing::Stereo { 2 } else { 1 };
    let scale = (channels * BLOCK) as f64;
    let sample = |frame: &[f32; 2], ch: usize| match mixing {
        Mixing::Stereo => frame[ch],
        Mixing::Average => (frame[0] + frame[1]) * 0.5,
    };
    let mut powers = Vec::with_capacity(total);
    let mut clipped = 0;
    let mut first_difference = None;
    let mut block = [[0.; 2]; BLOCK];
    for i in 0..total {
        if (first.end..second.start).contains(&i) {
            // Reuse the room preflight; both occurrences still advance the model below.
            powers.push(powers[i - first.end]);
            continue;
        }
        read(i, &mut block)?;
        let mut power = 0.;
        for frame in &block {
            if !frame[0].is_finite() || !frame[1].is_finite() {
                return Err(SampleError::Incomplete);
            }
            if frame[0].abs() >= 0.999 || frame[1].abs() >= 0.999 {
                clipped += if i < room_blocks { 2 } else { 1 };
            }
            if frame[0].to_bits() != frame[1].to_bits() {
                first_difference.get_or_insert(i);
            }
            let x = f64::from(sample(frame, 0));
            power += x * x;
        }
        if channels == 2 {
            for frame in &block {
                let x = f64::from(frame[1]);
                power += x * x;
            }
        }
        powers.push(power / scale);
    }
    if clipped > SAMPLE_RATE as usize / 100 {
        return Err(SampleError::Clipped);
    }
    let windows = onset_windows(&powers, 0..room_blocks, [first.clone(), second.clone()])?;
    let last = windows.last().expect("each phrase has an onset").end;
    const DELAY: usize = dfn3_rs::ALGORITHM_DELAY / BLOCK;
    let independent = channels == 2 && first_difference.is_some_and(|i| i < last + DELAY);
    let mut left = dfn3_rs::Denoiser::new();
    let mut right = independent.then(|| Box::new(dfn3_rs::Denoiser::new()));
    let mut dry = [[[0.; 2]; BLOCK]; DELAY + 1];
    let mut room_energy = Energy::default();
    let mut onsets = vec![Energy::default(); windows.len()];
    for i in 0..last + DELAY {
        let block = &mut dry[i % (DELAY + 1)];
        if i < total {
            read(i, block)?;
        } else {
            check_cancelled(cancelled)?;
            block.fill([0.; 2]);
        }
        let mut input = [[0.; BLOCK]; 2];
        let mut output = [[0.; BLOCK]; 2];
        for (slot, frame) in input[0].iter_mut().zip(&*block) {
            *slot = sample(frame, 0);
        }
        if let Some(right) = &mut right {
            for (slot, frame) in input[1].iter_mut().zip(&*block) {
                *slot = frame[1];
            }
            dfn3_rs::Denoiser::process_pair(&mut left, right, &input, &mut output);
        } else {
            left.process(&input[0], &mut output[0]);
        }
        let Some(index) = i.checked_sub(DELAY) else {
            continue;
        };
        let target = if index < room_blocks {
            &mut room_energy
        } else if let Some(w) = windows.iter().position(|w| w.contains(&index)) {
            &mut onsets[w]
        } else {
            continue;
        };
        let mut energy = Energy {
            dry: powers[index],
            ..Energy::default()
        };
        for ch in 0..channels {
            let enhanced = &output[if independent { ch } else { 0 }];
            for (frame, &wet) in dry[index % (DELAY + 1)].iter().zip(enhanced) {
                let b = f64::from(wet);
                energy.wet += b * b;
                energy.cross += f64::from(sample(frame, ch)) * b;
            }
        }
        energy.wet /= scale;
        energy.cross /= scale;
        target.add(energy);
    }
    Ok(choose(room_energy, &onsets))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_guides_stop_before_reading_the_recording() {
        struct Unread;
        impl GuideAudio for Unread {
            fn frame_count(&self) -> usize {
                48_000
            }
            fn read_frames(&self, _: usize, _: &mut [[f32; 2]]) -> Result<(), SampleError> {
                panic!("cancelled recording was read")
            }
        }
        let room = vec![[0.; 2]; BLOCK * 50];
        assert_eq!(
            recommend_from(&room, [&Unread, &Unread], Mixing::Stereo, &|| true).unwrap_err(),
            SampleError::Incomplete
        );
        assert_eq!(
            crate::tone::recommend_from(&room, &Unread, &|| true).unwrap_err(),
            SampleError::Incomplete
        );
    }

    #[test]
    fn calibrates_duplicated_speech_equally_in_mono_and_stereo() {
        let room = vec![[0.0004; 2]; BLOCK * 50];
        let mut phrase: Vec<_> = (0..BLOCK * 40)
            .map(|i| [(i as f32 * 0.04).sin() * 0.12; 2])
            .collect();
        // Each prompt contributes only complete 10 ms blocks.
        phrase.push([f32::NAN; 2]);
        let mono = recommend(&room, [&phrase, &phrase], Mixing::Average).unwrap();
        let stereo = recommend(&room, [&phrase, &phrase], Mixing::Stereo).unwrap();
        assert_eq!(mono.limit_db, stereo.limit_db);
        assert!((mono.onset_loss_db - stereo.onset_loss_db).abs() < 0.001);
        assert!((mono.room_reduction_db - stereo.room_reduction_db).abs() < 0.001);
    }

    #[test]
    fn stereo_guide_matches_full_independent_inference_through_the_last_onset() {
        let room = vec![[0.0003, -0.0005]; BLOCK * 30];
        let mut phrases = [Vec::new(), Vec::new()];
        for (n, phrase) in phrases.iter_mut().enumerate() {
            phrase.extend((0..65 * BLOCK).map(|i| {
                if (9 + 25 * n..62).contains(&(i / BLOCK)) {
                    [
                        (i as f32 * 0.041).sin() * 0.12,
                        (i as f32 * 0.017).cos() * 0.09,
                    ]
                } else {
                    [0.0002, -0.0006]
                }
            }));
        }
        let actual = recommend(&room, [&phrases[0], &phrases[1]], Mixing::Stereo).unwrap();
        let guide = [&room[..], &phrases[0], &room, &phrases[1]].concat();
        let mut reference = vec![Energy::default(); guide.len() / BLOCK];
        let delay = dfn3_rs::ALGORITHM_DELAY / BLOCK;
        for channel in 0..2 {
            let mut filter = dfn3_rs::Denoiser::new();
            for i in 0..reference.len() + delay {
                let input =
                    std::array::from_fn(|j| guide.get(i * BLOCK + j).map_or(0., |f| f[channel]));
                let mut output = [0.; BLOCK];
                filter.process(&input, &mut output);
                if let Some(k) = i.checked_sub(delay) {
                    let energy = &mut reference[k];
                    for (frame, wet) in guide[k * BLOCK..].iter().zip(output) {
                        let (a, b) = (f64::from(frame[channel]), f64::from(wet));
                        energy.dry += a * a;
                        energy.wet += b * b;
                        energy.cross += a * b;
                    }
                }
            }
        }
        for block in &mut reference {
            let scale = (2 * BLOCK) as f64;
            block.dry /= scale;
            block.wet /= scale;
            block.cross /= scale;
        }
        let onsets: Vec<_> = onset_windows(
            &reference.iter().map(|v| v.dry).collect::<Vec<_>>(),
            0..30,
            [30..95, 125..190],
        )
        .unwrap()
        .into_iter()
        .map(|r| Energy::sum(&reference[r]))
        .collect();
        let expected = choose(Energy::sum(&reference[..30]), &onsets);
        assert_eq!(actual.limit_db, expected.limit_db);
        assert_eq!(
            actual.onset_loss_db.to_bits(),
            expected.onset_loss_db.to_bits()
        );
        assert_eq!(
            actual.room_reduction_db.to_bits(),
            expected.room_reduction_db.to_bits()
        );

        // Samples after the last onset still participate in input validation.
        phrases[1].last_mut().unwrap()[1] = f32::NAN;
        assert_eq!(
            recommend(&room, [&phrases[0], &phrases[1]], Mixing::Stereo).unwrap_err(),
            SampleError::Incomplete
        );
    }

    #[test]
    fn strong_attenuation_is_suitable_only_when_both_onsets_survive() {
        let speech = [200..500, 700..1000];
        let choose = |blocks: &[Energy]| -> Result<Recommendation, SampleError> {
            let windows = onset_windows(
                &blocks.iter().map(|v| v.dry).collect::<Vec<_>>(),
                30..170,
                speech.clone(),
            )?;
            let onsets: Vec<_> = windows
                .into_iter()
                .map(|r| Energy::sum(&blocks[r]))
                .collect();
            Ok(super::choose(Energy::sum(&blocks[30..170]), &onsets))
        };
        let mut blocks = vec![
            Energy {
                dry: 0.00001,
                wet: 0.,
                cross: 0.
            };
            1000
        ];
        assert_eq!(choose(&blocks).unwrap_err(), SampleError::UnclearSpeech);
        for p in speech.clone() {
            blocks[p.start + 20..p.start + 85].fill(Energy {
                dry: 0.01,
                wet: 0.01,
                cross: 0.01,
            });
        }
        assert_eq!(choose(&blocks).unwrap().limit_db, 9);
        blocks[720..735].fill(Energy {
            dry: 0.01,
            wet: 0.,
            cross: 0.,
        });
        let limited = choose(&blocks).unwrap();
        assert_eq!(limited.limit_db, 6);
        assert!((limited.onset_loss_db - 6.).abs() < 0.01);
        blocks[720..785].fill(Energy::default());
        assert_eq!(choose(&blocks).unwrap_err(), SampleError::UnclearSpeech);
    }
}
