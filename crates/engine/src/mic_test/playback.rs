//! Physical output playback for in-memory previews and disk-backed microphone tests.

use super::{Clip, Frame, RECORDING_FILE, TonePreview, errors};
use crate::{
    formats::{Port, config, stream_config},
    host, is_virtual_cable,
};
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    FromSample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, RingBuffer};
use std::{
    fs::File,
    io::{BufReader, Seek},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// A saved microphone test. Only the WAV header is read when the test window opens.
pub struct SavedClip {
    reader: Arc<Mutex<Wav>>,
    frames: u32,
    rate: u32,
}

impl SavedClip {
    /// Find the current take, falling back to the newest legacy before/after take.
    ///
    /// # Errors
    ///
    /// Reports an inaccessible directory or an invalid saved recording.
    pub fn latest(dir: &Path) -> Result<Option<Self>> {
        latest_path(dir)?.map(|path| Self::open(&path)).transpose()
    }

    /// Open the header of a saved stereo float WAV for buffered playback.
    ///
    /// # Errors
    ///
    /// Reports an inaccessible file or an unsupported recording.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_file(File::open(path)?)
    }

    pub(super) fn from_file(file: File) -> Result<Self> {
        let reader = checked_reader(file)?;
        Ok(Self {
            frames: reader.duration(),
            rate: reader.spec().sample_rate,
            reader: Arc::new(Mutex::new(reader)),
        })
    }

    /// Duration of the saved take at its original capture rate.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(f64::from(self.frames) / f64::from(self.rate))
    }

    /// Stream the take to a physical output with bounded read-ahead.
    ///
    /// # Errors
    ///
    /// Reports an unreadable recording or an unavailable playback device.
    pub fn play(&self, id: Option<&str>) -> Result<Playback> {
        let (frames, errors) = Frames::buffered(self.reader.clone(), self.rate)?;
        Playback::start(id, self.rate, self.frames as usize, frames, Some(errors))
    }
}

pub(super) fn latest_path(dir: &Path) -> Result<Option<PathBuf>> {
    let current = dir.join(RECORDING_FILE);
    if current.try_exists()? {
        return Ok(Some(current));
    }
    let mut previous = Vec::new();
    for name in ["before.wav", "after.wav"] {
        let path = dir.join(name);
        if path.try_exists()? {
            previous.push((path.metadata()?.modified()?, path));
        }
    }
    Ok(previous
        .into_iter()
        .max_by_key(|(time, _)| *time)
        .map(|(_, path)| path))
}

type Wav = hound::WavReader<BufReader<std::fs::File>>;

pub(super) fn checked_reader(file: File) -> Result<Wav> {
    let mut file = BufReader::new(file);
    file.rewind()?;
    let (spec, frames) = {
        let header = hound::WavReader::new(&mut file)?;
        (header.spec(), header.duration())
    };
    ensure!(
        spec.channels == 2
            && spec.sample_format == hound::SampleFormat::Float
            && spec.bits_per_sample == 32
            && (8_000..=48_000).contains(&spec.sample_rate)
            && frames > 0
            && file.stream_position()? + u64::from(frames) * 8 <= file.get_ref().metadata()?.len(),
        "Unsupported microphone test WAV"
    );
    file.rewind()?;
    Ok(hound::WavReader::new(file)?)
}

impl Playback {
    pub(super) fn from_tone(preview: &TonePreview, id: Option<&str>) -> Result<Self> {
        let mut render = preview.clone().reader();
        let (frames, errors) = Frames::read_ahead::<{ 4 * orvia_core::BLOCK }>(
            orvia_core::SAMPLE_RATE,
            move |output| Ok(render(output)),
        )?;
        Self::start(
            id,
            orvia_core::SAMPLE_RATE,
            preview.len(),
            frames,
            Some(errors),
        )
    }

    pub(super) fn from_clip(clip: &Clip, id: Option<&str>) -> Result<Self> {
        Self::start(
            id,
            clip.rate,
            clip.frames.len(),
            Frames::Memory(clip.frames.clone()),
            None,
        )
    }

