//! Microphone recordings with local preview playback.
use crate::{
    formats::{InputRate, Port, config, stream_config, stream_continues},
    host, is_virtual_cable, label,
};
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    Device, FromSample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use orvia_core::{
    Mixing, SAMPLE_RATE,
    calibration::{self, Recommendation, SampleError},
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    io::BufWriter,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Capacity of the queue between the device callback and the recorder.
const QUEUE_DURATION: Duration = Duration::from_secs(10);
const RECORDING_FILE: &str = "recording.wav";
type Frame = [f32; 2];

fn is_test_source(device: &Device) -> bool {
    // PulseAudio also exposes a monitor for every physical speaker.
    #[cfg(target_os = "linux")]
    {
        device
            .id()
            .is_ok_and(|id| matches!(id.id(), "orvia_microphone" | "orvia_output.monitor"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        is_virtual_cable(device)
    }
}

/// List the recording endpoints that a receiving application can open.
///
/// # Errors
///
/// Returns an error if the host cannot enumerate capture devices.
pub fn sources() -> Result<Vec<(String, String)>, cpal::Error> {
    Ok(host()?
        .input_devices()?
        .filter(is_test_source)
        .filter_map(|d| Some((d.id().ok()?.to_string(), label(&d).ok()?)))
        .collect())
}

/// Open the selected playback side of the cable used by the live mixer.
///
/// # Errors
///
/// Returns an error if no supported virtual output is available.
pub fn virtual_output(id: Option<&str>) -> Result<Device> {
    let host = host()?;
    let output = match id {
        Some(id) => host
            .device_by_id(&id.parse()?)
            .context("Selected virtual output is disconnected")?,
        #[cfg(not(windows))]
        None => anyhow::bail!("Choose a virtual output in microphone settings"),
        #[cfg(windows)]
        None => host
            .output_devices()?
            .find(crate::is_orvia)
            .context("Choose a virtual output in microphone settings")?,
    };
    ensure!(
        output.description()?.supports_output() && is_virtual_cable(&output),
        "Select the playback side of a supported virtual cable"
    );
    Ok(output)
}

/// Resolve the recording side of a virtual output, honoring an explicit pairing.
/// Automatic matching uses the shared device ID, full endpoint name or packaged Linux pair.
///
/// # Errors
///
/// Returns an error when the selected receiver is disconnected or the pairing is ambiguous.
pub fn receiver(output: &Device, selected: Option<&str>) -> Result<Device> {
    let host = host()?;
    if let Some(id) = selected {
        let device = host
            .device_by_id(&id.parse()?)
            .context("Virtual microphone disconnected")?;
        ensure!(
            device.description()?.supports_input() && is_test_source(&device),
            "Choose a virtual microphone recording endpoint"
        );
        return Ok(device);
    }
    let output_id = output.id()?;
    let name = label(output)?;
    let mut matches = host
        .input_devices()?
        .filter(is_test_source)
        .filter(|device| {
            if cfg!(target_os = "linux") && output_id.id() == "orvia_output" {
                return device.id().is_ok_and(|id| id.id() == "orvia_microphone");
            }
            device.id().is_ok_and(|id| id == output_id)
                || label(device).is_ok_and(|label| label == name)
        });
    let found = matches
        .next()
        .context("Choose the virtual microphone recording endpoint in Devices")?;
    ensure!(
        matches.next().is_none(),
        "Multiple matching virtual microphones. Select a recording endpoint in Devices"
    );
    Ok(found)
}

/// Return the capture endpoint used by applications receiving this mix.
///
/// # Errors
///
/// Returns a pairing or device error if either side of the route is unavailable.
pub fn routed_source(settings: &orvia_core::Settings) -> Result<String> {
    let output = virtual_output(settings.output.as_deref())?;
    Ok(receiver(&output, settings.receiver.as_deref())?
        .id()?
        .to_string())
}

fn errors() -> (
    impl FnMut(cpal::Error) + Send + 'static,
    mpsc::Receiver<cpal::Error>,
) {
    let (tx, rx) = mpsc::sync_channel(1);
    (
        move |error: cpal::Error| {
            if !stream_continues(&error) {
                let _ = tx.try_send(error);
            }
        },
        rx,
    )
}

/// An active microphone recording. Drain it regularly while waiting for the reader. Dropping it stops capture.
pub struct Recording {
    stream: Stream,
    frames: Vec<Frame>,
    rx: Consumer<Frame>,
    errors: mpsc::Receiver<cpal::Error>,
    rate: u32,
}

impl Recording {
    /// Record the explicitly selected virtual microphone into a preallocated buffer.
    ///
    /// # Errors
    ///
    /// Returns an error for disconnected devices, physical inputs or unavailable formats.
    pub fn start(id: &str) -> Result<Self> {
        let device = host()?
            .device_by_id(&id.parse()?)
            .context("Virtual microphone disconnected")?;
        ensure!(is_test_source(&device), "Choose a virtual microphone");
        Self::open(&device, Port::Recording)
    }

    /// Capture the physical input selected in the mixer, using the default when omitted.
    ///
    /// # Errors
    ///
    /// Returns an error if the input is unavailable, virtual or cannot be opened.
    pub fn start_microphone(id: Option<&str>) -> Result<Self> {
        let host = host()?;
        let device = match id {
            Some(id) => host.device_by_id(&id.parse()?),
            None => host.default_input_device(),
        }
        .context("Microphone unavailable")?;
        ensure!(
            device.description()?.supports_input() && !is_virtual_cable(&device),
            "Choose a physical microphone"
        );
        Self::open(&device, Port::Microphone)
    }

    fn open(device: &Device, port: Port) -> Result<Self> {
        let format = config(device, port)?;
        let rate = format.sample_rate();
        let (tx, rx) = RingBuffer::new(rate as usize * QUEUE_DURATION.as_secs() as usize);
        let (error, errors) = errors();
        let stream = match format.sample_format() {
            SampleFormat::F32 => capture::<f32>(device, &format, tx, error),
            SampleFormat::I16 => capture::<i16>(device, &format, tx, error),
            other => return Err(anyhow!("Unsupported recording format: {other}")),
        }?;
        stream.play()?;
        Ok(Self {
            frames: Vec::new(),
            stream,
            rx,
            errors,
            rate,
        })
    }

    /// Duration of the frames received so far, excluding device startup time.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64((self.frames.len() + self.rx.slots()) as f64 / f64::from(self.rate))
    }

    /// Drain the device queue without stopping capture. Returns the complete take so far.
    ///
    /// # Errors
    ///
    /// Reports device loss, queue overflow or insufficient memory for the take.
    pub fn collect(&mut self) -> Result<&[[f32; 2]]> {
        if let Some(e) = self.error() {
            return Err(e.into());
        }
        let count = self.rx.slots();
        ensure!(
            count < self.rx.buffer().capacity(),
            "The recording device queue overflowed"
        );
        if count > 0 {
            self.frames.try_reserve(count)?;
            self.frames.extend(self.rx.read_chunk(count)?);
        }
        Ok(&self.frames)
    }

    /// Discard analyzed frames from the beginning of a take, keeping the active input stream open.
    /// Returns the number removed. Samples still in the device queue are unaffected.
    pub fn discard_prefix(&mut self, count: usize) -> usize {
        let count = count.min(self.frames.len());
        self.frames.drain(..count);
        count
    }

    /// Sample rate of the incoming PCM frames.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Take a device error reported during capture, if any.
    pub fn error(&self) -> Option<cpal::Error> {
        self.errors.try_recv().ok()
    }

    /// Stop capture and retain the clip for repeated playback.
    ///
    /// # Errors
    ///
    /// Returns an error if capture failed or the device delivered no frames.
    pub fn finish(mut self) -> Result<Clip> {
        self.collect()?;
        drop(self.stream);
        let count = self.rx.slots();
        if count > 0 {
            self.frames.extend(self.rx.read_chunk(count)?);
        }
        ensure!(
            !self.frames.is_empty(),
            "The microphone returned no audio frames"
        );
        Ok(Clip {
            frames: self.frames.into(),
            rate: self.rate,
        })
    }
}

fn capture<T: SizedSample>(
    device: &Device,
    format: &SupportedStreamConfig,
    mut tx: Producer<Frame>,
    error: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<Stream, cpal::Error>
where
    f32: FromSample<T>,
{
    let channels = usize::from(format.channels());
    device.build_input_stream(
        stream_config(format),
        move |data: &[T], _| {
            for frame in data.chunks_exact(channels).take(tx.slots()) {
                let left = frame[0].to_sample();
                let right = frame.get(1).map_or(left, |s| s.to_sample());
                let _ = tx.push([left, right]);
            }
        },
        error,
        None,
    )
}

/// Stereo audio from a microphone test, stored at its original capture rate.
pub struct Clip {
    frames: Arc<[Frame]>,
    rate: u32,
}

impl Clip {
    /// Estimate a suppression limit from a guided physical microphone take.
    ///
    /// Uses the live input rate converter for devices recording below 48 kHz.
    ///
    /// # Errors
    ///
    /// Returns a sample error when the recording cannot support a recommendation.
    pub fn recommend(
        &self,
        phrases: [&Self; 2],
        mixing: Mixing,
    ) -> Result<Recommendation, SampleError> {
        let room = self.model_frames();
        let [first, second] = phrases.map(Self::model_frames);
        calibration::recommend(&room, [&first, &second], mixing)
    }

    /// Build current and recommended headphone previews from the same neutral receiver recording.
    /// The room segment precedes the reading so low-frequency noise remains audible in the comparison.
    ///
    /// # Errors
    ///
    /// Returns a sample error for incomplete, clipped, or indistinct speech.
    pub fn tone_previews(
        &self,
        voice: &Self,
        current: orvia_core::Tone,
    ) -> Result<(orvia_core::Tone, [Self; 2]), SampleError> {
        let room = self.model_frames();
        let voice = voice.model_frames();
        // Keep the strongest room sound near the start of each preview even when the user waits before reading.
        let room_window = 2 * SAMPLE_RATE as usize;
        let loudest = room
            .chunks(orvia_core::BLOCK)
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                let energy =
                    |block: &[[f32; 2]]| block.iter().flatten().map(|s| s * s).sum::<f32>();
                energy(a).total_cmp(&energy(b))
            })
            .map_or(0, |(i, _)| i * orvia_core::BLOCK);
        let start = loudest
            .saturating_sub(room_window / 2)
            .min(room.len().saturating_sub(room_window));
        let room = &room[start..(start + room_window).min(room.len())];
        let recommended = orvia_core::tone::recommend(room, &voice)?;
        let clips = [current, recommended].map(|tone| {
            let mut processor = orvia_core::tone::ToneProcessor::new(tone);
            let mut frames = room.to_vec();
            frames.extend(std::iter::repeat_n([0.; 2], SAMPLE_RATE as usize / 4));
            frames.extend_from_slice(&voice);
            for block in frames.chunks_mut(orvia_core::BLOCK) {
                processor.process(block);
            }
            Self {
                frames: frames.into(),
                rate: SAMPLE_RATE,
            }
        });
        Ok((recommended, clips))
    }

    fn model_frames(&self) -> std::borrow::Cow<'_, [Frame]> {
        if self.rate == SAMPLE_RATE {
            return (&self.frames[..]).into();
        }
        let mut rate = InputRate::new(self.rate);
        let mut frames = Vec::new();
        for &frame in self.frames.iter() {
            rate.push(frame, |f| frames.push(f));
        }
        frames.into()
    }

    /// Store the current take in the microphone test directory, creating it if necessary.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be created or the WAV cannot be saved.
    pub fn save_latest(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        self.save(&dir.join(RECORDING_FILE))
    }

    /// Load the current take, or the newest before/after recording from an earlier version.
    ///
    /// Returns `None` when the user has never recorded a microphone test.
    ///
    /// # Errors
    ///
    /// Returns an error if the saved recording cannot be located or decoded.
    pub fn load_latest(dir: &Path) -> Result<Option<Self>> {
        let current = dir.join(RECORDING_FILE);
        if current.try_exists()? {
            return Self::load(&current).map(Some);
        }
        let mut previous = Vec::new();
        for name in ["before.wav", "after.wav"] {
            let path = dir.join(name);
            if path.try_exists()? {
                previous.push((path.metadata()?.modified()?, path));
            }
        }
        previous
            .into_iter()
            .max_by_key(|(time, _)| *time)
            .map(|(_, path)| Self::load(&path))
            .transpose()
    }

    /// Atomically save a stereo float WAV. A failed retake preserves the previous file.
    ///
    /// # Errors
    ///
    /// Returns a filesystem or WAV encoding error. The parent directory must exist.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp =
            tempfile::NamedTempFile::new_in(path.parent().context("Missing recording directory")?)?;
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: self.rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::new(BufWriter::new(tmp.as_file()), spec)?;
        for sample in self.frames.iter().flatten() {
            writer.write_sample(*sample)?;
        }
        writer.finalize()?;
        tmp.persist(path)?;
        Ok(())
    }

    /// Open a test WAV saved by this or a previous installation.
    ///
    /// # Errors
    ///
    /// Returns an error for unreadable, malformed, truncated or unsupported recordings.
    pub fn load(path: &Path) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        ensure!(
            spec.channels == 2
                && spec.sample_format == hound::SampleFormat::Float
                && spec.bits_per_sample == 32
                && (8_000..=48_000).contains(&spec.sample_rate)
                && reader.duration() > 0
                && u64::from(reader.duration()) * 8 <= path.metadata()?.len(),
            "Unsupported microphone test WAV"
        );
        let mut frames = Vec::new();
        frames.try_reserve_exact(reader.duration() as usize)?;
        let mut samples = reader.samples::<f32>();
        while let Some(left) = samples.next() {
            frames.push([left?, samples.next().context("Missing right channel")??]);
        }
        Ok(Self {
            frames: frames.into(),
            rate: spec.sample_rate,
        })
    }

    /// Duration of the captured clip.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames.len() as f64 / f64::from(self.rate))
    }

    /// Replay on a physical device, or on the system default output when omitted.
    ///
    /// # Errors
    ///
    /// Returns an error if the selected device is virtual, unavailable or incompatible.
    pub fn play(&self, id: Option<&str>) -> Result<Playback> {
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
            frames: self.frames.clone(),
            phase: 0,
            input_rate: self.rate,
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
            done,
            errors,
        })
    }
}

