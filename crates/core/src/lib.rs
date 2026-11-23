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
pub mod calibration;
mod events;
pub mod spectrum;
mod state;
pub mod tone;
pub use tone::Tone;

pub use audio::{
    BLOCK, BlockLevels, Mixing, NoiseMethod, Processor, SAMPLE_RATE, SignalHistory, SignalWindow,
};
pub use events::{ENGINE_LOG, ENGINE_LOG_TEXT, EngineEvent, LogEntry};
pub use state::{
    AUDITION_FILE, Audition, AuditionTarget, EngineStatus, Language, Settings, data_dir, read_json,
    write_json,
};

/// Request route health and audition acknowledgements without collecting mixer waveforms.
pub const SUMMARY_OBSERVER: &str = "observe-summary";

/// Exclusive heartbeat files for the mixer and the separately launchable microphone test.
/// Older clients and external diagnostic tools use the original shared observer files.
pub mod observer {
    /// The main window is displaying the waveform.
    pub const WAVEFORM: &str = "observe-window-waveform";
    /// The main window is displaying the frequency spectrum.
    pub const SPECTRUM: &str = "observe-window-spectrum";
    /// The main window needs only engine health and route changes.
    pub const SUMMARY: &str = "observe-window-summary";
    /// The standalone mic test needs engine health and audition acknowledgments.
    pub const MIC_TEST: &str = "observe-test-summary";
}

/// Adapter friendly name for the stereo, 48 kHz WASAPI transport.
/// Incompatible wire formats must use a different transport identity.
pub const ADAPTER: &str = "Orvia Audio v1";