    fn start(
        id: Option<&str>,
        input_rate: u32,
        length: usize,
        frames: Frames,
        read_errors: Option<mpsc::Receiver<hound::Error>>,
    ) -> Result<Self> {
        let host = host()?;
        let device = match id {
            Some(id) => host.device_by_id(&id.parse()?),
            None => host.default_output_device(),
        }
        .context("Playback device unavailable")?;
        ensure!(
            !is_virtual_cable(&device),
            "Choose physical headphones or speakers"
        );
        let format = config(&device, Port::Headphones)?;
        let mut cursor = Cursor {
            frames,
            length,
            phase: 0,
            input_rate,
            output_rate: format.sample_rate(),
            channels: usize::from(format.channels()),
        };
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let (error, errors) = errors();
        let cfg = stream_config(&format);
        let stream = match format.sample_format() {
            SampleFormat::F32 => device.build_output_stream(
                cfg,
                move |out: &mut [f32], _| cursor.fill(out, &finished),
                error,
                None,
            ),
            SampleFormat::I16 => device.build_output_stream(
                cfg,
                move |out: &mut [i16], _| cursor.fill(out, &finished),
                error,
                None,
            ),
            other => return Err(anyhow!("Unsupported playback format: {other}")),
        }?;
        stream.play()?;
        Ok(Playback {
            _stream: stream,
            read_errors,
            done,
            errors,
        })
    }
}

enum Frames {
    Memory(Arc<Vec<Frame>>),
    Buffered {
        queue: Consumer<Frame>,
        pair: [Frame; 2],
        consumed: usize,
    },
}

impl Frames {
    fn buffered(
        reader: Arc<Mutex<Wav>>,
        rate: u32,
    ) -> Result<(Self, mpsc::Receiver<hound::Error>)> {
        let mut position = 0;
        Self::read_ahead::<2048>(rate, move |buffer| {
            // Rapid replays can briefly overlap the previous reader. Each reads the same open take.
            let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
            if reader.len() as usize - reader.samples::<f32>().len() != position as usize * 2 {
                reader.seek(position)?;
            }
            let mut samples = reader.samples::<f32>();
            for (i, frame) in buffer.iter_mut().enumerate() {
                let Some(left) = samples.next() else {
                    position += i as u32;
                    return Ok(i);
                };
                *frame = [
                    left?,
                    samples
                        .next()
                        .ok_or(hound::Error::FormatError("Missing right channel"))??,
                ];
            }
            position += buffer.len() as u32;
            Ok(buffer.len())
        })
    }

    fn read_ahead<const CHUNK: usize>(
        rate: u32,
        mut read: impl FnMut(&mut [Frame]) -> hound::Result<usize> + Send + 'static,
    ) -> Result<(Self, mpsc::Receiver<hound::Error>)> {
        let (mut tx, queue) = RingBuffer::new(CHUNK * 4);
        let (errors, read_errors) = mpsc::channel();
        let mut buffer = [[0.; 2]; CHUNK];
        // Prime the first callback before opening the stream. The worker prepares subsequent audio.
        let refill_interval = Duration::from_secs_f64(CHUNK as f64 / (2. * f64::from(rate)));
        let first = read(&mut buffer)?;
        tx.push_entire_slice(&buffer[..first])
            .expect("empty read-ahead queue");
        if first == CHUNK {
            std::thread::Builder::new()
                .name("preview-reader".into())
                .spawn(move || {
                    while !tx.is_abandoned() {
                        if tx.slots() < CHUNK {
                            std::thread::sleep(refill_interval);
                            continue;
                        }
                        match read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => {
                                tx.push_entire_slice(&buffer[..n])
                                    .expect("reserved read-ahead space");
                            }
                            Err(error) => {
                                let _ = errors.send(error);
                                break;
                            }
                        }
                    }
                })?;
        }
        Ok((
            Self::Buffered {
                queue,
                pair: [[0.; 2]; 2],
                consumed: 0,
            },
            read_errors,
        ))
    }

    fn pair(&mut self, index: usize, length: usize) -> Option<[Frame; 2]> {
        let next = (index + 1).min(length - 1);
        match self {
            Self::Memory(frames) => Some([frames[index], frames[next]]),
            Self::Buffered {
                queue,
                pair,
                consumed,
            } => {
                while *consumed <= next {
                    let frame = queue.pop().ok()?;
                    *pair = [pair[1], frame];
                    *consumed += 1;
                }
                Some(if index == next { [pair[1]; 2] } else { *pair })
            }
        }
    }
}

struct Cursor {
    frames: Frames,
    length: usize,
    phase: u64,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
}