struct Cursor {
    frames: Arc<[Frame]>,
    phase: u64,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
}

impl Cursor {
    fn fill<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], done: &AtomicBool) {
        out.fill(T::EQUILIBRIUM);
        let rate = u64::from(self.output_rate);
        // Keep the stream alive through the callback that consumes the last sample.
        if self.phase / rate >= self.frames.len() as u64 {
            done.store(true, Ordering::Relaxed);
            return;
        }
        for frame in out.chunks_exact_mut(self.channels) {
            let index = (self.phase / rate) as usize;
            let Some(left) = self.frames.get(index) else {
                break;
            };
            let right = self.frames.get(index + 1).unwrap_or(left);
            let fraction = (self.phase % rate) as f32 / self.output_rate as f32;
            let sample = |ch: usize| left[ch] + (right[ch] - left[ch]) * fraction;
            if self.channels == 1 {
                frame[0] = T::from_sample((sample(0) + sample(1)) * 0.5);
            } else {
                frame[0] = T::from_sample(sample(0));
                frame[1] = T::from_sample(sample(1));
            }
            self.phase += u64::from(self.input_rate);
        }
    }
}

/// Holds the physical output stream. Dropping it stops the preview.
pub struct Playback {
    _stream: Stream,
    done: Arc<AtomicBool>,
    errors: mpsc::Receiver<cpal::Error>,
}

