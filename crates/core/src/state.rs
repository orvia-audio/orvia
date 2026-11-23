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
    /// Follow the system language.
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
    /// Stable audio endpoint ID, preserved while the device is unplugged.
    /// An absent ID follows the system default microphone.
    pub input: Option<String>,
    /// Stable audio playback ID of the selected virtual cable. When absent,
    /// Windows can fall back to the Orvia lab adapter.
    pub output: Option<String>,
    /// Capture endpoint paired with the virtual output. An absent ID enables automatic matching.
    pub receiver: Option<String>,
    /// Stable ID of the physical playback device for monitoring processed audio.
    pub monitor: Option<String>,
    /// Enable headphone monitoring. An explicit device is always required.
    pub monitor_enabled: bool,
    /// Language shared by settings and tray controls.
    pub language: Language,
    /// Check for new stable releases while the mixer window is open.
    pub check_updates_automatically: bool,
    /// Channel routing independent of the denoiser.
    pub mixing: crate::Mixing,
    /// Enable the selected noise filter.
    pub suppress_noise: bool,
    /// Selected noise filter. New configurations use DeepFilterNet3.
    pub noise_method: crate::NoiseMethod,
    /// Maximum DeepFilterNet3 attenuation in dB. Zero passes delayed dry audio
    /// and values at or above [`Self::UNLIMITED_ATTENUATION_DB`] use full suppression.
    pub deep_filter_limit_db: u16,
    /// Linear gain as a percentage, limited to 200 at processing time.
    pub gain_percent: u16,
    /// Stereo high-pass cutoff in Hz. Zero bypasses the filter.
    pub low_cut_hz: u16,
    /// Stereo-linked compressor ratio, from unity (bypass) to 4:1.
    pub compressor_ratio: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            receiver: None,
            monitor: None,
            monitor_enabled: false,
            language: Language::System,
            check_updates_automatically: true,
            mixing: Default::default(),
            suppress_noise: true,
            noise_method: Default::default(),
            deep_filter_limit_db: Self::UNLIMITED_ATTENUATION_DB,
            gain_percent: 100,
            low_cut_hz: 0,
            compressor_ratio: 1,
        }
    }
}

/// File shared by the mixer and engine for a single bounded live audition.
pub const AUDITION_FILE: &str = "suppression-audition.json";

/// Controls temporarily overridden during a guided recording.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum AuditionTarget {
    /// Audition a DeepFilterNet3 attenuation limit.
    Suppression {
        /// Maximum attenuation in dB.
        limit_db: u16,
    },
    /// Measure the receiver with neutral tone controls.
    ToneSetup {
        /// Temporary tone settings.
        tone: crate::Tone,
    },
}

impl AuditionTarget {
    /// Apply the override to an in-memory copy of the persistent mix.
    pub fn apply(self, settings: &mut Settings) {
        match self {
            Self::Suppression { limit_db } => settings.deep_filter_limit_db = limit_db,
            Self::ToneSetup { tone } => settings.set_tone(tone),
        }
    }
}

/// Temporary live settings. The engine checks the expiry even if the mixer exits.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Audition {
    /// Timestamp identifying this request in the engine acknowledgement.
    pub started: std::time::SystemTime,
    /// Last heartbeat from the active reading session.
    pub refreshed: SystemTime,
    /// Persistent route and settings at the time of the original recording.
    pub baseline: Settings,
    /// Controls to use on the receiving endpoint.
    #[serde(flatten)]
    pub target: AuditionTarget,
}

impl Audition {
    /// Return whether this request still belongs to the current mix and recording window.
    pub fn active(&self, settings: &Settings, now: std::time::SystemTime) -> bool {
        let applicable = match self.target {
            AuditionTarget::Suppression { limit_db } => {
                self.baseline.same_audition(settings)
                    && limit_db <= Settings::UNLIMITED_ATTENUATION_DB
            }
            AuditionTarget::ToneSetup { tone } => {
                self.baseline.same_mix(settings)
                    && tone == crate::Tone::default()
                    && settings.gain_percent > 0
            }
        };
        applicable
            && self.refreshed >= self.started
            && now
                .duration_since(self.refreshed)
                .is_ok_and(|age| age < std::time::Duration::from_secs(14))
    }
}

