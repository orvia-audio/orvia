//! Session-scoped controls for recording and replaying the virtual microphone.
use super::*;
use orvia_engine::mic_test::{self, FileRecording, Playback, SavedClip};

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
    pub(super) recording: Option<FileRecording>,
    pub(super) reader: reading::Reader,
    pub(super) clip: Option<SavedClip>,
    pub(super) playback: Option<Playback>,
    pub(super) adjust: Option<super::auto_adjust::AutoAdjust>,
    #[cfg(test)]
    pub(super) snapshot_action: Option<TestAction>,
    started: Option<Instant>,
    recording_route: Option<(String, String)>,
}

impl MicTest {
    pub(super) fn busy(&self) -> bool {
        matches!(
            self.primary_action(),
            TestAction::FinishRecording | TestAction::StopPlayback
        )
    }

    pub(super) fn primary_action(&self) -> TestAction {
        #[cfg(test)]
        if let Some(action) = self.snapshot_action {
            return action;
        }
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
        self.test.source = mic_test::routed_source(&self.prefs).ok();
        self.test.speaker = self.prefs.monitor.clone();
        match SavedClip::latest(&self.dir.join("mic-test")) {
            Ok(clip) => self.test.clip = clip,
            Err(error) => self.test_error(error),
        }
    }

    pub(super) fn test_action_enabled(&self, action: TestAction) -> bool {
        if self.test.adjust.is_some()
            && !matches!(action, TestAction::Play | TestAction::StopPlayback)
        {
            return false;
        }
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
        match recording.finish() {
            Ok(clip) => self.test.clip = Some(clip),
            Err(e) => self.test_error(e),
        }
    }

    fn start_test_recording(&mut self) {
        if let Some(id) = &self.test.source {
            match FileRecording::start(id, &self.dir.join("mic-test")) {
                Ok(recording) => {
                    self.error.clear();
                    self.test.recording = Some(recording);
                    self.test.reader = Default::default();
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

    pub(super) fn poll_mic_test(&mut self, cx: &mut Context<Self>) {
        if self.test.adjust.is_some() {
            self.poll_adjustment(cx);
        }
        let timeout = self.test.started.is_some_and(|t| {
            self.test
                .clip
                .as_ref()
                .is_some_and(|c| t.elapsed() > c.duration() + Duration::from_secs(4))
        });
        if let Some(recording) = &mut self.test.recording {
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
            } else {
                match self
                    .test
                    .reader
                    .listen_saved(recording, self.prefs.language)
                {
                    Ok(true) => self.finish_test_recording(),
                    Ok(false) => {}
                    Err(e) => {
                        self.test.recording = None;
                        self.test_error(e);
                    }
                }
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
