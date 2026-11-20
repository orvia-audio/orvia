//! Guided suppression measurements and auditions on the virtual microphone receiver.
use super::*;
use orvia_core::{
    AUDITION_FILE, Audition,
    calibration::{Recommendation, SampleError},
    write_json,
};
use orvia_engine::mic_test::{self, Clip, Recording};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::SystemTime,
};

pub(super) enum Stage {
    Room(Recording, Instant),
    Phrase {
        room: Clip,
        first: Option<Clip>,
        recording: Option<Recording>,
        reader: reading::Reader,
    },
    Analyzing(Receiver<Result<Recommendation, SampleError>>),
    Auditioning {
        result: Recommendation,
        trial: Trial,
        recording: Option<Box<(Recording, Recording)>>,
        reader: reading::Reader,
        ending: Option<Instant>,
    },
    Restoring {
        result: Recommendation,
        clip: Clip,
        issued: SystemTime,
        since: Instant,
    },
    Ready(Recommendation),
    Failed(String),
}

/// Removing the request restores the live limit. Engine expiry also covers a mixer crash.
pub(super) struct Trial {
    path: PathBuf,
    issued: SystemTime,
    since: Instant,
    last_refresh: Instant,
}

impl Trial {
    fn start(dir: &Path, settings: &Settings, limit_db: u16) -> io::Result<Self> {
        let path = dir.join(AUDITION_FILE);
        let issued = SystemTime::now();
        write_json(
            &path,
            &Audition {
                baseline: settings.clone(),
                started: issued,
                refreshed: issued,
                limit_db,
            },
        )?;
        Ok(Self {
            path,
            issued,
            since: Instant::now(),
            last_refresh: Instant::now(),
        })
    }
    fn keep_alive(&mut self, settings: &Settings, limit_db: u16) -> Result<(), String> {
        if self.last_refresh.elapsed() >= Duration::from_secs(3) {
            write_json(
                &self.path,
                &Audition {
                    baseline: settings.clone(),
                    started: self.issued,
                    refreshed: SystemTime::now(),
                    limit_db,
                },
            )
            .map_err(|e| e.to_string())?;
            self.last_refresh = Instant::now();
        }
        Ok(())
    }
}

impl Drop for Trial {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) struct AutoAdjust {
    settings: Settings,
    source: String,
    pub(super) stage: Stage,
}

fn changed(l: Language) -> String {
    l.text(
        "The audio route or mix changed. Record again.",
        "音声経路またはミックス設定が変わりました。録り直してください。",
    )
    .into()
}

fn sample_error(e: SampleError, l: Language) -> String {
    match e {
        SampleError::Incomplete => l.text(
            "Recording was interrupted. Check the microphone and try again.",
            "録音が途切れました。マイクを確認して録り直してください。",
        ),
        SampleError::UnclearSpeech => l.text(
            "Two clear phrases are needed. Speak during both prompts and pause between them.",
            "声を2回確認できませんでした。画面の合図に合わせて話し、間を空けてください。",
        ),
        SampleError::Clipped => l.text(
            "The microphone clipped. Lower its input level and try again.",
            "マイク入力が音割れしています。入力レベルを下げて録り直してください。",
        ),
    }
    .into()
}

