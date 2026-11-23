//! Incremental WAV storage for a microphone test with an open-ended reading time.
use super::{Frame, RECORDING_FILE, Recording, SavedClip};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) struct PendingWav {
    writer: hound::WavWriter<BufWriter<File>>,
    reader: File,
    replacement: Option<(tempfile::TempPath, PathBuf)>,
}

impl PendingWav {
    pub(super) fn new(path: &Path, rate: u32) -> Result<Self> {
        let (file, temporary) =
            tempfile::NamedTempFile::new_in(path.parent().context("Missing recording directory")?)?
                .into_parts();
        Self::with_file(file, Some((temporary, path.to_owned())), rate)
    }

    pub(super) fn preview(rate: u32) -> Result<Self> {
        Self::with_file(tempfile::tempfile()?, None, rate)
    }

    fn with_file(
        file: File,
        replacement: Option<(tempfile::TempPath, PathBuf)>,
        rate: u32,
    ) -> Result<Self> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        Ok(Self {
            writer: hound::WavWriter::new(
                BufWriter::with_capacity(64 * 1024, file.try_clone()?),
                spec,
            )?,
            reader: file,
            replacement,
        })
    }

    pub(super) fn append(&mut self, frames: &[Frame]) -> Result<()> {
        // RIFF uses a 32-bit file length. Leave room for the float WAV header.
        let total = u64::from(self.writer.duration()) + frames.len() as u64;
        ensure!(
            total <= (u64::from(u32::MAX) - 128) / 8,
            "Recording exceeds the WAV size limit"
        );
        for &sample in frames.iter().flatten() {
            self.writer.write_sample(sample)?;
        }
        Ok(())
    }

    pub(super) fn publish<T>(self, open: impl FnOnce(File) -> Result<T>) -> Result<T> {
        self.writer.finalize()?;
        let value = open(self.reader)?;
        let Some((temporary, destination)) = self.replacement else {
            return Ok(value);
        };
        // Windows permits this rename while another window is playing the old file.
        let staging = temporary.keep()?;
        if let Err(error) = std::fs::rename(&staging, destination) {
            let _ = std::fs::remove_file(staging);
            return Err(error.into());
        }
        Ok(value)
    }
}

/// A virtual microphone take streamed to temporary storage on the polling thread.
/// Dropping the take before completion discards it and preserves the previous recording.
pub struct FileRecording {
    capture: Recording,
    take: PendingWav,
}

impl FileRecording {
    /// Start a microphone test and prepare its temporary WAV beside the last saved take.
    ///
    /// # Errors
    ///
    /// Reports capture device, directory or WAV creation failures.
    pub fn start(id: &str, dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let capture = Recording::start(id)?;
        let take = PendingWav::new(&dir.join(RECORDING_FILE), capture.rate)?;
        Ok(Self { capture, take })
    }

    /// Record the virtual microphone for replay during the current session.
    /// The OS removes the temporary audio after the last reader closes.
    ///
    /// # Errors
    ///
    /// Reports capture device or temporary file creation failures.
    pub fn start_preview(id: &str) -> Result<Self> {
        let capture = Recording::start(id)?;
        let take = PendingWav::preview(capture.rate)?;
        Ok(Self { capture, take })
    }

    /// Sample rate of the recording device in Hz.
    pub fn rate(&self) -> u32 {
        self.capture.rate
    }

    /// Duration of all captured audio, including samples awaiting the next poll.
    pub fn duration(&self) -> Duration {
        let count = u64::from(self.take.writer.duration()) + self.capture.rx.slots() as u64;
        Duration::from_secs_f64(count as f64 / f64::from(self.rate()))
    }

    /// Append newly arrived frames to the WAV and return the pending speech-detection window.
    ///
    /// # Errors
    ///
    /// Reports capture errors, a full device queue or a failed disk write. Discard the take on error.
    pub fn collect(&mut self) -> Result<&[Frame]> {
        let saved = self.capture.frames.len();
        let frames = self.capture.collect()?;
        self.take.append(&frames[saved..])?;
        Ok(frames)
    }

    /// Write the device queue directly to disk when another stream supplies speech detection.
    ///
    /// # Errors
    ///
    /// Reports capture errors, a full device queue or a failed disk write.
    pub fn drain(&mut self) -> Result<()> {
        self.capture.frames.clear();
        self.capture.drain(|frames| self.take.append(frames))
    }

    /// Remove analyzed frames from memory. The WAV keeps every sample returned by collect.
    pub fn discard_prefix(&mut self, count: usize) -> usize {
        self.capture.discard_prefix(count)
    }

    /// Stop capture, append the final device buffer and commit the complete recording.
    ///
    /// # Errors
    ///
    /// Reports an empty recording, device loss or a file finalization error.
    pub fn finish(mut self) -> Result<SavedClip> {
        self.capture
            .stop_and_drain(|frames| self.take.append(frames))?;
        ensure!(
            self.take.writer.duration() > 0,
            "The microphone returned no audio frames"
        );
        // Open the completed inode before another window can replace the destination.
        self.take.publish(SavedClip::from_file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mic_test::Clip;

    #[test]
    fn cancelled_and_failed_retakes_keep_the_previous_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RECORDING_FILE);
        let mut tx = PendingWav::new(&path, 48_000).unwrap();
        tx.append(&[[0.2, 0.1]]).unwrap();
        tx.append(&[[-0.5, 0.3], [0., -0.25]]).unwrap();
        let clip = tx.publish(SavedClip::from_file).unwrap();
        assert_eq!(clip.duration(), Duration::from_secs_f64(3. / 48_000.));
        let original = Clip::load(&path).unwrap();
        for cancel in [true, false] {
            let mut tx = PendingWav::new(&path, 48_000).unwrap();
            tx.append(&[[0.8, -0.1]]).unwrap();
            if !cancel {
                assert!(
                    tx.publish::<()>(|_| anyhow::bail!("unreadable draft"))
                        .is_err()
                );
            }
        }
        assert_eq!(Clip::load(&path).unwrap().frames, original.frames);
        assert_eq!(dir.path().read_dir().unwrap().count(), 1);
    }
}
