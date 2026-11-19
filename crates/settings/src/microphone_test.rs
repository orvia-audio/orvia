//! Session-scoped controls for recording and replaying the virtual microphone.
use super::*;
use orvia_engine::mic_test::{self, Clip, Playback, Recording};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Take {
    #[default]
    Before,
    After,
}

impl Take {
    fn path(self, dir: &Path) -> PathBuf {
        dir.join("mic-test").join(match self {
            Self::Before => "before.wav",
            Self::After => "after.wav",
        })
    }
}

#[derive(Default)]
pub(super) struct MicTest {
    pub(super) take: Take,
    pub(super) sources: Vec<(String, String)>,
    pub(super) source: Option<String>,
    pub(super) speaker: Option<String>,
    pub(super) recording: Option<Recording>,
    pub(super) clip: Option<Clip>,
    pub(super) playback: Option<Playback>,
    started: Option<Instant>,
    recording_route: Option<(String, String)>,
}

impl MicTest {
    pub(super) fn busy(&self) -> bool {
        self.recording.is_some() || self.playback.is_some()
    }
}

impl Panel {
    pub(super) fn select_take(&mut self, take: Take) {
        if self.test.busy() {
            return;
        }
        self.test.take = take;
        self.test.clip = None;
        let path = take.path(&self.dir);
        match path.try_exists() {
            Ok(false) => {}
            Ok(true) => match Clip::load(&path) {
                Ok(clip) => self.test.clip = Some(clip),
                Err(error) => self.test_error(error),
            },
            Err(error) => self.test_error(error),
        }
    }

    pub(super) fn open_mic_test(&mut self) {
        match mic_test::sources() {
            Ok(sources) => self.test.sources = sources,
            Err(e) => self.error = e.to_string(),
        }
        if let Ok(devices) = orvia_engine::monitors() {
            self.monitors = devices;
        }
        let candidates = &self.test.sources;
        let output = self
            .outputs
            .iter()
            .find(|(id, _)| Some(id) == self.prefs.output.as_ref());
        let mut matching = candidates.iter().filter(|(id, name)| {
            output.is_some_and(|(out, label)| {
                id == out
                    || name == label
                    || (out == "pulseaudio:orvia_output" && id == "pulseaudio:orvia_microphone")
            })
        });
        let preferred = matching.next().filter(|_| matching.next().is_none());
        self.test.source = preferred
            .or(match candidates.as_slice() {
                [only] => Some(only),
                _ => None,
            })
            .map(|(id, _)| id.clone());
        self.test.speaker = self.prefs.monitor.clone();
        self.select_take(if Take::Before.path(&self.dir).is_file() {
            Take::After
        } else {
            Take::Before
        });
    }

    pub(super) fn can_record_test(&self) -> bool {
        self.test.recording.is_some()
            || (self.test.playback.is_none()
                && self.test.source.is_some()
                && self
                    .status
                    .as_ref()
                    .is_some_and(|s| s.message.starts_with("Streaming")))
    }

    pub(super) fn record_test(&mut self) {
        if let Some(recording) = self.test.recording.take() {
            self.test.recording_route = None;
            let path = self.test.take.path(&self.dir);
            let result = recording.finish().and_then(|clip| {
                fs::create_dir_all(self.dir.join("mic-test"))?;
                clip.save(&path)?;
                Ok(clip)
            });
            match result {
                Ok(clip) => self.test.clip = Some(clip),
                Err(e) => self.test_error(e),
            }
        } else if self.can_record_test()
            && let Some(id) = &self.test.source
        {
            match Recording::start(id) {
                Ok(recording) => {
                    self.error.clear();
                    self.test.recording = Some(recording);
                    self.test.started = Some(Instant::now());
                    self.test.recording_route = self
                        .status
                        .as_ref()
                        .map(|s| (s.version.clone(), s.message.clone()));
                }
                Err(e) => self.test_error(e),
            }
        }
    }

    pub(super) fn play_test(&mut self) {
        if self.test.playback.take().is_some() {
            return;
        }
        if self.test.recording.is_none()
            && let Some(clip) = &self.test.clip
        {
            match clip.play(self.test.speaker.as_deref()) {
                Ok(playback) => {
                    self.error.clear();
                    self.test.playback = Some(playback);
                    self.test.started = Some(Instant::now());
                }
                Err(e) => self.test_error(e),
            }
        }
    }

    fn test_error(&mut self, err: impl std::fmt::Display) {
        self.error = format!(
            "{} · {err}",
            self.prefs
                .language
                .text("Mic test failed", "マイクテストに失敗しました")
        );
    }

    pub(super) fn poll_mic_test(&mut self) {
        let timeout = self
            .test
            .started
            .is_some_and(|t| t.elapsed() > mic_test::LIMIT + Duration::from_secs(3));
        if let Some(recording) = &self.test.recording {
            if !self
                .status
                .as_ref()
                .zip(self.test.recording_route.as_ref())
                .is_some_and(|(s, (version, route))| s.version == *version && s.message == *route)
            {
                self.test.recording = None;
                self.error = self
                    .prefs
                    .language
                    .text(
                        "Audio route changed. Retake the recording.",
                        "音声経路が変わりました。録り直してください。",
                    )
                    .into();
            } else if let Some(error) = recording.error() {
                self.test.recording = None;
                self.test_error(error);
            } else if recording.duration() >= mic_test::LIMIT {
                self.record_test();
            } else if timeout {
                self.test.recording = None;
                self.error = self
                    .prefs
                    .language
                    .text(
                        "Recording stalled. Check the virtual microphone.",
                        "録音が途切れました。仮想マイクを確認してください。",
                    )
                    .into();
            }
        } else if let Some(playback) = &self.test.playback {
            match playback.finished() {
                Ok(true) => self.test.playback = None,
                Ok(false) if timeout => {
                    self.test.playback = None;
                    self.error = self
                        .prefs
                        .language
                        .text(
                            "Playback stalled. Check the output device.",
                            "再生が途切れました。再生先を確認してください。",
                        )
                        .into();
                }
                Ok(false) => {}
                Err(e) => {
                    self.test.playback = None;
                    self.test_error(e);
                }
            }
        }
    }
}
