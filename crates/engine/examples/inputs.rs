//! List the available microphone names and their default Windows shared formats.
//! This diagnostic opens no audio streams and writes no audio data.
use cpal::traits::{DeviceTrait, HostTrait};
fn main() -> anyhow::Result<()> {
    for device in cpal::default_host().input_devices()? {
        let config = device.default_input_config()?;
        println!(
            "{} · {} channel(s) · {} Hz · {}",
            orvia_engine::label(&device)?,
            config.channels(),
            config.sample_rate(),
            config.sample_format()
        );
    }
    Ok(())
}
