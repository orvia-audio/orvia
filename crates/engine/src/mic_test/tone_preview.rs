//! Render a tone comparison in continuous 10 ms blocks as the listener plays it.
use super::{Clip, Frame, GuideClip, Playback};
use anyhow::Result;
use orvia_core::{
    BLOCK, SAMPLE_RATE, Tone,
    calibration::{GuideAudio, SampleError},
    tone::ToneProcessor,
};
use std::sync::Arc;

const GAP: usize = SAMPLE_RATE as usize / 4;

/// One tone setting to audition over a shared neutral room excerpt and reading.
#[derive(Clone)]
pub struct TonePreview {
    room: Arc<Vec<Frame>>,
    voice: Arc<GuideClip>,
    tone: Tone,
}

impl TonePreview {
    pub(super) fn pair(
        room: Arc<Vec<Frame>>,
        voice: Arc<GuideClip>,
        current: Tone,
        recommended: Tone,
    ) -> [Self; 2] {
        let before = Self {
            room,
            voice,
            tone: current,
        };
        let after = Self {
            tone: recommended,
            ..before.clone()
        };
        [before, after]
    }

    /// Start at the beginning on the chosen headphones, or on the default physical output.
    /// Each playback has its own filter history and a bounded read-ahead queue.
    ///
    /// # Errors
    ///
    /// Returns an error if the output device or playback worker cannot be started.
    pub fn play(&self, id: Option<&str>) -> Result<Playback> {
        Playback::from_tone(self, id)
    }

    pub(super) fn len(&self) -> usize {
        self.room.len() + GAP + self.voice.frame_count()
    }

    // Fill the room, pause, and reading on one timeline so filters never restart at a boundary.
    fn copy_frames(&self, position: usize, output: &mut [Frame]) -> std::io::Result<()> {
        let voice_start = self.room.len() + GAP;
        let room_count = self.room.len().saturating_sub(position).min(output.len());
        let (room, remaining) = output.split_at_mut(room_count);
        if !room.is_empty() {
            room.copy_from_slice(&self.room[position..position + room_count]);
        }
        let gap_count = voice_start
            .saturating_sub(position + room_count)
            .min(remaining.len());
        let (gap, voice) = remaining.split_at_mut(gap_count);
        gap.fill([0.; 2]);
        let offset = position.saturating_sub(voice_start);
        if !voice.is_empty() {
            self.voice.read(offset, voice)?;
        }
        Ok(())
    }

    /// Produce successive buffers of whole DSP blocks, with a short final block at EOF.
    pub(super) fn reader(self) -> impl FnMut(&mut [Frame]) -> std::io::Result<usize> {
        let mut position = 0;
        let mut processor = ToneProcessor::new(self.tone);
        move |output| {
            let count = (self.len() - position).min(output.len());
            debug_assert!(count.is_multiple_of(BLOCK) || position + count == self.len());
            let output = &mut output[..count];
            self.copy_frames(position, output)?;
            for block in output.chunks_mut(BLOCK) {
                processor.process_unmetered(block);
            }
            position += count;
            Ok(count)
        }
    }

    pub(super) fn render(self) -> Result<Clip, SampleError> {
        let mut frames = vec![[0.; 2]; self.len()];
        self.reader()(&mut frames).map_err(|_| SampleError::Incomplete)?;
        Ok(Clip {
            rate: SAMPLE_RATE,
            frames: frames.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_match_eager_processing_across_the_room_gap_and_partial_final_block() {
        let audio = |count| {
            (0..count)
                .map(|i| {
                    let x = 0.81 * (i as f32 * 0.08).sin();
                    [x, -0.6 * x]
                })
                .collect::<Vec<_>>()
        };
        let room = Arc::new(audio(BLOCK * 52 + 37));
        let voice = Arc::new(audio(BLOCK * 73 + 9));
        let boosted = Tone {
            low_cut_hz: 80,
            gain_percent: 135,
            compressor_ratio: 4,
        };
        let guide = Arc::new(GuideClip::memory(voice.clone()));
        let previews = TonePreview::pair(room.clone(), guide.clone(), Tone::default(), boosted);
        for preview in previews {
            assert!(Arc::ptr_eq(&room, &preview.room) && Arc::ptr_eq(&guide, &preview.voice));
            let mut expected = room.as_ref().clone();
            expected.resize(room.len() + GAP, [0.; 2]);
            expected.extend_from_slice(&voice);
            let mut processor = ToneProcessor::new(preview.tone);
            for block in expected.chunks_mut(BLOCK) {
                processor.process(block);
            }
            let mut first = preview.clone().reader();
            let mut replay = preview.reader();
            let mut buffer = [[0.; 2]; 4 * BLOCK];
            assert_eq!(first(&mut buffer).unwrap(), buffer.len());
            for chunk in expected.chunks(buffer.len()) {
                let count = replay(&mut buffer).unwrap();
                assert_eq!(&buffer[..count], chunk);
            }
            assert_eq!(replay(&mut buffer).unwrap(), 0);
            for chunk in expected[buffer.len()..].chunks(buffer.len()) {
                let count = first(&mut buffer).unwrap();
                assert_eq!(&buffer[..count], chunk);
            }
            assert_eq!(first(&mut buffer).unwrap(), 0);
        }
    }
}
