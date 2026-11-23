//! End a microphone check when the waveform settles after an utterance.
use std::time::{Duration, Instant};

use orvia_core::Language;
use orvia_engine::mic_test::Recording;

const BASELINE_BLOCKS: usize = 15;
const ONSET_BLOCKS: usize = 4;
const RELEASE_BLOCKS: usize = 75;

/// The recording cue follows a 300 ms room sample. Once speech starts, wait for
/// 1.5 seconds near the measured noise floor before completing the take.
#[derive(Default)]
pub(super) struct Reader {
    checked: usize,
    last_data: Option<Instant>,
    baseline: [f32; BASELINE_BLOCKS],
    measured: usize,
    floor: f32,
    onset: usize,
    quiet: usize,
    resuming: usize,
    speaking: bool,
}

impl Reader {
    /// Whether the microphone has measured the room and the user can start speaking.
    pub(super) fn ready(&self) -> bool {
        self.measured == BASELINE_BLOCKS
    }

    /// Process newly captured audio and report completion or a stalled device.
    pub(super) fn listen(
        &mut self,
        recording: &mut Recording,
        language: Language,
    ) -> Result<bool, String> {
        let rate = recording.rate();
        let frames = recording.collect().map_err(|e| e.to_string())?;
        let step = (rate / 50) as usize;
        let now = Instant::now();
        if frames.len() - self.checked >= step {
            self.last_data = Some(now);
        } else if now.duration_since(*self.last_data.get_or_insert(now)) > Duration::from_secs(4) {
            return Err(language
                .text(
                    "Microphone disconnected or stalled.",
                    "マイクから音声が届いていません。",
                )
                .into());
        }
        Ok(self.process(frames, step))
    }

    /// Keep only the incomplete analysis block when another stream supplies the actual preview.
    pub(super) fn discard_checked(&mut self, recording: &mut Recording) {
        self.checked -= recording.discard_prefix(self.checked);
    }

    /// Keep a quiet lead-in for tone analysis while the reader waits for the user to start.
    /// Once an onset is possible, retain the whole phrase until trailing silence.
    pub(super) fn trim_waiting(&mut self, recording: &mut Recording) {
        let second = recording.rate() as usize;
        if !self.speaking && self.onset == 0 && self.checked > second * 2 {
            self.checked -= recording.discard_prefix(self.checked - second);
        }
    }

    fn process(&mut self, frames: &[[f32; 2]], step: usize) -> bool {
        // Arming takes a UI tick so previously buffered audio cannot finish a take before the cue appears.
        let armed = self.ready();
        for block in frames[self.checked..].chunks_exact(step) {
            self.checked += step;
            // Use the louder channel so a microphone plugged into either side is audible.
            let mut power = [0.0_f32; 2];
            for frame in block {
                for ch in 0..2 {
                    power[ch] += frame[ch] * frame[ch];
                }
            }
            if self.observe(power[0].max(power[1]) / step as f32, armed) {
                return true;
            }
        }
        false
    }

    /// Observe mean-square power in one 20 ms block.
    pub(super) fn observe(&mut self, power: f32, armed: bool) -> bool {
        if !self.ready() {
            self.baseline[self.measured] = power;
            self.measured += 1;
            if self.ready() {
                self.baseline.sort_unstable_by(f32::total_cmp);
                // Use the upper part of the room sample to allow ordinary noise fluctuations.
                self.floor = self.baseline[BASELINE_BLOCKS * 4 / 5];
            }
        } else if armed && !self.speaking {
            if power > (self.floor * 6.3).max(1e-8) {
                self.onset += 1;
                self.speaking = self.onset >= ONSET_BLOCKS;
            } else {
                self.onset = 0;
                self.floor += 0.04 * (power - self.floor);
            }
        } else if self.speaking {
            if power <= (self.floor * 4.0).max(4e-9) {
                self.quiet += 1;
                self.resuming = 0;
            } else {
                // A single click during the trailing silence should not restart the timer.
                self.resuming += 1;
                if self.resuming >= 3 {
                    self.quiet = 0;
                }
            }
        }
        self.quiet >= RELEASE_BLOCKS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(reader: &mut Reader, power: f32, blocks: usize) -> bool {
        (0..blocks).any(|_| reader.observe(power, reader.ready()))
    }

    #[test]
    fn room_sound_and_short_clicks_do_not_start_the_take() {
        let mut reader = Reader::default();
        assert!(!play(&mut reader, 1e-6, 600));
        assert!(reader.ready());
        assert!(!play(&mut reader, 0.02, 2));
        assert!(!play(&mut reader, 1.2e-6, 100));
        assert!(!reader.speaking);
    }

    #[test]
    fn a_quiet_utterance_can_finish_without_reading_the_full_count() {
        let mut reader = Reader::default();
        assert!(!play(&mut reader, 1e-8, BASELINE_BLOCKS));
        assert!(!play(&mut reader, 1e-6, 10));
        assert!(reader.speaking);
        assert!(!play(&mut reader, 1e-8, RELEASE_BLOCKS - 1));
        assert!(play(&mut reader, 1e-8, 1));
    }

    #[test]
    fn a_click_during_trailing_silence_does_not_postpone_the_end() {
        let mut reader = Reader::default();
        play(&mut reader, 1e-6, BASELINE_BLOCKS);
        play(&mut reader, 1e-3, 12);
        assert!(!play(&mut reader, 1e-6, 60));
        assert!(!play(&mut reader, 0.04, 1));
        assert!(play(&mut reader, 1e-6, 15));
    }

    #[test]
    fn a_short_pause_preserves_the_take_and_the_ending_is_relative_to_the_room() {
        let mut reader = Reader::default();
        assert!(!play(&mut reader, 1e-4, BASELINE_BLOCKS));
        assert!(!play(&mut reader, 1e-2, 10));
        assert!(!play(&mut reader, 1e-4, 50));
        assert!(!play(&mut reader, 1e-2, 10));
        assert!(!play(&mut reader, 1.5e-4, RELEASE_BLOCKS - 1));
        assert!(play(&mut reader, 1.5e-4, 1));
    }
}
