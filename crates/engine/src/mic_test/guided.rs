//! Short readings stay in memory. Longer readings spill into a private, session-only PCM file.
use super::{Clip, Frame, InputRate, Recording, TonePreview, room};
use anyhow::{Result, ensure};
use orvia_core::{
    BLOCK, Mixing, SAMPLE_RATE, Tone,
    calibration::{self, GuideAudio, Recommendation, SampleError},
};
use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    sync::{Arc, Mutex},
};

const MEMORY_FRAMES: usize = SAMPLE_RATE as usize * 8;
enum Storage {
    Memory(Arc<Vec<Frame>>),
    File(Mutex<(BufReader<File>, usize)>),
}

/// A complete reading at 48 kHz, shared by analysis and independent headphone previews.
pub struct GuideClip {
    storage: Storage,
    len: usize,
}

impl GuideClip {
    pub(super) fn memory(frames: Arc<Vec<Frame>>) -> Self {
        Self {
            len: frames.len(),
            storage: Storage::Memory(frames),
        }
    }

    pub(super) fn read(&self, start: usize, out: &mut [Frame]) -> io::Result<()> {
        let end = start
            .checked_add(out.len())
            .filter(|&n| n <= self.len)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "Reading exceeds guide length")
            })?;
        match &self.storage {
            Storage::Memory(frames) => out.copy_from_slice(&frames[start..end]),
            Storage::File(reader) => {
                let mut locked = reader.lock().unwrap_or_else(|e| e.into_inner());
                let (file, pos) = &mut *locked;
                // Invalidate the logical position if the OS returns a short read or an error.
                if std::mem::replace(pos, usize::MAX) != start {
                    file.seek(SeekFrom::Start(start as u64 * 8))?;
                }
                file.read_exact(bytemuck::cast_slice_mut(out))?;
                *pos = end;
            }
        }
        Ok(())
    }
}

impl GuideAudio for GuideClip {
    fn frame_count(&self) -> usize {
        self.len
    }
    fn read_frames(&self, start: usize, out: &mut [Frame]) -> Result<(), SampleError> {
        self.read(start, out).map_err(|_| SampleError::Incomplete)
    }
}

enum Draft {
    Memory(Vec<Frame>),
    File(BufWriter<File>, usize),
}

impl Draft {
    fn append(&mut self, batch: &[Frame]) -> Result<()> {
        if let Self::Memory(frames) = self {
            let length = frames
                .len()
                .checked_add(batch.len())
                .ok_or_else(|| anyhow::anyhow!("Reading exceeds addressable memory"))?;
            if length <= MEMORY_FRAMES {
                if length > frames.capacity() && frames.capacity() > MEMORY_FRAMES / 2 {
                    frames.try_reserve_exact(MEMORY_FRAMES - frames.len())?;
                } else {
                    frames.try_reserve(batch.len())?;
                }
                frames.extend_from_slice(batch);
                return Ok(());
            }
            let mut file = BufWriter::with_capacity(64 * 1024, tempfile::tempfile()?);
            file.write_all(bytemuck::cast_slice(frames))?;
            *self = Self::File(file, frames.len());
        }
        if let Self::File(file, length) = self {
            let next = length
                .checked_add(batch.len())
                .ok_or_else(|| anyhow::anyhow!("Reading exceeds addressable storage"))?;
            file.write_all(bytemuck::cast_slice(batch))?;
            *length = next;
        }
        Ok(())
    }

    fn finish(self) -> Result<GuideClip> {
        let clip = match self {
            Self::Memory(mut frames) => {
                frames.shrink_to_fit();
                GuideClip::memory(frames.into())
            }
            Self::File(file, len) => {
                let mut file = file.into_inner().map_err(io::Error::from)?;
                file.rewind()?;
                GuideClip {
                    storage: Storage::File(Mutex::new((
                        BufReader::with_capacity(64 * 1024, file),
                        0,
                    ))),
                    len,
                }
            }
        };
        ensure!(clip.len > 0, "The microphone returned no audio frames");
        Ok(clip)
    }
}

/// A spoken guide with a bounded speech-detection window and a reconstructible full reading.
pub struct GuidedRecording {
    input: Recording,
    draft: Draft,
    converter: Option<InputRate>,
}

impl GuidedRecording {
    /// Keep the supplied capture at its original rate for speech detection and at 48 kHz for analysis.
    pub fn new(input: Recording) -> Self {
        Self {
            converter: (input.rate() != SAMPLE_RATE).then(|| InputRate::new(input.rate())),
            input,
            draft: Draft::Memory(Vec::new()),
        }
    }

    /// Pending source-rate frames for the speech detector, including the trimmable quiet lead-in.
    pub fn input(&mut self) -> &mut Recording {
        &mut self.input
    }

    /// Keep all analyzed frames following the retained lead-in and discard them from the capture buffer.
    ///
    /// # Errors
    ///
    /// Reports a failed temporary file write or insufficient memory.
    pub fn retain(&mut self, count: usize) -> Result<usize> {
        let count = count.min(self.input.frames.len());
        store(
            &mut self.draft,
            &mut self.converter,
            &self.input.frames[..count],
        )?;
        Ok(self.input.discard_prefix(count))
    }

    /// Close the device and make the entire reading available for analysis or replay.
    ///
    /// # Errors
    ///
    /// Reports an empty recording, capture loss, or a temporary file error.
    pub fn finish(mut self) -> Result<GuideClip> {
        store(&mut self.draft, &mut self.converter, &self.input.frames)?;
        self.input
            .stop_and_drain(|frames| store(&mut self.draft, &mut self.converter, frames))?;
        self.draft.finish()
    }
}

