use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Display language for the desktop window and notification-area menu.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// Follow the user's Windows language.
    #[default]
    System,
    /// English interface.
    English,
    /// Japanese interface.
    Japanese,
}

impl Language {
    /// Choose a translation, falling back to English for other system locales.
    pub fn text<'a>(self, english: &'a str, japanese: &'a str) -> &'a str {
        static JAPANESE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let use_japanese = match self {
            Self::English => false,
            Self::Japanese => true,
            Self::System => *JAPANESE.get_or_init(|| {
                sys_locale::get_locale().is_some_and(|s| {
                    s.split(['-', '_'])
                        .next()
                        .is_some_and(|s| s.eq_ignore_ascii_case("ja"))
                })
            }),
        };
        if use_japanese { japanese } else { english }
    }
}

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
    /// Stable ID of the physical playback device for monitoring processed audio.
    pub monitor: Option<String>,
    /// Enable headphone monitoring. An explicit device is always required.
    pub monitor_enabled: bool,
    /// Language shared by settings and tray controls.
    pub language: Language,
    /// Channel routing independent of the denoiser.
    pub mixing: crate::Mixing,
    /// Enable the selected noise filter.
    pub suppress_noise: bool,
    /// Selected noise filter. Older settings select RNNoise.
    pub noise_method: crate::NoiseMethod,
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
            monitor: None,
            monitor_enabled: false,
            language: Language::System,
            mixing: Default::default(),
            suppress_noise: false,
            noise_method: Default::default(),
            gain_percent: 100,
            muted: false,
        }
    }
}

/// A diagnostic snapshot containing transport counters and the processed peak.
/// Audio samples are never serialized.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EngineStatus {
    /// Rolling filter envelopes for the most recent two seconds.
    #[serde(default)]
    pub signal: crate::SignalHistory,
    /// Whether the optional monitoring stream is delivering audio.
    #[serde(default)]
    pub monitoring: bool,
    /// A device diagnostic for the monitor, independent of the primary route.
    #[serde(default)]
    pub monitor_error: Option<String>,
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
    io::Write::write_all(&mut tmp, &serde_json::to_vec(value)?)?;
    tmp.persist(path)?;
    Ok(())
}

impl Settings {
    /// Whether both settings use the same microphone, virtual output and DSP.
    /// Changes to gain, monitoring and language leave these streams running.
    pub fn same_signal_path(&self, other: &Self) -> bool {
        self.input == other.input
            && self.output == other.output
            && self.mixing == other.mixing
            && self.suppress_noise == other.suppress_noise
            && (!self.suppress_noise || self.noise_method == other.noise_method)
            && self.muted == other.muted
    }

    /// Apply a settings edit while holding the per-user writer lock, shared by
    /// the tray and settings window. Returns the newly persisted preferences.
    ///
    /// # Errors
    ///
    /// Returns an error if locking, loading or persisting the settings fails.
    pub fn update(path: &Path, edit: impl FnOnce(&mut Self)) -> io::Result<Self> {
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let mut settings = Self::load(path)?;
        edit(&mut settings);
        write_json(path, &settings)?;
        Ok(settings)
    }
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
    fn monitoring_and_language_changes_preserve_the_live_route() {
        let original = Settings::default();
        let mut changed = Settings {
            gain_percent: 157,
            monitor_enabled: true,
            monitor: Some("headphones".into()),
            language: Language::Japanese,
            ..original.clone()
        };
        assert!(original.same_signal_path(&changed));
        changed.muted = true;
        assert!(!original.same_signal_path(&changed));
        assert_eq!(changed.language.text("Mute", "ミュート"), "ミュート");
    }
    #[test]
    fn old_channel_and_filter_preferences_migrate() {
        for channel in ["left", "right"] {
            let json = format!(r#"{{"mixing":"{channel}","suppress_noise":true}}"#);
            let settings: Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(settings.mixing, crate::Mixing::Average);
            assert!(settings.suppress_noise);
            assert_eq!(settings.noise_method, crate::NoiseMethod::Rnnoise);
        }
    }

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