/// A diagnostic snapshot containing transport counters and the processed peak.
/// Audio samples are never serialized.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EngineStatus {
    /// Whether this engine accepts separate observer heartbeats for each settings window.
    #[serde(default)]
    pub window_observers: bool,
    /// Rolling filter envelopes for the most recent two seconds.
    #[serde(default)]
    pub signal: crate::SignalHistory,
    /// Frequency bands of the processed output when the spectrum view is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<crate::spectrum::Spectrum>,
    /// Whether the optional monitoring stream is delivering audio.
    #[serde(default)]
    pub monitoring: bool,
    /// A device diagnostic for the monitor, independent of the primary route.
    #[serde(default)]
    pub monitor_error: Option<String>,
    /// Start time of the temporary processing override acknowledged on this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audition: Option<SystemTime>,
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
/// Returns an error when the platform's per-user data location is unavailable.
pub fn data_dir() -> io::Result<PathBuf> {
    fn absolute(name: &str) -> Option<PathBuf> {
        env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    }

    #[cfg(windows)]
    let dir = absolute("LOCALAPPDATA").map(|p| p.join("Orvia"));
    #[cfg(target_os = "macos")]
    let dir = absolute("HOME").map(|p| p.join("Library/Application Support/Orvia"));
    #[cfg(target_os = "linux")]
    let dir = absolute("XDG_DATA_HOME")
        .or_else(|| absolute("HOME").map(|p| p.join(".local/share")))
        .map(|p| p.join("orvia"));
    dir.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "User data directory is unavailable",
        )
    })
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
    /// DeepFilterNet3's sentinel for unrestricted attenuation, also the legacy default.
    pub const UNLIMITED_ATTENUATION_DB: u16 = 100;

    /// Whether the same microphone recording and suppression audition remain applicable.
    pub fn same_audition(&self, other: &Self) -> bool {
        self.same_mix(other)
            && self.active_noise_method() == Some(crate::NoiseMethod::DeepFilterNet3)
    }

    /// Whether a recording still represents the current audio route and processing.
    pub fn same_mix(&self, other: &Self) -> bool {
        self.same_signal_path(other)
            && self.receiver == other.receiver
            && self.mixing == other.mixing
            && self.tone() == other.tone()
            && self.active_noise_method() == other.active_noise_method()
            && (self.active_noise_method() != Some(crate::NoiseMethod::DeepFilterNet3)
                || self.deep_filter_limit_db == other.deep_filter_limit_db)
    }

    /// Read the effective tone settings, including gain saved by earlier releases.
    pub fn tone(&self) -> crate::Tone {
        crate::Tone {
            low_cut_hz: self.low_cut_hz,
            gain_percent: self.gain_percent,
            compressor_ratio: self.compressor_ratio,
        }
        .bounded()
    }

    /// Replace the three tone settings together.
    pub fn set_tone(&mut self, tone: crate::Tone) {
        let tone = tone.bounded();
        self.low_cut_hz = tone.low_cut_hz;
        self.gain_percent = tone.gain_percent;
        self.compressor_ratio = tone.compressor_ratio;
    }

    /// Whether the open audio streams can be reused. Processing controls are
    /// applied at a block boundary without clearing the signal history.
    pub fn same_signal_path(&self, other: &Self) -> bool {
        self.input == other.input && self.output == other.output
    }

    /// The filter currently applied to the signal, or bypass when disabled.
    pub fn active_noise_method(&self) -> Option<crate::NoiseMethod> {
        self.suppress_noise.then_some(self.noise_method)
    }

    /// Apply a settings edit while holding the per-user writer lock, shared by
    /// settings writers. Returns the newly persisted preferences.
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
    fn auditions_expire_and_only_apply_to_the_recorded_mix() {
        let now = SystemTime::now();
        let request = Audition {
            started: now,
            refreshed: now,
            baseline: Settings::default(),
            target: AuditionTarget::Suppression { limit_db: 6 },
        };
        let settings = &request.baseline;
        assert!(request.active(settings, now));
        assert!(!request.active(settings, now + std::time::Duration::from_secs(14)));
        assert!(!request.active(settings, now - std::time::Duration::from_secs(1)));
        let changed = Settings {
            output: Some("other-cable".into()),
            ..settings.clone()
        };
        assert!(!request.active(&changed, now));
        let changed = Settings {
            deep_filter_limit_db: 3,
            ..settings.clone()
        };
        assert!(!request.active(&changed, now));
    }

    #[test]
    fn tone_setup_restores_neutral_processing_without_changing_suppression() {
        let old: Settings =
            serde_json::from_str(r#"{"gain_percent":73,"suppress_noise":false}"#).unwrap();
        assert_eq!(
            old.tone(),
            crate::Tone {
                gain_percent: 73,
                ..Default::default()
            }
        );
        let now = SystemTime::now();
        let request = Audition {
            started: now,
            refreshed: now,
            baseline: old.clone(),
            target: AuditionTarget::ToneSetup {
                tone: crate::Tone::default(),
            },
        };
        let restored: Audition =
            serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert!(restored.active(&old, now));
        let mut live = old.clone();
        restored.target.apply(&mut live);
        assert_eq!(live.tone(), crate::Tone::default());
        assert_eq!(live.active_noise_method(), old.active_noise_method());
        assert!(!restored.active(&live, now));
        assert!(!restored.active(&old, now + std::time::Duration::from_secs(14)));
    }

    #[test]
    fn active_readers_extend_the_audition_while_abandoned_requests_expire() {
        let start = SystemTime::now();
        let now = start + std::time::Duration::from_secs(90);
        let request = Audition {
            started: start,
            refreshed: now,
            baseline: Settings::default(),
            target: AuditionTarget::Suppression { limit_db: 9 },
        };
        assert!(request.active(&request.baseline, now));
        assert!(!request.active(&request.baseline, now + std::time::Duration::from_secs(14)));
    }

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
        changed.output = Some("virtual-output".into());
        assert!(!original.same_signal_path(&changed));
        assert_eq!(changed.language.text("Mixer", "ミキサー"), "ミキサー");
    }

    #[test]
    fn switching_between_filters_and_bypass_keeps_the_capture_stream() {
        let mut filtered = Settings::default();
        let bypass = Settings {
            suppress_noise: false,
            ..filtered.clone()
        };
        assert!(bypass.same_signal_path(&filtered));
        let first = filtered.clone();
        filtered.noise_method = crate::NoiseMethod::WebRtc;
        assert!(first.same_signal_path(&filtered));
        assert_ne!(first.active_noise_method(), filtered.active_noise_method());
        assert!(filtered.same_signal_path(&bypass));
    }
    #[test]
    fn old_channel_preferences_migrate() {
        for channel in ["left", "right"] {
            let json = format!(r#"{{"mixing":"{channel}","suppress_noise":true}}"#);
            let settings: Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(settings.mixing, crate::Mixing::Average);
            assert!(settings.suppress_noise);
        }
    }

    #[test]
    fn old_preferences_keep_unlimited_suppression_and_remember_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{}").unwrap();
        assert_eq!(
            Settings::load(&path).unwrap().deep_filter_limit_db,
            Settings::UNLIMITED_ATTENUATION_DB
        );
        Settings::update(&path, |s| s.deep_filter_limit_db = 12).unwrap();
        assert_eq!(Settings::load(&path).unwrap().deep_filter_limit_db, 12);
    }

    #[test]
    fn automatic_update_checks_default_on_and_remember_an_opt_out() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(old.check_updates_automatically);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        Settings::update(&path, |s| s.check_updates_automatically = false).unwrap();
        let saved = Settings::load(&path).unwrap();
        assert!(!saved.check_updates_automatically);
        assert!(old.same_signal_path(&saved));
    }

    #[test]
    fn new_preferences_enable_deepfilternet_and_saved_choices_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let fresh = Settings::load(&path).unwrap();
        assert_eq!(
            fresh.active_noise_method(),
            Some(crate::NoiseMethod::DeepFilterNet3)
        );
        for method in [
            None,
            Some(crate::NoiseMethod::Rnnoise),
            Some(crate::NoiseMethod::WebRtc),
        ] {
            Settings::update(&path, |s| {
                s.suppress_noise = method.is_some();
                if let Some(method) = method {
                    s.noise_method = method;
                }
            })
            .unwrap();
            assert_eq!(Settings::load(&path).unwrap().active_noise_method(), method);
        }
    }

    #[test]
    fn a_saved_spectral_selection_uses_webrtc() {
        let saved: Settings =
            serde_json::from_str(r#"{"noise_method":"spectral","suppress_noise":true}"#).unwrap();
        assert_eq!(
            saved.active_noise_method(),
            Some(crate::NoiseMethod::WebRtc)
        );
    }

    #[test]
    fn settings_ignore_unknown_fields_and_report_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        fs::write(&path, br#"{"obsolete_setting":true,"gain_percent":139}"#).unwrap();
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.output, None);
        assert_eq!(settings.gain_percent, 139);
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
