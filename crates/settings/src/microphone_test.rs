//! Session-scoped controls for recording and replaying the virtual microphone.
use super::*;
use orvia_engine::mic_test::{self, Clip, Playback, Recording};

/// The operation shown on a microphone test button when it was rendered.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TestAction {
    Record,
    FinishRecording,
    Play,
    StopPlayback,
    Retake,
}

impl TestAction {
    pub(super) fn label(self, l: Language) -> &'static str {
        match self {
            Self::Record => l.text("Record", "録音"),
            Self::FinishRecording => l.text("Stop recording", "録音を停止"),
            Self::Play => l.text("Play recording", "録音を再生"),
            Self::StopPlayback => l.text("Stop playback", "再生を停止"),
            Self::Retake => l.text("Retake", "録り直す"),
        }
    }
}

#[derive(Default)]
pub(super) struct MicTest {
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

    pub(super) fn primary_action(&self) -> TestAction {
        if self.recording.is_some() {
            TestAction::FinishRecording
        } else if self.playback.is_some() {
            TestAction::StopPlayback
        } else if self.clip.is_some() {
            TestAction::Play
        } else {
            TestAction::Record
        }
    }
}

impl Panel {
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
        match Clip::load_latest(&self.dir.join("mic-test")) {
            Ok(clip) => self.test.clip = clip,
            Err(error) => self.test_error(error),
        }
    }

    pub(super) fn test_action_enabled(&self, action: TestAction) -> bool {
        let primary = self.test.primary_action();
        match action {
            TestAction::Record | TestAction::Retake => {
                let expected = if action == TestAction::Retake {
                    TestAction::Play
                } else {
                    TestAction::Record
                };
                primary == expected
                    && self.test.source.is_some()
                    && self
                        .status
                        .as_ref()
                        .is_some_and(|s| s.message.starts_with("Streaming"))
            }
            _ => action == primary,
        }
    }

    pub(super) fn run_test_action(&mut self, action: TestAction) {
        // An automatic stop can land between a button press and its click.
        if !self.test_action_enabled(action) {
            return;
        }
        match action {
            TestAction::Record | TestAction::Retake => self.start_test_recording(),
            TestAction::FinishRecording => self.finish_test_recording(),
            TestAction::Play => self.play_test(),
            TestAction::StopPlayback => self.test.playback = None,
        }
    }

    pub(super) fn finish_test_recording(&mut self) {
        let Some(recording) = self.test.recording.take() else {
            return;
        };
        self.test.recording_route = None;
        let result = recording.finish().and_then(|clip| {
            clip.save_latest(&self.dir.join("mic-test"))?;
            Ok(clip)
        });
        match result {
            Ok(clip) => self.test.clip = Some(clip),
            Err(e) => self.test_error(e),
        }
    }

    fn start_test_recording(&mut self) {
        if let Some(id) = &self.test.source {
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

    fn play_test(&mut self) {
        if let Some(clip) = &self.test.clip {
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
                self.finish_test_recording();
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
