//! File requests and observer heartbeats consumed by the audio supervisor.

use orvia_core::{AUDITION_FILE, SUMMARY_OBSERVER, spectrum::OBSERVER_FILE};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// The control requests visible during one supervisor tick.
#[derive(Default)]
pub(super) struct Requests {
    pub stop: bool,
    pub metering: bool,
    pub spectrum: bool,
    pub summary: bool,
    /// An audition may be present. Its contents and expiry must still be checked on the route.
    check_audition: bool,
}

impl Requests {
    const ALL: Self = Self {
        stop: true,
        metering: true,
        spectrum: true,
        summary: true,
        check_audition: true,
    };
}

pub(super) struct ControlFiles {
    pub stop: PathBuf,
    audition: PathBuf,
    mixer: PathBuf,
    spectrum: PathBuf,
    summary: PathBuf,
    #[cfg(windows)]
    dir: PathBuf,
    #[cfg(windows)]
    retry_scan: Option<std::time::Instant>,
}

impl ControlFiles {
    pub(super) fn new(dir: &Path) -> Self {
        Self {
            stop: super::stop_request(dir),
            audition: dir.join(AUDITION_FILE),
            mixer: dir.join("observe"),
            spectrum: dir.join(OBSERVER_FILE),
            summary: dir.join(SUMMARY_OBSERVER),
            #[cfg(windows)]
            dir: dir.to_owned(),
            #[cfg(windows)]
            retry_scan: None,
        }
    }

    /// Read the current file so a cancelled or replaced audition is never reused.
    pub(super) fn audition(&self, requests: &Requests) -> Option<orvia_core::Audition> {
        if requests.check_audition {
            orvia_core::read_json(&self.audition).ok()
        } else {
            None
        }
    }

    /// Read the current requests, falling back to the named paths if enumeration is unsuitable.
    pub(super) fn poll(&mut self) -> Requests {
        #[cfg(windows)]
        let mut requests = self.present();
        #[cfg(not(windows))]
        let mut requests = Requests::ALL;
        requests.stop = requests.stop && self.stop.exists();
        if !requests.stop {
            // NTFS directory entries can retain old timestamps when another hard link is
            // written. Query the actual paths for present heartbeats, also following symlinks.
            requests.metering = requests.metering && recently_observed(&self.mixer);
            requests.spectrum = requests.spectrum && recently_observed(&self.spectrum);
            requests.summary = requests.summary
                && !requests.metering
                && !requests.spectrum
                && recently_observed(&self.summary);
        }
        requests
    }

    #[cfg(windows)]
    fn present(&mut self) -> Requests {
        if self
            .retry_scan
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(60))
        {
            if let Some(requests) = self.scan() {
                // With all three heartbeat files present we already need the named metadata
                // queries. Avoid repeatedly listing the directory on top of those probes.
                self.retry_scan = (requests.metering && requests.spectrum && requests.summary)
                    .then(std::time::Instant::now);
                return requests;
            }
            self.retry_scan = Some(std::time::Instant::now());
        }
        Requests::ALL
    }

    #[cfg(windows)]
    fn scan(&self) -> Option<Requests> {
        let mut requests = Requests::default();
        let stop = self.stop.file_name()?.to_str()?;
        for (i, entry) in fs::read_dir(&self.dir).ok()?.enumerate() {
            // A custom data directory can contain arbitrarily many unrelated files.
            if i >= 64 {
                return None;
            }
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            match name {
                "observe" => requests.metering = true,
                OBSERVER_FILE => requests.spectrum = true,
                SUMMARY_OBSERVER => requests.summary = true,
                AUDITION_FILE => requests.check_audition = true,
                n if n == stop => requests.stop = true,
                // Let the filesystem resolve other spellings on case-sensitive and
                // case-insensitive directories, including Unicode case mappings.
                n if !n.is_ascii()
                    || [
                        stop,
                        "observe",
                        OBSERVER_FILE,
                        SUMMARY_OBSERVER,
                        AUDITION_FILE,
                    ]
                    .iter()
                    .any(|key| key.eq_ignore_ascii_case(n)) =>
                {
                    return None;
                }
                _ => {}
            }
        }
        Some(requests)
    }
}

fn recently_observed(path: &Path) -> bool {
    fresh(fs::metadata(path).and_then(|m| m.modified()).ok())
}

fn fresh(modified: Option<SystemTime>) -> bool {
    modified
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orvia_core::{Audition, AuditionTarget, Settings, write_json};

    #[test]
    fn heartbeats_follow_file_times_and_stop_requests_are_process_specific() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = ControlFiles::new(dir.path());
        fs::write(dir.path().join("stop-engine-other"), b"1").unwrap();
        let now = SystemTime::now();
        fs::File::create(&files.mixer)
            .unwrap()
            .set_modified(now - Duration::from_secs(10))
            .unwrap();
        fs::File::create(&files.spectrum)
            .unwrap()
            .set_modified(now + Duration::from_secs(20))
            .unwrap();
        fs::write(&files.summary, b"1").unwrap();
        let requests = files.poll();
        assert!(!requests.stop && !requests.metering && !requests.spectrum && requests.summary);
        fs::write(&files.mixer, b"1").unwrap();
        fs::remove_file(&files.summary).unwrap();
        let requests = files.poll();
        assert!(requests.metering && !requests.summary);
        fs::write(&files.stop, b"1").unwrap();
        assert!(files.poll().stop);
    }

    #[test]
    fn a_pending_audition_is_read_after_the_directory_poll() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = ControlFiles::new(dir.path());
        let now = SystemTime::now();
        let mut audition = Audition {
            started: now,
            refreshed: now,
            baseline: Settings::default(),
            target: AuditionTarget::Suppression { limit_db: 9 },
        };
        write_json(&files.audition, &audition).unwrap();
        let requests = files.poll();
        audition.target = AuditionTarget::Suppression { limit_db: 17 };
        write_json(&files.audition, &audition).unwrap();
        assert!(matches!(
            files.audition(&requests).unwrap().target,
            AuditionTarget::Suppression { limit_db: 17 }
        ));
        fs::remove_file(&files.audition).unwrap();
        assert!(files.audition(&requests).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn a_large_or_unlistable_directory_falls_back_to_named_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = ControlFiles::new(&dir.path().join("new"));
        assert!(!files.poll().metering && files.retry_scan.is_some());
        fs::create_dir(&files.dir).unwrap();
        fs::write(&files.mixer, b"1").unwrap();
        assert!(files.poll().metering);
        files.retry_scan = None;
        for n in 0..65 {
            fs::write(files.dir.join(n.to_string()), []).unwrap();
        }
        assert!(files.poll().metering && files.retry_scan.is_some());
        fs::write(&files.stop, b"1").unwrap();
        assert!(files.poll().stop);
    }

    #[cfg(windows)]
    #[test]
    fn an_alternate_spelling_uses_the_filesystems_case_rules() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = ControlFiles::new(dir.path());
        fs::write(dir.path().join("OBSERVE"), b"1").unwrap();
        assert_eq!(files.poll().metering, recently_observed(&files.mixer));
        assert!(files.retry_scan.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn updates_through_a_hard_link_use_the_current_heartbeat_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = ControlFiles::new(dir.path());
        fs::write(&files.spectrum, b"1").unwrap();
        let alias = dir.path().join("alias");
        if let Err(error) = fs::hard_link(&files.spectrum, &alias) {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                return; // Some Windows sandboxes disallow creating hard links.
            }
            panic!("create hard link: {error}");
        }
        fs::File::options()
            .write(true)
            .open(alias)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(15))
            .unwrap();
        assert!(!files.poll().spectrum);
    }
}
