//! Persisted audio engine events shared with the live log viewer.
use serde::{Deserialize, Serialize};

/// Filename of the per-user event history, written by the resident engine.
pub const ENGINE_LOG: &str = "engine-log.json";

/// One change in engine state. Entries contain device labels and diagnostics.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogEntry {
    /// Wall-clock time with its UTC offset, captured when the event occurred.
    pub timestamp: String,
    /// What changed in the engine.
    pub event: EngineEvent,
}

/// Runtime changes retained across audio engine restarts.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineEvent {
    /// The engine acquired its lock and started with this package version.
    Started(String),
    /// The engine received a shutdown request from Preferences or the tray.
    Stopped,
    /// The named microphone and output streams opened successfully.
    Streaming(String),
    /// The current stream stopped delivering audio or the default input changed.
    Reconnecting,
    /// An error prevented routing. The message includes its underlying cause.
    RouteError(String),
    /// Filter identifier at the time of the event, or bypass. Historical entries
    /// can name a filter that is no longer available.
    Noise(Option<String>),
    /// The headphone stream opened or closed.
    Monitoring(bool),
    /// Headphone playback failed independently of the main output.
    MonitorError(String),
    /// A stored event that this version cannot describe.
    #[serde(other)]
    Unknown,
}
