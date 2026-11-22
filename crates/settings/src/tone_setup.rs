//! Capture a neutral virtual microphone take and audition three output controls together.
use super::*;
use auto_adjust::Trial;
use orvia_core::{AuditionTarget, Tone, calibration::SampleError};
use orvia_engine::mic_test::{self, Clip, Playback, Recording};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Clone, Copy)]
pub(super) enum Action {
    Open,
    Start,
    Read,
    Play(usize),
    Apply,
}

type Comparison = (Tone, [Clip; 2]);

enum Stage {
    Intro,
    Opening,
    Room(Recording, Instant),
    Reading(Clip, Recording, reading::Reader),
    Analyzing(Receiver<Result<Comparison, SampleError>>),
    Ready(Comparison),
    Failed(String),
}

/// The live trial ends when capture completes or the user leaves the wizard.
pub(super) struct Setup {
    baseline: Settings,
    receiver: String,
    trial: Option<Trial>,
    stage: Stage,
    playback: Option<(usize, Playback)>,
    playback_error: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum Screen<'a> {
    Intro,
    Opening,
    Room,
    Reading(view::ReadingCue),
    Analyzing,
    Ready(Tone, Option<usize>),
    Failed(&'a str),
}

impl Setup {
    pub(super) fn screen(&self) -> Screen<'_> {
        match &self.stage {
            Stage::Intro => Screen::Intro,
            Stage::Opening => Screen::Opening,
            Stage::Room(r, _) if r.duration() < Duration::from_millis(500) => Screen::Opening,
            Stage::Room(..) => Screen::Room,
            Stage::Reading(_, _, reader) => Screen::Reading(reader.into()),
            Stage::Analyzing(_) => Screen::Analyzing,
            Stage::Ready((tone, _)) => Screen::Ready(*tone, self.playback.as_ref().map(|p| p.0)),
            Stage::Failed(e) => Screen::Failed(e),
        }
    }

    fn fail(&mut self, error: String) {
        self.trial = None;
        self.playback = None;
        self.stage = Stage::Failed(error);
    }

    fn start(&mut self, dir: &Path, status: Option<&EngineStatus>) -> Result<(), String> {
        let l = self.baseline.language;
        if self.baseline.gain_percent == 0 {
            return Err(l
                .text(
                    "Raise the output gain before recording.",
                    "録音するには出力ゲインを上げてください。",
                )
                .into());
        }
        if !status.is_some_and(|s| {
            s.message.starts_with("Streaming") && s.version == env!("CARGO_PKG_VERSION")
        }) {
            return Err(l
                .text(
                    "Connect the virtual output and start the matching audio engine.",
                    "仮想出力を接続し、同じバージョンの音声エンジンを起動してください。",
                )
                .into());
        }
        self.receiver = mic_test::routed_source(&self.baseline).map_err(|e| e.to_string())?;
        self.trial = Some(
            Trial::start(
                dir,
                &self.baseline,
                AuditionTarget::ToneSetup {
                    tone: Tone::default(),
                },
            )
            .map_err(|e| e.to_string())?,
        );
        self.stage = Stage::Opening;
        Ok(())
    }

    fn start_reading(&mut self) -> Result<(), String> {
        if !matches!(&self.stage, Stage::Room(r, _) if r.duration() >= Duration::from_millis(500)) {
            return Ok(());
        }
        if let Stage::Room(recording, _) = std::mem::replace(&mut self.stage, Stage::Opening) {
            let room = recording.finish().map_err(|e| e.to_string())?;
            let reading = Recording::start(&self.receiver).map_err(|e| e.to_string())?;
            self.stage = Stage::Reading(room, reading, Default::default());
        }
        Ok(())
    }

    fn play(&mut self, index: usize, speaker: Option<&str>) {
        if let Stage::Ready((_, clips)) = &self.stage {
            self.playback = None;
            match clips[index].play(speaker) {
                Ok(player) => self.playback = Some((index, player)),
                Err(e) => self.playback_error = Some(e.to_string()),
            }
        }
    }

