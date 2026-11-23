//! Stable-release checks shared by the automatic suggestion and the manual button.

use orvia_core::{Language, read_json, write_json};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Check {
    checked_at: SystemTime,
    version: Option<Version>,
}

impl Check {
    /// Record the stable release seen by a completed check.
    pub(super) fn new(version: Option<Version>) -> Self {
        Self {
            checked_at: SystemTime::now(),
            version,
        }
    }
}

pub(super) enum CheckError {
    Network,
    Invalid,
}

/// Visual role of a short notification in the mixer.
pub(super) enum Notice {
    Progress,
    Success,
    Information,
    Error,
}

/// The last successful check survives restarts. Failed refreshes retain it.
#[derive(Default)]
pub(super) struct Updates {
    last: Option<Check>,
    error: Option<CheckError>,
    attempted: Option<Instant>,
    pub(super) checking: bool,
}

impl Updates {
    pub(super) fn load(dir: &Path) -> Self {
        Self {
            last: read_json(&dir.join("release-check.json")).ok(),
            ..Self::default()
        }
    }

    pub(super) fn due(&self) -> bool {
        let fresh = self.last.as_ref().is_some_and(|check| {
            check
                .checked_at
                .elapsed()
                .is_ok_and(|age| age < CHECK_INTERVAL)
        });
        !self.checking && !fresh && self.attempted.is_none_or(|t| t.elapsed() >= RETRY_INTERVAL)
    }

    pub(super) fn start(&mut self) {
        self.checking = true;
        self.attempted = Some(Instant::now());
        self.error = None;
    }

    pub(super) fn finish(&mut self, result: Result<Check, CheckError>) {
        self.checking = false;
        match result {
            Ok(check) => self.last = Some(check),
            Err(error) => self.error = Some(error),
        }
    }

    pub(super) fn available(&self, installed: &Version) -> Option<&Version> {
        self.last
            .as_ref()?
            .version
            .as_ref()
            .filter(|v| v.pre.is_empty() && v.cmp_precedence(installed).is_gt())
    }

    /// Describe the current manual check for the dismissible notification.
    pub(super) fn status(&self, l: Language, installed: &Version) -> Option<(Notice, String)> {
        if self.checking {
            return Some((
                Notice::Progress,
                l.text("Checking releases…", "更新を確認中…").into(),
            ));
        }
        if let Some(error) = &self.error {
            let message = match error {
                CheckError::Network => l.text(
                    "Update check failed. Check your connection.",
                    "更新を確認できません。接続を確認してください。",
                ),
                CheckError::Invalid => l.text(
                    "Unrecognized release information",
                    "リリース情報を読み取れません",
                ),
            };
            return Some((Notice::Error, message.into()));
        }
        let check = self.last.as_ref()?;
        if let Some(v) = self.available(installed) {
            return Some((
                Notice::Information,
                format!(
                    "{} · {v}",
                    l.text("Update available", "アップデートがあります")
                ),
            ));
        }
        if check.version.is_none() {
            return Some((
                Notice::Information,
                l.text("No public release available.", "公開リリースはありません。")
                    .into(),
            ));
        }
        Some((
            Notice::Success,
            l.text("Up to date", "最新バージョンです").into(),
        ))
    }
}

/// Fetch the latest stable tag and cache successful responses, including an empty release list.
pub(super) fn fetch(dir: &Path) -> Result<Check, CheckError> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent();
    let version = match agent
        .get("https://api.github.com/repos/orvia-audio/orvia/releases/latest")
        .header("User-Agent", "Orvia")
        .call()
    {
        Ok(mut response) => {
            let release = response
                .body_mut()
                .read_json::<Release>()
                .map_err(|_| CheckError::Invalid)?;
            let version = Version::parse(
                release
                    .tag_name
                    .strip_prefix('v')
                    .unwrap_or(&release.tag_name),
            )
            .map_err(|_| CheckError::Invalid)?;
            if !version.pre.is_empty() {
                return Err(CheckError::Invalid);
            }
            Some(version)
        }
        Err(ureq::Error::StatusCode(404)) => None,
        Err(_) => return Err(CheckError::Network),
    };
    let check = Check::new(version);
    let _ = write_json(&dir.join("release-check.json"), &check);
    Ok(check)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_newer_stable_versions_are_suggested() {
        let installed = Version::new(0, 1, 1);
        for (tag, expected) in [
            ("0.1.0", false),
            ("0.1.1+build.2", false),
            ("0.1.2-snapshot.1", false),
            ("0.1.2", true),
        ] {
            let mut updates = Updates::default();
            updates.finish(Ok(Check {
                checked_at: SystemTime::now(),
                version: Some(Version::parse(tag).unwrap()),
            }));
            assert_eq!(updates.available(&installed).is_some(), expected, "{tag}");
        }
    }

    #[test]
    fn pending_requests_and_fresh_results_suppress_automatic_checks() {
        let mut updates = Updates::default();
        assert!(updates.due());
        updates.start();
        assert!(!updates.due());
        updates.finish(Err(CheckError::Network));
        assert!(!updates.due());
        let cached = Updates {
            last: Some(Check {
                checked_at: SystemTime::now(),
                version: None,
            }),
            ..Updates::default()
        };
        assert!(!cached.due());
        let mut expired = Updates::default();
        expired.finish(Ok(Check {
            checked_at: SystemTime::now() - CHECK_INTERVAL,
            version: Some(Version::new(1, 0, 0)),
        }));
        assert!(expired.due());
        expired.start();
        expired.finish(Err(CheckError::Network));
        assert!(expired.available(&Version::new(0, 1, 1)).is_some());
    }
}
