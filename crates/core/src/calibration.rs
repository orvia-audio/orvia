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

#[derive(Clone, Copy, Default)]
struct Energy {
    dry: f64,
    wet: f64,
    cross: f64,
}

impl Energy {
    fn sum(blocks: &[Self]) -> Self {
        blocks.iter().fold(Self::default(), |a, b| Self {
            dry: a.dry + b.dry,
            wet: a.wet + b.wet,
            cross: a.cross + b.cross,
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
    blocks: &[Energy],
    room_range: Range<usize>,
    phrases: [Range<usize>; 2],
) -> Result<Vec<Range<usize>>, SampleError> {
    fn percentile90(blocks: &[Energy]) -> f64 {
        let mut powers: Vec<_> = blocks.iter().map(|b| b.dry).collect();
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
        if speech.iter().filter(|b| b.dry > threshold).take(20).count() < 20 {
            return Err(SampleError::UnclearSpeech);
        }
        let first = speech
            .windows(4)
            .position(|w| {
                w[0].dry > threshold && w.iter().filter(|b| b.dry > threshold).count() >= 3
            })
            .ok_or(SampleError::UnclearSpeech)?;
        if first + 15 > speech.len() {
            return Err(SampleError::UnclearSpeech);
        }
        let start = phrase.start + first;
        let previous = onsets.len();
        for i in (start..start + 15).step_by(5) {
            if Energy::sum(&blocks[i..i + 5]).dry > 5. * floor {
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
    // Round each boundary to complete model blocks and preserve a room interval before each onset.
    let sections = [room, phrases[0], room, phrases[1]];
    let chunks = || sections.iter().flat_map(|s| s.chunks_exact(BLOCK));
    let room_blocks = room.len() / BLOCK;
    let first = room_blocks..room_blocks + phrases[0].len() / BLOCK;
    let start = first.end + room_blocks;
    let second = start..start + phrases[1].len() / BLOCK;
    if chunks().flatten().flatten().any(|x| !x.is_finite()) {
        return Err(SampleError::Incomplete);
    }
    if chunks()
        .flatten()
        .filter(|f| f.iter().any(|x| x.abs() >= 0.999))
        .count()
        > SAMPLE_RATE as usize / 100
    {
        return Err(SampleError::Clipped);
    }
    let channels = if mixing == Mixing::Stereo { 2 } else { 1 };
    let scale = (channels * BLOCK) as f64;
    let sample = |frame: &[f32; 2], ch: usize| match mixing {
        Mixing::Stereo => frame[ch],
        Mixing::Average => (frame[0] + frame[1]) * 0.5,
    };
    let mut blocks: Vec<_> = chunks()
        .map(|chunk| {
            let mut dry = 0.;
            for ch in 0..channels {
                for frame in chunk {
                    let x = f64::from(sample(frame, ch));
                    dry += x * x;
                }
            }
            Energy {
                dry: dry / scale,
                ..Energy::default()
            }
        })
        .collect();
    let windows = onset_windows(&blocks, 0..room_blocks, [first, second])?;
    let last = windows.last().expect("each phrase has an onset").end;
    let delay = dfn3_rs::ALGORITHM_DELAY / BLOCK;
    let independent = channels == 2
        && chunks()
            .take(last + delay)
            .flatten()
            .any(|f| f[0].to_bits() != f[1].to_bits());
    let mut left = dfn3_rs::Denoiser::new();
    let mut right = independent.then(|| Box::new(dfn3_rs::Denoiser::new()));
    let mut aligned = chunks().zip(&mut blocks);
    // Keep all inference history through the last required window, including lookahead.
    for (i, chunk) in chunks()
        .chain(std::iter::repeat_n(&[][..], delay))
        .take(last + delay)
        .enumerate()
    {
        let mut input = [[0.; BLOCK]; 2];
        for (slot, frame) in input[0].iter_mut().zip(chunk) {
            *slot = sample(frame, 0);
        }
        let mut output = [[0.; BLOCK]; 2];
        if let Some(right) = &mut right {
            for (slot, frame) in input[1].iter_mut().zip(chunk) {
                *slot = frame[1];
            }
            dfn3_rs::Denoiser::process_pair(&mut left, right, &input, &mut output);
        } else {
            left.process(&input[0], &mut output[0]);
        }
        let Some(index) = i.checked_sub(delay) else {
            continue;
        };
        let (dry, energy) = aligned.next().expect("window lies within the guide");
        if index >= room_blocks && !windows.iter().any(|w| w.contains(&index)) {
            continue;
        }
        // Accumulate in channel order even when identical stereo shares a model.
        for ch in 0..channels {
            let enhanced = &output[if independent { ch } else { 0 }];
            for (frame, &wet) in dry.iter().zip(enhanced) {
                let b = f64::from(wet);
                energy.wet += b * b;
                energy.cross += f64::from(sample(frame, ch)) * b;
            }
        }
        energy.wet /= scale;
        energy.cross /= scale;
    }
    let onsets: Vec<_> = windows
        .into_iter()
        .map(|r| Energy::sum(&blocks[r]))
        .collect();
    Ok(choose(Energy::sum(&blocks[..room_blocks]), &onsets))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let onsets: Vec<_> = onset_windows(&reference, 0..30, [30..95, 125..190])
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
            let windows = onset_windows(blocks, 30..170, speech.clone())?;
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
