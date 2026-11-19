//! Short recordings from the receiving side of a virtual microphone.
use crate::{
    formats::{Port, config, stream_config},
    host, is_virtual_cable, label,
};
use anyhow::{Context, Result, anyhow, ensure};
use cpal::{
    Device, FromSample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
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

/// Maximum length of a microphone test clip.
pub const LIMIT: Duration = Duration::from_secs(10);
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

fn errors() -> (
    impl FnMut(cpal::Error) + Send + 'static,
    mpsc::Receiver<cpal::Error>,
) {
    let (tx, rx) = mpsc::sync_channel(1);
    (
        move |error: cpal::Error| {
            if error.kind() != cpal::ErrorKind::Xrun {
                let _ = tx.try_send(error);
            }
        },
        rx,
    )
}

/// An active, length-limited virtual microphone recording. Dropping it stops capture.
pub struct Recording {
    stream: Stream,
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
        let format = config(&device, Port::Recording)?;
        let rate = format.sample_rate();
        let (tx, rx) = RingBuffer::new(rate as usize * LIMIT.as_secs() as usize);
        let (error, errors) = errors();
        let stream = match format.sample_format() {
            SampleFormat::F32 => capture::<f32>(&device, &format, tx, error),
            SampleFormat::I16 => capture::<i16>(&device, &format, tx, error),
            other => return Err(anyhow!("Unsupported recording format: {other}")),
        }?;
        stream.play()?;
        Ok(Self {
            stream,
            rx,
            errors,
            rate,
        })
    }

    /// Duration of the frames received so far, excluding device startup time.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.rx.slots() as f64 / f64::from(self.rate))
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
        drop(self.stream);
        if let Ok(error) = self.errors.try_recv() {
            return Err(error.into());
        }
        let count = self.rx.slots();
        ensure!(count > 0, "The virtual microphone returned no audio frames");
        let frames = self
            .rx
            .read_chunk(count)?
            .into_iter()
            .collect::<Vec<_>>()
            .into();
        Ok(Clip {
            frames,
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
    /// Returns an error for unreadable, malformed, or oversized recordings.
    pub fn load(path: &Path) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        ensure!(
            spec.channels == 2
                && spec.sample_format == hound::SampleFormat::Float
                && spec.bits_per_sample == 32
                && (8_000..=48_000).contains(&spec.sample_rate)
                && reader.duration() > 0
                && u64::from(reader.duration()) <= u64::from(spec.sample_rate) * LIMIT.as_secs(),
            "Unsupported microphone test WAV"
        );
        let mut frames = Vec::with_capacity(reader.duration() as usize);
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
    fn saved_comparisons_survive_reopening_and_retaking_one_clip() {
        let dir = tempfile::tempdir().unwrap();
        let before = dir.path().join("before.wav");
        let after = dir.path().join("after.wav");
        let mut clip = Clip {
            frames: Arc::from([[0.4, 0.3], [0., 0.]]),
            rate: 44_100,
        };
        clip.save(&before).unwrap();
        clip.save(&after).unwrap();
        clip.frames = Arc::from([[0.125, -0.125]]);
        clip.save(&after).unwrap();
        let original = Clip::load(&before).unwrap();
        assert_eq!(original.rate, 44_100);
        assert_eq!(&*original.frames, &[[0.4, 0.3], [0., 0.]]);
        assert_eq!(Clip::load(&after).unwrap().frames, clip.frames);
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