impl Stage {
    fn advance(
        self,
        settings: &Settings,
        source: &str,
        panel: &mut Panel,
        cx: &mut Context<Panel>,
    ) -> Result<Self, String> {
        let l = settings.language;
        match self {
            Self::Failed(_) => Ok(self),
            _ if !settings.same_audition(&panel.prefs) => {
                panel.test.playback = None;
                panel.test.clip = None;
                Err(changed(l))
            }
            Self::Room(mut recording, since) => {
                recording.collect().map_err(|e| e.to_string())?;
                if recording.duration() >= Duration::from_millis(1500) {
                    let room = recording.finish().map_err(|e| e.to_string())?;
                    Ok(Self::Phrase {
                        room,
                        first: None,
                        recording: None,
                        reader: Default::default(),
                    })
                } else if since.elapsed() > Duration::from_secs(5) {
                    Err(sample_error(SampleError::Incomplete, l))
                } else {
                    Ok(Self::Room(recording, since))
                }
            }
            Self::Phrase {
                room,
                first,
                recording,
                mut reader,
            } => {
                let Some(mut recording) = recording else {
                    let mic = Recording::start_microphone(settings.input.as_deref())
                        .map_err(|e| e.to_string())?;
                    return Ok(Self::Phrase {
                        room,
                        first,
                        recording: Some(mic),
                        reader,
                    });
                };
                if !reader.listen(&mut recording, l)? {
                    return Ok(Self::Phrase {
                        room,
                        first,
                        recording: Some(recording),
                        reader,
                    });
                }
                let phrase = recording.finish().map_err(|e| e.to_string())?;
                let Some(first) = first else {
                    return Ok(Self::Phrase {
                        room,
                        first: Some(phrase),
                        recording: None,
                        reader: Default::default(),
                    });
                };
                let (tx, rx) = mpsc::channel();
                let mixing = settings.mixing;
                cx.background_executor()
                    .spawn(async move {
                        let _ = tx.send(room.recommend([&first, &phrase], mixing));
                    })
                    .detach();
                Ok(Self::Analyzing(rx))
            }
            Self::Analyzing(ref rx) => match rx.try_recv() {
                Ok(Ok(result)) => {
                    let trial = Trial::start(&panel.dir, settings, result.limit_db)
                        .map_err(|e| e.to_string())?;
                    Ok(Self::Auditioning {
                        result,
                        trial,
                        recording: None,
                        reader: Default::default(),
                        ending: None,
                    })
                }
                Ok(Err(e)) => Err(sample_error(e, l)),
                Err(TryRecvError::Empty) => Ok(self),
                Err(TryRecvError::Disconnected) => Err(l
                    .text(
                        "Analysis interrupted. Try again.",
                        "解析が中断されました。もう一度お試しください。",
                    )
                    .into()),
            },
            Self::Auditioning {
                result,
                mut trial,
                recording,
                mut reader,
                mut ending,
            } => {
                trial.keep_alive(settings, result.limit_db)?;
                let acknowledged = panel
                    .status
                    .as_ref()
                    .is_some_and(|s| s.audition == Some(trial.issued));
                let recording = match recording {
                    Some(_) if !acknowledged => return Err(changed(l)),
                    Some(streams) => {
                        let (mut microphone, mut receiver) = *streams;
                        receiver.collect().map_err(|e| e.to_string())?;
                        if let Some(stop) = ending {
                            if stop.elapsed() >= Duration::from_millis(350) {
                                let clip = receiver.finish().map_err(|e| e.to_string())?;
                                let issued = trial.issued;
                                drop(trial);
                                return Ok(Self::Restoring {
                                    result,
                                    clip,
                                    issued,
                                    since: Instant::now(),
                                });
                            }
                        } else if reader.listen(&mut microphone, l)? {
                            ending = Some(Instant::now());
                        }
                        Some(Box::new((microphone, receiver)))
                    }
                    None if acknowledged => {
                        let receiver = Recording::start(source).map_err(|e| e.to_string())?;
                        let microphone = Recording::start_microphone(settings.input.as_deref())
                            .map_err(|e| e.to_string())?;
                        Some(Box::new((microphone, receiver)))
                    }
                    None if trial.since.elapsed() > Duration::from_secs(4) => return Err(l
                        .text(
                            "The engine could not start the audition. Check the virtual output.",
                            "試聴を開始できません。仮想出力を確認してください。",
                        )
                        .into()),
                    None => None,
                };
                Ok(Self::Auditioning {
                    result,
                    trial,
                    recording,
                    reader,
                    ending,
                })
            }
            Self::Restoring {
                result,
                clip,
                issued,
                since,
            } => {
                if panel
                    .status
                    .as_ref()
                    .is_some_and(|s| s.audition != Some(issued))
                {
                    panel.test.clip = Some(clip);
                    panel.play_test();
                    Ok(Self::Ready(result))
                } else if since.elapsed() > Duration::from_secs(4) {
                    Err(l
                        .text(
                            "The engine did not confirm the end of the audition.",
                            "試聴の終了をエンジンに確認できませんでした。",
                        )
                        .into())
                } else {
                    Ok(Self::Restoring {
                        result,
                        clip,
                        issued,
                        since,
                    })
                }
            }
            Self::Ready(_) => Ok(self),
        }
    }
}

impl Panel {
    pub(super) fn start_adjustment(&mut self) {
        if self.prefs.active_noise_method() != Some(NoiseMethod::DeepFilterNet3) {
            return;
        }
        let speaker = if self.test.adjust.is_some() {
            self.test.speaker.clone()
        } else {
            self.prefs.monitor.clone()
        };
        self.page = Page::MicTest;
        self.choosing = None;
        self.error.clear();
        self.test = Default::default();
        self.test.speaker = speaker;
        let source = mic_test::routed_source(&self.prefs);
        let mut id = String::new();
        let l = self.prefs.language;
        let stage = match source {
            Err(e) => Stage::Failed(format!(
                "{} · {e}",
                l.text(
                    "Check the recording endpoint in Devices",
                    "デバイス画面で仮想マイクの録音側を確認してください"
                )
            )),
            Ok(_) if self.prefs.gain_percent == 0 => Stage::Failed(
                l.text(
                    "Raise the output gain to hear a preview.",
                    "プレビューを聴くには出力ゲインを上げてください。",
                )
                .into(),
            ),
            Ok(_)
                if !self.status.as_ref().is_some_and(|s| {
                    s.message.starts_with("Streaming") && s.version == env!("CARGO_PKG_VERSION")
                }) =>
            {
                Stage::Failed(
                    l.text(
                        "Connect the virtual output and start the matching audio engine.",
                        "仮想出力を接続し、同じバージョンの音声エンジンを起動してください。",
                    )
                    .into(),
                )
            }
            Ok(source) => {
                id = source;
                match Recording::start_microphone(self.prefs.input.as_deref()) {
                    Ok(r) => Stage::Room(r, Instant::now()),
                    Err(e) => Stage::Failed(e.to_string()),
                }
            }
        };
        self.test.adjust = Some(AutoAdjust {
            settings: self.prefs.clone(),
            source: id,
            stage,
        });
    }

    pub(super) fn poll_adjustment(&mut self, cx: &mut Context<Self>) {
        let Some(mut adjust) = self.test.adjust.take() else {
            return;
        };
        adjust.stage = adjust
            .stage
            .advance(&adjust.settings, &adjust.source, self, cx)
            .unwrap_or_else(Stage::Failed);
        self.test.adjust = Some(adjust);
    }

    pub(super) fn apply_adjustment(&mut self, cx: &mut Context<Self>) {
        let Some(adjust) = &self.test.adjust else {
            return;
        };
        let Stage::Ready(result) = adjust.stage else {
            return;
        };
        let mut applied = false;
        match Settings::update(&self.dir.join("settings.json"), |settings| {
            if adjust.settings.same_audition(settings) {
                settings.deep_filter_limit_db = result.limit_db;
                applied = true;
            }
        }) {
            Ok(saved) => {
                self.prefs = saved;
                if applied {
                    self.error.clear();
                    self.activate(Control::Page(Page::Mixer), cx);
                } else {
                    self.poll_adjustment(cx);
                }
            }
            Err(e) => self.error = e.to_string(),
        }
    }
}