impl Cursor {
    fn fill<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], done: &AtomicBool) {
        out.fill(T::EQUILIBRIUM);
        let rate = u64::from(self.output_rate);
        let mut index = self.phase / rate;
        // Keep the stream alive through the callback that consumes the last sample.
        if index >= self.length as u64 {
            done.store(true, Ordering::Relaxed);
            return;
        }
        // Carry the fractional source frame forward exactly across rates and callback boundaries.
        let mut remainder = self.phase % rate;
        let step = u64::from(self.input_rate) / rate;
        let fraction_step = u64::from(self.input_rate) % rate;
        for frame in out.chunks_exact_mut(self.channels) {
            if index >= self.length as u64 {
                break;
            }
            let Some([left, right]) = self.frames.pair(index as usize, self.length) else {
                break;
            };
            let fraction = remainder as f32 / self.output_rate as f32;
            let sample = |ch: usize| left[ch] + (right[ch] - left[ch]) * fraction;
            if self.channels == 1 {
                frame[0] = T::from_sample((sample(0) + sample(1)) * 0.5);
            } else {
                frame[0] = T::from_sample(sample(0));
                frame[1] = T::from_sample(sample(1));
            }
            index += step;
            remainder += fraction_step;
            if remainder >= rate {
                remainder -= rate;
                index += 1;
            }
        }
        self.phase = index * rate + remainder;
    }
}

/// Holds the physical output stream. Dropping it stops the preview.
pub struct Playback {
    read_errors: Option<mpsc::Receiver<hound::Error>>,
    _stream: Stream,
    done: Arc<AtomicBool>,
    errors: mpsc::Receiver<cpal::Error>,
}

