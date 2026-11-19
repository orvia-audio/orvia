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

fn choose(
    blocks: &[Energy],
    room_range: Range<usize>,
    phrases: [Range<usize>; 2],
) -> Result<Recommendation, SampleError> {
    let room_blocks = &blocks[room_range];
    if room_blocks.is_empty() {
        return Err(SampleError::Incomplete);
    }
    let room = Energy::sum(room_blocks);
    let mut noise: Vec<_> = room_blocks.iter().map(|b| b.dry).collect();
    noise.sort_unstable_by(f64::total_cmp);
    // Demand a sustained rise above typical room fluctuations in both prompted phrases.
    let floor = (noise[noise.len() * 9 / 10] * 4.).max(1e-8);
    let mut onsets = Vec::with_capacity(6);
    for phrase in phrases {
        let speech = &blocks[phrase];
        if speech.len() < 20 {
            return Err(SampleError::UnclearSpeech);
        }
        let mut levels: Vec<_> = speech.iter().map(|b| b.dry).collect();
        levels.sort_unstable_by(f64::total_cmp);
        let threshold = floor.max(levels[levels.len() * 9 / 10] * 0.025);
        if speech.iter().filter(|b| b.dry > threshold).count() < 20 {
            return Err(SampleError::UnclearSpeech);
        }
        let first = speech
            .windows(4)
            .position(|w| {
                w[0].dry > threshold && w.iter().filter(|b| b.dry > threshold).count() >= 3
            })
            .ok_or(SampleError::UnclearSpeech)?;
        let onset = speech
            .get(first..first + 15)
            .ok_or(SampleError::UnclearSpeech)?;
        let previous = onsets.len();
        onsets.extend(
            onset
                .chunks_exact(5)
                .map(Energy::sum)
                .filter(|b| b.dry > 5. * floor),
        );
        if onsets.len() == previous {
            return Err(SampleError::UnclearSpeech);
        }
    }
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
    Ok(candidates
        .iter()
        .copied()
        .find(|r| r.room_reduction_db >= target)
        .unwrap_or(candidates[0]))
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
    let mut input = Vec::new();
    let mut append = |samples: &[[f32; 2]]| {
        let start = input.len() / BLOCK;
        input.extend_from_slice(&samples[..samples.len() / BLOCK * BLOCK]);
        start..input.len() / BLOCK
    };
    let room_range = append(room);
    let first = append(phrases[0]);
    append(room);
    let second = append(phrases[1]);
    let block_count = input.len() / BLOCK;
    if input.iter().flatten().any(|x| !x.is_finite()) {
        return Err(SampleError::Incomplete);
    }
    if input
        .iter()
        .filter(|f| f.iter().any(|x| x.abs() >= 0.999))
        .count()
        > SAMPLE_RATE as usize / 100
    {
        return Err(SampleError::Clipped);
    }
    let dry: Vec<_> = input
        .iter()
        .map(|&f| match mixing {
            Mixing::Average => [(f[0] + f[1]) * 0.5; 2],
            Mixing::Stereo => f,
        })
        .collect();
    let mut wet = vec![[0.; 2]; dry.len()];
    let channels = if mixing == Mixing::Stereo { 2 } else { 1 };
    let delay = dfn3_rs::ALGORITHM_DELAY / BLOCK;
    for ch in 0..channels {
        let mut filter = dfn3_rs::Denoiser::new();
        for i in 0..block_count + delay {
            let mut enhanced = [0.; BLOCK];
            let frame = std::array::from_fn(|j| dry.get(i * BLOCK + j).map_or(0., |f| f[ch]));
            filter.process(&frame, &mut enhanced);
            if let Some(output) = i.checked_sub(delay) {
                for (f, s) in wet[output * BLOCK..][..BLOCK].iter_mut().zip(enhanced) {
                    f[ch] = s;
                    if channels == 1 {
                        f[1] = s;
                    }
                }
            }
        }
    }
    let blocks: Vec<_> = dry
        .chunks_exact(BLOCK)
        .zip(wet.chunks_exact(BLOCK))
        .map(|(d, w)| {
            let mut e = Energy::default();
            for (&a, &b) in d.iter().flatten().zip(w.iter().flatten()) {
                let (a, b) = (f64::from(a), f64::from(b));
                e.dry += a * a;
                e.wet += b * b;
                e.cross += a * b;
            }
            let n = (2 * BLOCK) as f64;
            Energy {
                dry: e.dry / n,
                wet: e.wet / n,
                cross: e.cross / n,
            }
        })
        .collect();
    choose(&blocks, room_range, [first, second])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_attenuation_is_suitable_only_when_both_onsets_survive() {
        let speech = [200..500, 700..1000];
        let choose = |blocks: &[Energy]| super::choose(blocks, 30..170, speech.clone());
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