    fn poll(
        &mut self,
        prefs: &Settings,
        status: Option<&EngineStatus>,
        cx: &Context<Panel>,
    ) -> Result<(), String> {
        if matches!(self.stage, Stage::Intro | Stage::Failed(_)) {
            return Ok(());
        }
        let l = prefs.language;
        if !prefs.same_mix(&self.baseline) {
            return Err(l
                .text(
                    "The mix changed. Record again.",
                    "ミックス設定が変わりました。録り直してください。",
                )
                .into());
        }
        if let Some(trial) = &mut self.trial {
            trial.keep_alive(&self.baseline)?;
            if status.is_none_or(|s| s.audition != Some(trial.issued)) {
                if matches!(self.stage, Stage::Opening)
                    && trial.since.elapsed() < Duration::from_secs(5)
                {
                    return Ok(());
                }
                return Err(l
                    .text(
                        "The engine could not confirm the recording route.",
                        "音声エンジンが録音経路を確認できませんでした。",
                    )
                    .into());
            }
        }
        fn audio_error(e: impl std::fmt::Display) -> String {
            e.to_string()
        }
        match &mut self.stage {
            Stage::Opening => {
                self.stage = Stage::Room(
                    Recording::start(&self.receiver).map_err(audio_error)?,
                    Instant::now(),
                );
            }
            Stage::Room(r, since) => {
                r.collect().map_err(audio_error)?;
                if since.elapsed() > Duration::from_secs(90)
                    || since.elapsed().saturating_sub(r.duration()) > Duration::from_secs(4)
                {
                    return Err(l
                        .text(
                            "Room recording timed out. Try again.",
                            "環境音を録り直してください。",
                        )
                        .into());
                }
            }
            Stage::Reading(_, recording, reader) => {
                if recording.duration() > Duration::from_secs(120) {
                    return Err(l.text("No end of speech detected. Record again and pause after reading.", "読み終わりを確認できませんでした。録り直し、読み終えたら少し間を空けてください。").into());
                }
                if reader.listen(recording, l)? {
                    let Stage::Reading(room, recording, _) =
                        std::mem::replace(&mut self.stage, Stage::Opening)
                    else {
                        unreachable!()
                    };
                    let voice = recording.finish().map_err(audio_error)?;
                    self.trial = None;
                    let current = prefs.tone();
                    let (tx, rx) = mpsc::channel();
                    cx.background_executor()
                        .spawn(async move {
                            let _ = tx.send(room.tone_previews(&voice, current));
                        })
                        .detach();
                    self.stage = Stage::Analyzing(rx);
                }
            }
            Stage::Analyzing(rx) => match rx.try_recv() {
                Ok(Ok(comparison)) => {
                    self.stage = Stage::Ready(comparison);
                    self.play(1, prefs.monitor.as_deref());
                }
                Ok(Err(e)) => {
                    return Err(match e {
                        SampleError::Clipped => l.text(
                            "The recording clipped. Lower the microphone input and try again.",
                            "音割れしています。マイク入力を下げて録り直してください。",
                        ),
                        SampleError::UnclearSpeech => l.text(
                            "Speech was too close to the room level. Record again.",
                            "声と環境音を区別できませんでした。録り直してください。",
                        ),
                        SampleError::Incomplete => l.text(
                            "The recording was interrupted. Try again.",
                            "録音が途切れました。録り直してください。",
                        ),
                    }
                    .into());
                }
                Err(TryRecvError::Disconnected) => {
                    return Err(l
                        .text("Analysis interrupted.", "解析が中断されました。")
                        .into());
                }
                Err(TryRecvError::Empty) => {}
            },
            Stage::Ready(_) => {
                if let Some((_, player)) = &self.playback {
                    match player.finished() {
                        Ok(false) => {}
                        Ok(true) => self.playback = None,
                        Err(e) => {
                            self.playback = None;
                            self.playback_error = Some(e.to_string());
                        }
                    }
                }
            }
            Stage::Intro | Stage::Failed(_) => {}
        }
        Ok(())
    }
}

impl Panel {
    pub(super) fn tone_action(&mut self, action: Action, cx: &mut Context<Self>) {
        if matches!(action, Action::Open) {
            self.error.clear();
            self.choosing = None;
            self.tone_setup = Some(Setup {
                baseline: self.prefs.clone(),
                receiver: String::new(),
                stage: Stage::Intro,
                trial: None,
                playback: None,
                playback_error: None,
            });
            return;
        }
        let Some(setup) = &mut self.tone_setup else {
            return;
        };
        let result = match action {
            Action::Start if matches!(setup.stage, Stage::Intro | Stage::Failed(_)) => {
                setup.baseline = self.prefs.clone();
                setup.start(&self.dir, self.status.as_ref())
            }
            Action::Read => setup.start_reading(),
            Action::Play(index) => {
                setup.play(index, self.prefs.monitor.as_deref());
                Ok(())
            }
            Action::Apply => {
                if let Stage::Ready((tone, _)) = setup.stage {
                    let mut applied = false;
                    match Settings::update(&self.dir.join("settings.json"), |s| {
                        if s.same_mix(&setup.baseline) {
                            s.set_tone(tone);
                            applied = true;
                        }
                    }) {
                        Ok(prefs) => {
                            self.prefs = prefs;
                            if applied {
                                self.activate(Control::Page(Page::Mixer), cx);
                            } else {
                                self.poll_tone_setup(cx);
                            }
                        }
                        Err(e) => self.error = e.to_string(),
                    }
                }
                return;
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            setup.fail(e);
        }
    }

    pub(super) fn poll_tone_setup(&mut self, cx: &Context<Self>) {
        if let Some(setup) = &mut self.tone_setup {
            if let Err(e) = setup.poll(&self.prefs, self.status.as_ref(), cx) {
                setup.fail(e);
            }
            if let Some(e) = setup.playback_error.take() {
                self.error = e;
            }
        }
    }
}
