use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Preferences shared by the settings window and engine. Missing fields take
/// their defaults when loading settings from an earlier version.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Stable WASAPI endpoint ID, preserved while the device is unplugged.
    /// An absent ID follows the Windows default microphone.
    pub input: Option<String>,
    /// Stable WASAPI playback ID of the selected virtual cable. When absent,
    /// the engine looks only for the Orvia lab adapter used by older settings.
    pub output: Option<String>,
    /// Channel routing independent of the denoiser.
    pub mixing: crate::Mixing,
    /// Enable the embedded RNNoise model.
    pub suppress_noise: bool,
    /// Linear gain as a percentage, limited to 200 at processing time.
    pub gain_percent: u16,
    /// Silence every output frame, including after a reconnect.
    pub muted: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            mixing: Default::default(),
            suppress_noise: false,
            gain_percent: 100,
            muted: false,
        }
    }
}

/// A diagnostic snapshot containing transport counters and the processed peak.
/// Audio samples are never serialized.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EngineStatus {
    /// Observation time used by the UI to detect a stalled engine.
    pub observed: SystemTime,
    /// Engine package version, independent of the driver transport version.
    pub version: String,
    /// Human-readable stream state or device error.
    pub message: String,
    /// Largest absolute processed sample since the previous observation.
    pub peak: f32,
    /// Stereo frames replaced with silence after playback started.
    pub missing_frames: u64,
    /// Excess or stale frames discarded since the stream opened.
    pub dropped_frames: u64,
}

/// Locate the current user's Orvia directory without creating it.
///
/// # Errors
///
/// Returns an error if Windows did not provide a local application data directory.
pub fn data_dir() -> io::Result<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(|p| PathBuf::from(p).join("Orvia"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable"))
}

/// Decode a settings file or diagnostic snapshot.
///
/// # Errors
///
/// Returns the underlying file error or an invalid-data error for malformed JSON.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Atomically replace a JSON file within its own directory. Concurrent readers
/// see a complete previous or new snapshot. The parent directory must exist.
///
/// # Errors
///
/// Returns an error on encoding, writing or renaming. An unsuccessful write
/// leaves an existing destination intact.
pub fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    serde_json::to_writer_pretty(&mut tmp, value)?;
    tmp.persist(path)?;
    Ok(())
}

impl Settings {
    /// Load preferences, using defaults only when the file is absent.
    ///
    /// # Errors
    ///
    /// Propagates malformed JSON and I/O errors so the engine can fail silent.
    pub fn load(path: &Path) -> io::Result<Self> {
        match read_json(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_settings_preserve_mute_and_corruption_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert!(!Settings::load(&path).unwrap().muted);
        fs::write(&path, br#"{"muted":true}"#).unwrap();
        let settings = Settings::load(&path).unwrap();
        assert!(settings.muted);
        assert_eq!(settings.output, None);
        assert_eq!(settings.gain_percent, 100);
        let selected = Settings {
            output: Some("saved-playback-id".into()),
            ..settings
        };
        write_json(&path, &selected).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), selected);
        fs::write(&path, b"{").unwrap();
        assert!(Settings::load(&path).is_err());
    }
}
