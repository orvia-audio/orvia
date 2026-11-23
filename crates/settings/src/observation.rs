//! Window-owned observer heartbeats and waveform status polling.
use super::*;

/// Release only this window's private marker. Shared compatibility markers
/// may have other writers and retain their normal engine-side expiry.
#[derive(Default)]
pub(super) struct ObserverLease(Option<PathBuf>);

impl ObserverLease {
    pub(super) fn refresh(&mut self, dir: &Path, desired: Option<(&str, bool)>) {
        let owned = desired.and_then(|(name, legacy_spectrum)| {
            let path = dir.join(name);
            let _ = fs::write(&path, b"1");
            if legacy_spectrum {
                let _ = fs::write(dir.join(orvia_core::spectrum::OBSERVER_FILE), b"1");
            }
            (name != "observe").then_some(path)
        });
        if self.0 != owned {
            if let Some(previous) = self.0.take() {
                let _ = fs::remove_file(previous);
            }
            self.0 = owned;
        }
    }
}

impl Drop for ObserverLease {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

pub(super) enum StatusUpdate {
    Keep,
    Replace(Option<EngineStatus>),
}

/// Only waveform reports justify retaining the last serialized snapshot.
#[derive(Default)]
pub(super) struct StatusReader(Vec<u8>);

impl StatusReader {
    pub(super) fn decode(
        &mut self,
        bytes: Option<Vec<u8>>,
        previous: Option<&EngineStatus>,
    ) -> StatusUpdate {
        let fresh = |s: &EngineStatus| {
            s.observed
                .elapsed()
                .is_ok_and(|age| age < Duration::from_secs(2))
        };
        let current = previous.is_some_and(fresh);
        if let Some(bytes) = bytes {
            if current && !self.0.is_empty() && self.0 == bytes {
                return StatusUpdate::Keep;
            }
            if let Ok(next) = serde_json::from_slice::<EngineStatus>(&bytes)
                && fresh(&next)
            {
                self.0 = if next.signal.peaks.len() >= 2 {
                    bytes
                } else {
                    Vec::new()
                };
                return StatusUpdate::Replace(Some(next));
            }
        }
        // Unreadable or malformed status must not hide a still-fresh report.
        self.0 = Vec::new();
        if current {
            StatusUpdate::Keep
        } else {
            StatusUpdate::Replace(None)
        }
    }
}

#[cfg(test)]
mod observer_tests {
    use super::*;
    use orvia_core::observer;

    #[test]
    fn changing_and_closing_views_releases_only_the_owned_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let mut main = ObserverLease::default();
        let mut test = ObserverLease::default();
        test.refresh(dir, Some((observer::MIC_TEST, false)));
        main.refresh(dir, Some((observer::WAVEFORM, false)));
        main.refresh(dir, Some((observer::SPECTRUM, false)));
        assert!(!dir.join(observer::WAVEFORM).exists());
        assert!(dir.join(observer::SPECTRUM).is_file());
        drop(test);
        assert!(!dir.join(observer::MIC_TEST).exists());
        assert!(dir.join(observer::SPECTRUM).is_file());
        main.refresh(dir, Some(("observe", true)));
        assert!(!dir.join(observer::SPECTRUM).exists());
        drop(main);
        assert!(dir.join("observe").is_file());
        assert!(dir.join(orvia_core::spectrum::OBSERVER_FILE).is_file());
    }

    #[test]
    fn waveform_reuse_requires_identical_bytes_and_a_fresh_heartbeat() {
        let mut reader = StatusReader::default();
        let mut json = serde_json::json!({
            "signal": {"peaks": [[1,2],[3,4]], "attenuation_db": null},
            "version": env!("CARGO_PKG_VERSION"), "message": "Streaming",
            "observed": std::time::SystemTime::now(), "peak": 0.2,
            "monitoring": false, "monitor_error": null, "missing_frames": 0, "dropped_frames": 0
        });
        let bytes = serde_json::to_vec(&json).unwrap();
        let StatusUpdate::Replace(Some(mut status)) = reader.decode(Some(bytes.clone()), None)
        else {
            panic!("missing initial status")
        };
        // An older schema can use the same release version as the current client.
        assert!(!status.window_observers);
        assert!(matches!(
            reader.decode(Some(bytes.clone()), Some(&status)),
            StatusUpdate::Keep
        ));
        json["window_observers"] = true.into();
        json["version"] = "2.0.0".into();
        let StatusUpdate::Replace(Some(capable)) =
            reader.decode(Some(serde_json::to_vec(&json).unwrap()), Some(&status))
        else {
            panic!("did not observe new capabilities")
        };
        assert!(capable.window_observers);
        assert!(matches!(
            reader.decode(Some(b"invalid".to_vec()), Some(&capable)),
            StatusUpdate::Keep
        ));
        assert!(reader.0.is_empty());
        status.observed = std::time::SystemTime::UNIX_EPOCH;
        let stale = serde_json::to_vec(&status).unwrap();
        reader.0.clone_from(&stale);
        assert!(matches!(
            reader.decode(Some(stale), Some(&status)),
            StatusUpdate::Replace(None)
        ));
        assert!(reader.0.is_empty());
    }
}
