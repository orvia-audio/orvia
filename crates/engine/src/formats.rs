//! Stream format selection shared by the mixer and microphone test.
use crate::label;
use anyhow::{Result, anyhow};
use cpal::{Device, SampleFormat, SupportedStreamConfig, traits::DeviceTrait};
use orvia_core::SAMPLE_RATE;

/// Whether CPAL continues delivering callbacks after this runtime notification.
pub fn stream_continues(error: &cpal::Error) -> bool {
    // CPAL 0.18's WASAPI fallback preserves the Win32 error kind when both
    // priority requests fail, then enters the normal stream loop.
    let priority_denied = cfg!(windows)
        && error.message().is_some_and(|s| {
            s.starts_with("Failed to promote audio thread to real-time priority:")
        });
    priority_denied
        || matches!(
            error.kind(),
            cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied
        )
}

/// Use a ten-millisecond server buffer on Linux.
pub fn stream_config(config: &SupportedStreamConfig) -> cpal::StreamConfig {
    let mut stream = config.config();
    if cfg!(target_os = "linux") {
        // PulseAudio's server defaults can exceed the entire 100 ms transport queue.
        stream.buffer_size = cpal::BufferSize::Fixed(stream.sample_rate / 100);
    }
    stream
}

#[derive(Clone, Copy, PartialEq)]
/// The role of an endpoint when negotiating a shared stream format.
pub enum Port {
    /// Physical input used by the resident engine.
    Microphone,
    /// Capture side of a virtual cable, opened by a receiving app.
    Recording,
    /// Playback side of a virtual cable.
    Cable,
    /// Physical device used to listen to the mix.
    Headphones,
}

/// Choose a supported mono or multichannel format close to 48 kHz.
///
/// # Errors
///
/// Returns an error if the device has no compatible shared format.
pub fn config(device: &Device, port: Port) -> Result<SupportedStreamConfig> {
    let input = matches!(port, Port::Microphone | Port::Recording);
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    ranges
        .into_iter()
        .filter(|r| {
            r.sample_format() == SampleFormat::F32
                || (port != Port::Microphone && r.sample_format() == SampleFormat::I16)
        })
        .filter_map(|r| {
            let rate = if input {
                r.max_sample_rate().min(SAMPLE_RATE)
            } else {
                SAMPLE_RATE.clamp(r.min_sample_rate(), r.max_sample_rate())
            };
            let suitable =
                rate >= 8_000 && r.channels() > 0 && (port != Port::Cable || r.channels() <= 2);
            (suitable && rate >= r.min_sample_rate()).then(|| r.with_sample_rate(rate))
        })
        .max_by_key(|c| {
            (
                std::cmp::Reverse(c.sample_rate().abs_diff(SAMPLE_RATE)),
                c.channels() == 2,
                c.sample_format() == SampleFormat::F32,
            )
        })
        .ok_or_else(|| {
            anyhow!(
                "{} has no compatible shared format. Try 48 kHz in your sound settings.",
                label(device).unwrap_or_default()
            )
        })
}

/// Convert the capture clock to the model's fixed rate. Integer phase persists
/// between callbacks, including across partial 10 ms blocks.
pub struct InputRate {
    rate: u32,
    phase: u32,
    previous: Option<[f32; 2]>,
}

impl InputRate {
    /// Begin converting a stream at the incoming sample rate.
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            phase: 0,
            previous: None,
        }
    }

    /// Convert a device batch, retaining the previous sample across callbacks.
    /// Integer upsampling ratios use the same fixed phases for every input frame.
    pub fn push_batch(
        &mut self,
        frames: impl Iterator<Item = [f32; 2]>,
        mut emit: impl FnMut([f32; 2]),
    ) {
        match self.rate {
            8_000 => self.upsample_integer::<6>(frames, emit),
            12_000 => self.upsample_integer::<4>(frames, emit),
            16_000 => self.upsample_integer::<3>(frames, emit),
            24_000 => self.upsample_integer::<2>(frames, emit),
            SAMPLE_RATE => frames.for_each(emit),
            _ => frames.for_each(|frame| self.push(frame, &mut emit)),
        }
    }

    fn upsample_integer<const N: usize>(
        &mut self,
        frames: impl Iterator<Item = [f32; 2]>,
        mut emit: impl FnMut([f32; 2]),
    ) {
        let phases: [f32; N] = std::array::from_fn(|i| {
            (i as u32 * (SAMPLE_RATE / N as u32)) as f32 / SAMPLE_RATE as f32
        });
        for frame in frames {
            let previous = self.previous.replace(frame).unwrap_or(frame);
            let delta = [frame[0] - previous[0], frame[1] - previous[1]];
            for phase in phases {
                emit([
                    previous[0] + delta[0] * phase,
                    previous[1] + delta[1] * phase,
                ]);
            }
        }
    }

    /// Emit the 48 kHz samples covered by one incoming stereo frame.
    pub fn push(&mut self, frame: [f32; 2], mut emit: impl FnMut([f32; 2])) {
        if self.rate == SAMPLE_RATE {
            emit(frame);
            return;
        }
        let previous = self.previous.replace(frame).unwrap_or(frame);
        while self.phase < SAMPLE_RATE {
            let ratio = self.phase as f32 / SAMPLE_RATE as f32;
            emit(std::array::from_fn(|c| {
                previous[c] + (frame[c] - previous[c]) * ratio
            }));
            self.phase += self.rate;
        }
        self.phase -= SAMPLE_RATE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_preserve_the_sample_clock_across_splits() {
        let input = [
            [-0.0, 0.0],
            [0.123, -0.75],
            [f32::from_bits(3), -1.0],
            [0.5, -0.0],
        ];
        for hz in [8_000, 12_000, 16_000, 24_000, 44_100, 48_000, 96_000] {
            let mut reference = InputRate::new(hz);
            let mut expected = Vec::new();
            for frame in input {
                reference.push(frame, |s| expected.push(s.map(f32::to_bits)));
            }
            for split in 0..=input.len() {
                let mut clock = InputRate::new(hz);
                let mut actual = Vec::new();
                for batch in [&input[..split], &input[split..]] {
                    clock.push_batch(batch.iter().copied(), |s| actual.push(s.map(f32::to_bits)));
                }
                assert_eq!(actual, expected, "{hz} Hz split at {split}");
            }
        }
    }
}