fn store(draft: &mut Draft, converter: &mut Option<InputRate>, frames: &[Frame]) -> Result<()> {
    let Some(rate) = converter else {
        return draft.append(frames);
    };
    let mut batch = [[0.; 2]; BLOCK * 6];
    // The lowest negotiated rate is 8 kHz; at most six output frames are emitted per input.
    for frames in frames.chunks(BLOCK) {
        let mut n = 0;
        rate.push_batch(frames.iter().copied(), |f| {
            batch[n] = f;
            n += 1;
        });
        draft.append(&batch[..n])?;
    }
    Ok(())
}

impl Clip {
    /// Analyze two complete guided readings, stopping when the settings window abandons the result.
    ///
    /// # Errors
    ///
    /// Reports an incomplete, clipped or unclear guide, including a cancelled or unreadable take.
    pub fn recommend_guided(
        self,
        phrases: [GuideClip; 2],
        mixing: Mixing,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Recommendation, SampleError> {
        if cancelled() {
            return Err(SampleError::Incomplete);
        }
        let room = self.into_model_rate()?;
        calibration::recommend_from(&room.frames, [&phrases[0], &phrases[1]], mixing, cancelled)
    }

    /// Prepare tone previews sharing a complete, possibly disk-backed reading.
    ///
    /// # Errors
    ///
    /// Reports a cancelled, unreadable, clipped or unclear guide.
    pub fn guided_tone_previews(
        self,
        voice: GuideClip,
        current: Tone,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Tone, [TonePreview; 2]), SampleError> {
        if cancelled() {
            return Err(SampleError::Incomplete);
        }
        voice.tone_previews(self.room_excerpt()?, current, cancelled)
    }

    pub(super) fn room_excerpt(self) -> Result<Arc<Vec<Frame>>, SampleError> {
        let whole = self.into_model_rate()?.frames;
        let excerpt = room::loudest_excerpt(&whole);
        Ok(if excerpt.len() == whole.len() {
            whole
        } else {
            Arc::new(excerpt.to_vec())
        })
    }
}

impl GuideClip {
    pub(super) fn tone_previews(
        self,
        room: Arc<Vec<Frame>>,
        current: Tone,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Tone, [TonePreview; 2]), SampleError> {
        let recommended = orvia_core::tone::recommend_from(&room, &self, cancelled)?;
        Ok((
            recommended,
            TonePreview::pair(room, Arc::new(self), current, recommended),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_final_flush_failures_discard_the_guide() {
        let original = tempfile::NamedTempFile::new().unwrap();
        let read_only = || {
            Draft::File(
                BufWriter::with_capacity(16, File::open(original.path()).unwrap()),
                0,
            )
        };
        let mut immediate = read_only();
        assert!(immediate.append(&[[0.; 2]; 5]).is_err());
        let mut buffered = read_only();
        buffered.append(&[[0.; 2]]).unwrap();
        assert!(buffered.finish().is_err());
        assert_eq!(original.as_file().metadata().unwrap().len(), 0);
    }

    #[test]
    fn dropping_the_last_preview_releases_the_anonymous_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut draft = Draft::File(
            BufWriter::new(tempfile::tempfile_in(dir.path()).unwrap()),
            0,
        );
        draft.append(&[[0.125, -0.125]; BLOCK]).unwrap();
        let voice = Arc::new(draft.finish().unwrap());
        let [before, after] = TonePreview::pair(
            Arc::new(Vec::new()),
            voice,
            Tone::default(),
            Tone::default(),
        );
        drop(before);
        drop(after);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn short_and_spilled_readings_replay_exact_model_frames_across_seeks() {
        for (rate, seconds) in [(48_000, 2), (48_000, 11), (44_100, 11), (8_000, 11)] {
            let mut draft = Draft::Memory(Vec::new());
            let mut conversion = (rate != SAMPLE_RATE).then(|| InputRate::new(rate));
            let mut reference = InputRate::new(rate);
            let mut expected = Vec::new();
            let mut native = Vec::new();
            for i in 0..rate * seconds + 37 {
                let sample = (i % 991) as f32 * 0.00031;
                let frame = [sample, -sample];
                reference.push(frame, |f| expected.push(f));
                native.push(frame);
                if native.len() == 915 {
                    store(&mut draft, &mut conversion, &native).unwrap();
                    native.clear();
                }
            }
            store(&mut draft, &mut conversion, &native).unwrap();
            let take = draft.finish().unwrap();
            assert_eq!(matches!(&take.storage, Storage::File(_)), seconds > 8);
            assert_eq!(take.frame_count(), expected.len());
            let mut buffer = [[0.; 2]; 503];
            for start in [1003, 1, expected.len() - buffer.len(), 0] {
                take.read_frames(start, &mut buffer).unwrap();
                assert_eq!(&buffer, &expected[start..start + buffer.len()]);
            }
            for (at, batch) in expected.chunks(buffer.len()).enumerate() {
                let out = &mut buffer[..batch.len()];
                take.read_frames(at * 503, out).unwrap();
                assert_eq!(
                    bytemuck::cast_slice::<_, u8>(out),
                    bytemuck::cast_slice::<_, u8>(batch)
                );
            }
            assert!(take.read_frames(expected.len(), &mut buffer[..1]).is_err());
        }
    }
}