impl Playback {
    /// Return whether the clip has ended or the playback device failed.
    ///
    /// # Errors
    ///
    /// Returns an error if the audio backend reports device loss or the WAV reader fails.
    pub fn finished(&self) -> Result<bool> {
        if let Ok(error) = self.errors.try_recv() {
            return Err(error.into());
        }
        if let Some(error) = self.read_errors.as_ref().and_then(|rx| rx.try_recv().ok()) {
            return Err(error.into());
        }
        Ok(self.done.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_replays_keep_the_open_take_across_wraps_and_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let length = 22_135;
        let clip = Clip {
            rate: 44_100,
            frames: (0..length)
                .map(|i| [i as f32 * 0.001, -(i as f32)])
                .collect::<Vec<_>>()
                .into(),
        };
        clip.save_latest(dir.path()).unwrap();
        let saved = SavedClip::latest(dir.path()).unwrap().unwrap();
        assert_eq!(saved.duration(), clip.duration());
        let (mut first, errors) = Frames::buffered(saved.reader.clone(), saved.rate).unwrap();
        let (mut replay, _) = Frames::buffered(saved.reader.clone(), saved.rate).unwrap();
        Clip {
            rate: 8_000,
            frames: vec![[0.; 2]; 100].into(),
        }
        .save_latest(dir.path())
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        for i in 0..length {
            let expected = [clip.frames[i], clip.frames[(i + 1).min(length - 1)]];
            for source in [&mut first, &mut replay] {
                loop {
                    if let Some(pair) = source.pair(i, length) {
                        assert_eq!(pair, expected);
                        break;
                    }
                    assert!(errors.try_recv().is_err());
                    assert!(std::time::Instant::now() < deadline, "reader stalled");
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        assert_ne!(
            SavedClip::latest(dir.path()).unwrap().unwrap().duration(),
            saved.duration()
        );
    }

    #[test]
    fn anonymous_preview_survives_replay_without_replacing_the_saved_take() {
        let dir = tempfile::tempdir().unwrap();
        let original = Clip {
            rate: 48_000,
            frames: vec![[0.22, -0.15]; 25].into(),
        };
        original.save_latest(dir.path()).unwrap();
        let frames: Vec<Frame> = (0..16_223).map(|i| [i as f32 / 32_000., -0.02]).collect();
        let mut wav = super::super::file_recording::PendingWav::preview(48_000).unwrap();
        for chunk in frames.chunks(397) {
            wav.append(chunk).unwrap();
        }
        let clip = wav.publish(SavedClip::from_file).unwrap();
        let (mut first, _) = Frames::buffered(clip.reader.clone(), clip.rate).unwrap();
        let (mut replay, _) = Frames::buffered(clip.reader.clone(), clip.rate).unwrap();
        drop(clip);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        for i in 0..frames.len() {
            for stream in [&mut first, &mut replay] {
                let pair = loop {
                    if let Some(pair) = stream.pair(i, frames.len()) {
                        break pair;
                    }
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                };
                assert_eq!(pair, [frames[i], frames[(i + 1).min(frames.len() - 1)]]);
            }
        }
        assert_eq!(
            Clip::load_latest(dir.path()).unwrap().unwrap().frames,
            original.frames
        );
        assert_eq!(dir.path().read_dir().unwrap().count(), 1);
    }

    #[test]
    fn a_truncated_take_is_rejected_before_playback() {
        let dir = tempfile::tempdir().unwrap();
        Clip {
            rate: 48_000,
            frames: vec![[0.; 2]; 100].into(),
        }
        .save_latest(dir.path())
        .unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join(RECORDING_FILE))
            .unwrap();
        file.set_len(file.metadata().unwrap().len() - 1).unwrap();
        assert!(SavedClip::latest(dir.path()).is_err());
    }

    #[test]
    fn replay_preserves_pauses_across_buffers_and_resamples_both_channels() {
        let mut cursor = Cursor {
            frames: Frames::Memory(Arc::new(vec![[0.5, -0.5], [0., 0.], [0., 0.], [0.5, -0.5]])),
            length: 4,
            phase: 0,
            input_rate: 24_000,
            output_rate: 48_000,
            channels: 2,
        };
        let done = AtomicBool::new(false);
        let mut output = [1.0_f32; 20];
        for chunk in output.chunks_mut(6) {
            cursor.fill(chunk, &done);
        }
        assert_eq!(
            output,
            [
                0.5, -0.5, 0.25, -0.25, 0., 0., 0., 0., 0., 0., 0.25, -0.25, 0.5, -0.5, 0.5, -0.5,
                0., 0., 0., 0.
            ]
        );
        assert!(done.load(Ordering::Relaxed));
    }

    #[test]
    fn a_starved_replay_resumes_at_the_exact_fractional_frame_of_a_long_take() {
        let source: Vec<Frame> = (0..17)
            .map(|i| [i as f32 / 21., 0.25 - i as f32 / 29.])
            .collect();
        for (input_rate, output_rate, channels) in [
            (48_000, 48_000, 2),
            (44_100, 48_000, 2),
            (16_000, 48_000, 1),
            (48_000, 44_100, 3),
            (48_000, 16_000, 2),
        ] {
            // Start a day into the source without allocating its preceding samples.
            let start = 86_400 * u64::from(input_rate);
            let rate = u64::from(output_rate);
            let step = u64::from(input_rate);
            let initial_fraction = step % rate;
            let phase = start * rate + initial_fraction;
            let (mut tx, queue) = RingBuffer::new(source.len());
            let mut cursor = Cursor {
                frames: Frames::Buffered {
                    queue,
                    pair: [[0.; 2]; 2],
                    consumed: start as usize,
                },
                length: start as usize + source.len(),
                phase,
                input_rate,
                output_rate,
                channels,
            };
            let done = AtomicBool::new(false);
            let mut callback = vec![1_f32; 3 * channels + usize::from(channels > 1)];
            tx.push(source[0]).unwrap();
            cursor.fill(&mut callback, &done);
            assert!(callback.iter().all(|&sample| sample == 0.));
            assert_eq!(cursor.phase, phase);
            assert!(!done.load(Ordering::Relaxed));

            tx.push_entire_slice(&source[1..]).unwrap();
            let output_frames = (source.len() as u64 * rate - initial_fraction).div_ceil(step);
            for first in (0..output_frames).step_by(3) {
                cursor.fill(&mut callback, &done);
                assert!(!done.load(Ordering::Relaxed));
                let mut expected = vec![0.; callback.len()];
                for (offset, frame) in expected.chunks_exact_mut(channels).enumerate() {
                    let pos = initial_fraction + (first + offset as u64) * step;
                    let index = (pos / rate) as usize;
                    if let Some(left) = source.get(index) {
                        let right = source[(index + 1).min(source.len() - 1)];
                        let fraction = (pos % rate) as f32 / output_rate as f32;
                        let sample = |ch: usize| left[ch] + (right[ch] - left[ch]) * fraction;
                        if channels == 1 {
                            frame[0] = (sample(0) + sample(1)) * 0.5;
                        } else {
                            frame[..2].copy_from_slice(&[sample(0), sample(1)]);
                        }
                    }
                }
                assert_eq!(callback, expected);
            }
            assert_eq!(cursor.phase, phase + output_frames * step);
            cursor.fill(&mut callback, &done);
            assert!(done.load(Ordering::Relaxed));
            assert!(callback.iter().all(|&sample| sample == 0.));
        }
    }
}
