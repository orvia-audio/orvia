//! Fixed-rate audio processing and per-user settings for Orvia.
//!
//! Create a fresh processor whenever the microphone or processing options change.
//!
//! ~~~
//! use orvia_core::{BLOCK, Mixing, Processor, Settings};
//! let settings = Settings { mixing: Mixing::Average, suppress_noise: false, ..Settings::default() };
//! let mut audio = [[0.25, -0.5]; BLOCK];
//! assert_eq!(Processor::new(&settings).process(&mut audio), 0.125);
//! assert_eq!(audio[0], [-0.125; 2]);
//! ~~~

mod audio;
mod events;
mod state;

pub use audio::{BLOCK, BlockLevels, Mixing, NoiseMethod, Processor, SAMPLE_RATE, SignalHistory};
pub use events::{ENGINE_LOG, EngineEvent, LogEntry};
pub use state::{EngineStatus, Language, Settings, data_dir, read_json, write_json};

/// Adapter friendly name for the stereo, 48 kHz WASAPI transport.
/// Incompatible wire formats must use a different transport identity.
pub const ADAPTER: &str = "Orvia Audio v1";
