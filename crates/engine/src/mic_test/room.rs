//! Keep the two seconds around the loudest room sound for tone analysis and replay.
use super::{Clip, Frame, InputRate, Recording};
use anyhow::{Result, ensure};
use orvia_core::{BLOCK, SAMPLE_RATE};
use std::{collections::VecDeque, time::Duration};

const WINDOW: usize = 2 * SAMPLE_RATE as usize;

/// Select the same excerpt from an existing 48 kHz clip.
pub(super) fn loudest_excerpt(frames: &[Frame]) -> &[Frame] {
    if frames.len() <= WINDOW {
        return frames;
    }
    let loudest = frames
        .chunks(BLOCK)
        .enumerate()
        .map(|(i, block)| {
            (
                i * BLOCK,
                block.iter().flatten().map(|s| s * s).sum::<f32>(),
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("nonempty recording")
        .0;
    let start = loudest
        .saturating_sub(WINDOW / 2)
        .min(frames.len() - WINDOW);
    &frames[start..start + WINDOW]
}

/// Captures room noise for tone setup while retaining the strongest two-second excerpt.
/// Selection uses the same 48 kHz energy windows as offline tone analysis.
pub struct RoomRecording {
    capture: Recording,
    resampler: InputRate,
    excerpt: RoomExcerpt,
}

impl RoomRecording {
    /// Start recording the selected virtual microphone at its negotiated rate.
    ///
    /// # Errors
    ///
    /// Reports an unavailable device or unsupported capture format.
    pub fn start(id: &str) -> Result<Self> {
        let capture = Recording::start(id)?;
        Ok(Self {
            resampler: InputRate::new(capture.rate()),
            capture,
            excerpt: RoomExcerpt::default(),
        })
    }

    /// Duration of the full capture, including audio waiting in the device queue.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(
            self.excerpt.seen as f64 / f64::from(SAMPLE_RATE)
                + self.capture.rx.slots() as f64 / f64::from(self.capture.rate()),
        )
    }

    /// Evaluate all available frames and retain enough context to center the excerpt.
    ///
    /// # Errors
    ///
    /// Reports a device error or capture queue overflow.
    pub fn collect(&mut self) -> Result<()> {
        self.capture.drain(|frames| {
            self.excerpt.extend(frames, &mut self.resampler);
            Ok(())
        })
    }

    /// Stop recording and return the loudest excerpt as 48 kHz stereo PCM.
    ///
    /// # Errors
    ///
    /// Reports an empty take, device error or queue overflow.
    pub fn finish(mut self) -> Result<Clip> {
        let tail = self.capture.stop()?;
        self.excerpt.extend(&tail, &mut self.resampler);
        ensure!(
            self.excerpt.seen > 0,
            "The microphone returned no audio frames"
        );
        Ok(Clip {
            rate: SAMPLE_RATE,
            frames: self.excerpt.finish().into(),
        })
    }
}

#[derive(Default)]
struct RoomExcerpt {
    recent: VecDeque<Frame>,
    best: Vec<Frame>,
    seen: usize,
    energy: f32,
    highest: Option<f32>,
    // A newer maximum can supersede this window before its right half arrives.
    wanted_end: Option<usize>,
}

impl RoomExcerpt {
    fn extend(&mut self, frames: &[Frame], resampler: &mut InputRate) {
        for &frame in frames {
            resampler.push(frame, |sample| {
                if self.recent.len() == WINDOW {
                    self.recent.pop_front();
                }
                self.recent.push_back(sample);
                self.seen += 1;
                for value in sample {
                    self.energy += value * value;
                }
                if self.seen.is_multiple_of(BLOCK) {
                    self.compare(self.seen - BLOCK);
                }
                if self.wanted_end == Some(self.seen) {
                    self.best.clear();
                    self.best.extend(self.recent.iter().copied());
                    self.wanted_end = None;
                }
            });
        }
    }

    fn compare(&mut self, block_start: usize) {
        // max_by selects the last equally loud block.
        if self
            .highest
            .is_none_or(|old| !self.energy.total_cmp(&old).is_lt())
        {
            self.highest = Some(self.energy);
            self.wanted_end = Some(block_start.saturating_sub(WINDOW / 2) + WINDOW);
        }
        self.energy = 0.;
    }

    fn finish(mut self) -> Vec<Frame> {
        let partial = self.seen % BLOCK;
        if partial > 0 {
            self.compare(self.seen - partial);
        }
        if self.wanted_end.is_some() {
            self.recent.into()
        } else {
            self.best
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_excerpt_matches_offline_bounds_ties_and_rate_conversion() {
        for rate in [32_000, 44_100, 48_000] {
            for (seconds, bursts) in [
                (1, [0, 0]),
                (5, [0, 0]),
                (5, [2, 2]),
                (5, [0, 4]),
                (5, [1, 3]),
                (5, [5, 5]),
            ] {
                let input: Vec<_> = (0..seconds * rate + 211)
                    .map(|i| {
                        let background = ((i % 53) as f32 - 26.) * 0.00001;
                        [
                            if bursts
                                .iter()
                                .any(|s| (s * rate..s * rate + 450).contains(&i))
                            {
                                0.42
                            } else {
                                background
                            },
                            background,
                        ]
                    })
                    .collect();
                let mut converted = Vec::new();
                let mut resampler = InputRate::new(rate);
                for &frame in &input {
                    resampler.push(frame, |sample| converted.push(sample));
                }
                let mut live = RoomExcerpt::default();
                let mut resampler = InputRate::new(rate);
                for chunk in input.chunks(1013) {
                    live.extend(chunk, &mut resampler);
                }
                assert!(live.best.len() <= WINDOW && live.recent.len() <= WINDOW);
                let expected = loudest_excerpt(&converted);
                assert!(
                    live.finish() == expected,
                    "sample rate {rate}, peaks at {bursts:?}"
                );
            }
        }
    }
}
