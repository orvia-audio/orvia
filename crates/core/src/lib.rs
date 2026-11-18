//! Fixed-rate audio processing and per-user settings for Orvia.
//!
//! Create a fresh processor whenever the microphone or processing options change.
//!
//! ~~~
//! use orvia_core::{BLOCK, Mixing, Processor, Settings};
//! let settings = Settings { mixing: Mixing::Left, ..Settings::default() };
//! let mut audio = [[0.25, -0.5]; BLOCK];
//! assert_eq!(Processor::new(&settings).process(&mut audio), 0.25);
//! assert_eq!(audio[0], [0.25; 2]);
//! ~~~

mod audio;
mod state;

pub use audio::{BLOCK, Mixing, Processor, SAMPLE_RATE};
pub use state::{EngineStatus, Settings, data_dir, read_json, write_json};

/// Adapter friendly name for the stereo, 48 kHz WASAPI transport.
/// Incompatible wire formats must use a different transport identity.
pub const ADAPTER: &str = "Orvia Audio v1";
