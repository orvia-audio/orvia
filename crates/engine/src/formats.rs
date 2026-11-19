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