impl Playback {
    /// Return whether the clip has ended or the playback device failed.
    ///
    /// # Errors
    ///
    /// Returns an error if the audio backend reports device loss.
    pub fn finished(&self) -> Result<bool> {
        if let Ok(error) = self.errors.try_recv() {
            return Err(error.into());
        }
        Ok(self.done.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_previews_use_the_same_receiver_take_and_the_live_processor() {
        let room = Clip {
            rate: SAMPLE_RATE,
            frames: vec![[0.; 2]; orvia_core::BLOCK * 75].into(),
        };
        let voice = Clip {
            rate: SAMPLE_RATE,
            frames: (0..SAMPLE_RATE as usize)
                .map(|i| {
                    let x = if (9000..36000).contains(&i) {
                        0.05 * (i as f32 * 0.03).sin()
                    } else {
                        0.
                    };
                    [x, x * 0.6]
                })
                .collect(),
        };
        let (settings, [before, after]) = room.tone_previews(&voice, Default::default()).unwrap();
        assert_eq!(
            &before.frames[before.frames.len() - voice.frames.len()..],
            voice.frames.as_ref()
        );
        let mut reference = before.frames.to_vec();
        let mut live = orvia_core::Processor::new(&orvia_core::Settings {
            suppress_noise: false,
            low_cut_hz: settings.low_cut_hz,
            gain_percent: settings.gain_percent,
            compressor_ratio: settings.compressor_ratio,
            ..Default::default()
        });
        for block in reference.chunks_exact_mut(orvia_core::BLOCK) {
            live.process(block.try_into().unwrap());
        }
        assert_eq!(reference.as_slice(), after.frames.as_ref());
    }

    #[test]
    fn retaking_preserves_older_recordings_and_reopens_the_current_take() {
        let dir = tempfile::tempdir().unwrap();
        let before = dir.path().join("before.wav");
        let after = dir.path().join("after.wav");
        let mut clip = Clip {
            frames: Arc::from([[0.4, 0.3], [0., 0.]]),
            rate: 44_100,
        };
        assert!(Clip::load_latest(dir.path()).unwrap().is_none());
        clip.save(&before).unwrap();
        assert_eq!(
            Clip::load_latest(dir.path()).unwrap().unwrap().frames,
            clip.frames
        );
        clip.frames = Arc::from([[0.125, -0.125]]);
        clip.save(&after).unwrap();
        assert_eq!(
            Clip::load_latest(dir.path()).unwrap().unwrap().frames,
            clip.frames
        );
        std::fs::File::options()
            .write(true)
            .open(&after)
            .unwrap()
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();
        let original = Clip::load(&before).unwrap();
        assert_eq!(original.rate, 44_100);
        assert_eq!(
            Clip::load_latest(dir.path()).unwrap().unwrap().frames,
            original.frames
        );
        let previous = clip.frames.clone();
        clip.frames = Arc::from([[0.7, -0.7]]);
        clip.save_latest(dir.path()).unwrap();
        assert_eq!(
            Clip::load_latest(dir.path()).unwrap().unwrap().frames,
            clip.frames
        );
        assert_eq!(Clip::load(&before).unwrap().frames, original.frames);
        assert_eq!(Clip::load(&after).unwrap().frames, previous);
    }

    #[test]
    fn a_slow_reading_survives_reopening_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let frames = vec![[0.01, 0.02]; 96_000];
        let clip = Clip {
            frames: frames.into(),
            rate: 8_000,
        };
        clip.save_latest(dir.path()).unwrap();
        let loaded = Clip::load_latest(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.duration(), Duration::from_secs(12));
        assert_eq!(loaded.frames, clip.frames);
    }

    #[test]
    fn replay_preserves_pauses_across_buffers_and_resamples_both_channels() {
        let mut cursor = Cursor {
            frames: Arc::from([[0.5, -0.5], [0., 0.], [0., 0.], [0.5, -0.5]]),
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
}
