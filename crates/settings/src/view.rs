//! Layout and localized controls for the mixer, device routing and preferences.
use super::*;
use crate::icons::glyph;

#[path = "tone_view.rs"]
mod tone_view;
use tone_view::tone_labels;

#[path = "mixer.rs"]
mod mixer;
pub(super) use mixer::MixerViews;
#[path = "picker.rs"]
mod picker;
#[path = "spectrum.rs"]
mod spectrum;

const INK: u32 = 0x242a2b;
const MUTED: u32 = 0x575f61;
const PAPER: u32 = 0xf7f5f1;
const LINE: u32 = 0xdedbd4;
const TERRACOTTA: u32 = 0x954b37;
const CORAL: u32 = 0xffb18c;

fn row() -> gpui::Div {
    div().flex().items_center().gap_2().flex_wrap()
}
fn stack() -> gpui::Div {
    div().flex().flex_col().gap_3().min_w_0()
}
fn card() -> gpui::Div {
    stack()
        .p_4()
        .rounded_xl()
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(rgb(LINE))
        .flex_shrink_0()
}
fn heading(label: &'static str, icon: &'static str, color: u32) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_base()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(glyph(icon, 20., color))
        .child(label)
}
fn note(s: impl Into<SharedString>) -> gpui::Div {
    div().text_sm().text_color(rgb(MUTED)).child(s.into())
}

/// The visible instruction for the current microphone reading.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadingCue {
    Measuring,
    Read,
}

/// Values that can change visibly during an audio poll. The engine and reader
/// continue polling while a displayed instruction or playback label is steady.
#[derive(PartialEq, Eq)]
pub(super) enum AudioDisplay {
    Idle,
    Opening,
    Room,
    Read(u8, ReadingCue),
    Analyzing,
    Preview(Option<ReadingCue>),
    Restoring,
    Ready(Option<usize>),
    Failed,
    Test(TestAction, ReadingCue, Option<String>),
}

impl Panel {
    pub(super) fn audio_display(&self) -> AudioDisplay {
        use tone_setup::Screen;
        if let Some(setup) = &self.tone_setup {
            return match setup.screen() {
                Screen::Intro => AudioDisplay::Idle,
                Screen::Opening => AudioDisplay::Opening,
                Screen::Room => AudioDisplay::Room,
                Screen::Reading(cue) => AudioDisplay::Read(1, cue),
                Screen::Analyzing => AudioDisplay::Analyzing,
                Screen::Ready(_, playing) => AudioDisplay::Ready(playing),
                Screen::Failed(_) => AudioDisplay::Failed,
            };
        }
        if self.page != Page::MicTest {
            return AudioDisplay::Idle;
        }
        if let Some(adjust) = &self.test.adjust {
            return match (&adjust.stage).into() {
                AdjustmentScreen::Room => AudioDisplay::Room,
                AdjustmentScreen::Phrase(take, cue) => AudioDisplay::Read(take, cue),
                AdjustmentScreen::Analyzing => AudioDisplay::Analyzing,
                AdjustmentScreen::Preview(cue) => AudioDisplay::Preview(cue),
                AdjustmentScreen::Restoring => AudioDisplay::Restoring,
                AdjustmentScreen::Ready(_) => {
                    AudioDisplay::Ready(self.test.playback.as_ref().map(|_| 0))
                }
                AdjustmentScreen::Failed(_) => AudioDisplay::Failed,
            };
        }
        AudioDisplay::Test(
            self.test.primary_action(),
            (&self.test.reader).into(),
            self.test
                .recording
                .as_ref()
                .map(|r| recording_length(r.duration())),
        )
    }
}

fn recording_length(duration: Duration) -> String {
    format!("{:.1} s", duration.as_secs_f32())
}

impl From<&reading::Reader> for ReadingCue {
    fn from(reader: &reading::Reader) -> Self {
        if !reader.ready() {
            Self::Measuring
        } else {
            Self::Read
        }
    }
}

/// Visible stages of a guided adjustment, independent of the active audio streams.
#[derive(Clone, Copy)]
pub(super) enum AdjustmentScreen<'a> {
    Room,
    Phrase(u8, ReadingCue),
    Analyzing,
    Preview(Option<ReadingCue>),
    Restoring,
    Ready(orvia_core::calibration::Recommendation),
    Failed(&'a str),
}

impl<'a> From<&'a auto_adjust::Stage> for AdjustmentScreen<'a> {
    fn from(stage: &'a auto_adjust::Stage) -> Self {
        use auto_adjust::Stage;

        match stage {
            Stage::Room(..) => Self::Room,
            Stage::Phrase { first, reader, .. } => {
                Self::Phrase(if first.is_some() { 2 } else { 1 }, reader.into())
            }
            Stage::Analyzing(_) => Self::Analyzing,
            Stage::Auditioning {
                recording, reader, ..
            } => Self::Preview(recording.as_ref().map(|_| reader.into())),
            Stage::Restoring { .. } => Self::Restoring,
            Stage::Ready(result) => Self::Ready(*result),
            Stage::Failed(error) => Self::Failed(error),
        }
    }
}

/// Reveal the optional count only after the recording endpoint is ready for speech.
fn speech_prompt(l: Language, cue: ReadingCue) -> gpui::Div {
    let body = stack().gap_1();
    if matches!(cue, ReadingCue::Measuring) {
        return body.child(note(l.text("Measuring room level…", "周囲の音量を測定中…")));
    }
    body.child(heading(
        l.text("Read aloud", "読み上げてください"),
        "microphone.svg",
        TERRACOTTA,
    ))
    .child(
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(l.text("“1　2　3　4　5”", "「１　２　３　４　５」")),
    )
}

impl Panel {
    fn button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        selected: bool,
        control: Control,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let disabled = (matches!(control, Control::RestartEngine) && self.restarting)
            || (matches!(control, Control::CheckUpdate) && self.updates.checking)
            || match control {
                Control::Test(action) => !self.test_action_enabled(action),
                Control::Choose(Endpoint::TestSpeaker | Endpoint::TestSource) => self.test.busy(),
                _ => false,
            };
        let leading = match &control {
            Control::AutoAdjust | Control::Tone(tone_setup::Action::Open) => {
                Some("adjustments-horizontal.svg")
            }
            Control::Tone(tone_setup::Action::Apply) => Some("check.svg"),
            Control::Tone(tone_setup::Action::Play(_)) => Some("player-play.svg"),
            Control::ApplyAdjustment => Some("check.svg"),
            Control::Test(TestAction::Record) => Some("microphone.svg"),
            Control::Test(TestAction::Play) => Some("player-play.svg"),
            Control::Test(TestAction::FinishRecording | TestAction::StopPlayback) => {
                Some("player-stop.svg")
            }
            Control::Test(TestAction::Retake) => Some("refresh.svg"),
            Control::RestartEngine => Some("refresh.svg"),
            Control::OpenLog => Some("external-link.svg"),
            Control::CheckUpdate => Some("refresh.svg"),
            Control::Releases => Some("external-link.svg"),
            _ => None,
        };
        let trailing = match &control {
            Control::Choose(_) if selected => Some("chevron-up.svg"),
            Control::Choose(_) => Some("chevron-down.svg"),
            _ => None,
        };
        let trigger = match control {
            Control::Choose(side) => Some(side),
            _ => None,
        };
        let wide = trigger.is_some();
        let primary = matches!(control, Control::Test(action) if action != TestAction::Retake);
        let apply = matches!(
            control,
            Control::ApplyAdjustment | Control::Tone(tone_setup::Action::Apply)
        );
        div()
            .id(id.into())
            .flex()
            .items_center()
            .gap_2()
            .when(wide, |d| d.w_full())
            .when(apply, |d| d.min_w(px(148.)).min_h(px(44.)).justify_center())
            .tab_index(0)
            .when(primary, |d| {
                d.track_focus(&self.test_primary_focus)
                    .w(px(148.))
                    .justify_center()
            })
            .when_some(trigger, |d, side| {
                d.track_focus(&self.picker_anchors[side as usize].focus)
            })
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .text_sm()
            .text_color(rgb(INK))
            .cursor_pointer()
            .border_color(rgb(if selected { 0xda9070 } else { LINE }))
            .bg(rgb(if selected { 0xffddca } else { 0xfaf9f6 }))
            .hover(|s| s.bg(rgb(0xffe4d7)))
            .focus(|s| s.border_color(rgb(0x215d91)))
            .on_click(cx.listener(move |p, event, w, cx| {
                if disabled {
                    return;
                }
                if let Some(side) = trigger {
                    w.focus(&p.picker_anchors[side as usize].focus);
                    if p.choosing == Some(side) && matches!(event, gpui::ClickEvent::Keyboard(_)) {
                        p.confirm_picker(side, w, cx);
                        return;
                    }
                }
                p.activate(control.clone(), cx);
                if matches!(control, Control::Test(TestAction::Retake))
                    && p.test.recording.is_some()
                {
                    w.focus(&p.test_primary_focus);
                }
            }))
            .when(disabled, |d| d.opacity(0.55))
            .when_some(leading, |d, icon| d.child(glyph(icon, 16., INK)))
            .child(
                div()
                    .when(wide, |d| d.flex_1().min_w_0())
                    .child(label.into()),
            )
            .when_some(trailing, |d, icon| d.child(glyph(icon, 16., INK)))
    }

    fn switch(
        &self,
        id: &'static str,
        on: bool,
        control: Control,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let l = self.prefs.language;
        div()
            .id(id)
            .tab_index(0)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .cursor_pointer()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xffffff))
            .hover(|s| s.bg(rgb(PAPER)))
            .focus(|s| s.border_color(rgb(0x215d91)))
            .on_click(cx.listener(move |p, _, w, cx| {
                p.activate(control.clone(), cx);
                if let Some(side) = p.choosing {
                    w.focus(&p.picker_anchors[side as usize].focus);
                }
            }))
            .child(
                div()
                    .w(px(36.))
                    .h(px(20.))
                    .px(px(3.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .when(on, |d| d.justify_end())
                    .bg(rgb(if on { 0xad4c32 } else { 0x697171 }))
                    .child(div().size(px(14.)).rounded_full().bg(rgb(0xffffff))),
            )
            .child(if on {
                l.text("On", "オン")
            } else {
                l.text("Off", "オフ")
            })
    }

    fn nav(
        &self,
        id: &'static str,
        label: &'static str,
        page: Page,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.page == page;
        let icon = match page {
            Page::Mixer => "adjustments-horizontal.svg",
            Page::MicTest => "microphone.svg",
            Page::Devices => "plug-connected.svg",
            Page::Preferences => "settings.svg",
        };
        let color = if active { 0xffc4a6 } else { 0xe1e4e0 };
        div()
            .flex()
            .items_center()
            .gap_2()
            .id(id)
            .tab_index(0)
            .w_full()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(if active { 0x495150 } else { INK }))
            .bg(rgb(if active { 0x394140 } else { INK }))
            .text_color(rgb(color))
            .hover(|s| s.bg(rgb(0x394140)))
            .focus(|s| s.border_color(rgb(CORAL)))
            .on_click(cx.listener(move |p, _, _, cx| p.activate(Control::Page(page), cx)))
            .child(glyph(icon, 20., color))
            .child(label)
    }

    fn device_name(&self, side: Endpoint) -> String {
        let l = self.prefs.language;
        let (saved, list, empty) = match side {
            Endpoint::Input => (
                &self.prefs.input,
                &self.inputs,
                l.text("System default microphone", "システムの既定のマイク"),
            ),
            Endpoint::Output => (
                &self.prefs.output,
                &self.outputs,
                l.text("Select virtual output", "仮想出力を選択"),
            ),
            Endpoint::Monitor => (
                &self.prefs.monitor,
                &self.monitors,
                l.text("Select headphones", "ヘッドホンを選択"),
            ),
            Endpoint::Receiver => (
                &self.prefs.receiver,
                &self.receivers,
                l.text("Match virtual output", "仮想出力に合わせる"),
            ),
            Endpoint::TestSource => (
                &self.test.source,
                &self.test.sources,
                l.text("Select virtual microphone", "仮想マイクを選択"),
            ),
            Endpoint::TestSpeaker => (
                &self.test.speaker,
                &self.monitors,
                l.text("System default output", "システムの既定の出力"),
            ),
        };
        saved.as_ref().map_or_else(
            || empty.to_owned(),
            |id| {
                list.iter().find(|d| &d.0 == id).map_or_else(
                    || l.text("Device disconnected", "デバイス未接続").into(),
                    |d| d.1.clone(),
                )
            },
        )
    }

    fn state_label(&self) -> &'static str {
        let l = self.prefs.language;
        match self.status.as_ref().map(|s| s.message.as_str()) {
            Some(s) if s.starts_with("Streaming") => l.text("Live", "出力中"),
            Some(s) if s.contains("Choose a virtual output") => {
                l.text("Select an output in Devices", "デバイス画面で出力を選択")
            }
            Some(s) if s.contains("disconnected") => {
                l.text("Device disconnected", "デバイス未接続")
            }
            Some(_) => l.text("Check audio devices", "デバイスを確認"),
            None => l.text("Connecting", "接続中"),
        }
    }

    fn mic_test_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        #[cfg(test)]
        if let Some(screen) = self.adjustment_snapshot {
            return self.adjustment_page(screen, cx);
        }
        if let Some(adjust) = &self.test.adjust {
            return self.adjustment_page((&adjust.stage).into(), cx);
        }
        let recording = self.test.recording.as_ref();
        let action = self.test.primary_action();
        let length = recording
            .map(|r| r.duration())
            .or_else(|| self.test.clip.as_ref().map(|c| c.duration()))
            .unwrap_or_default();
        let status = match action {
            TestAction::FinishRecording => l.text("Recording", "録音中"),
            TestAction::StopPlayback => l.text("Playing", "再生中"),
            TestAction::Play => l.text("Recorded", "録音済み"),
            _ => self.state_label(),
        };
        card()
            .child(
                row()
                    .justify_between()
                    .child(heading(
                        l.text("Virtual microphone", "仮想マイク"),
                        "microphone.svg",
                        TERRACOTTA,
                    ))
                    .child(note(status)),
            )
            .child(self.picker(Endpoint::TestSource, cx))
            .child(heading(
                l.text("Playback device", "再生先"),
                "headphones.svg",
                TERRACOTTA,
            ))
            .child(self.picker(Endpoint::TestSpeaker, cx))
            .child(
                row()
                    .border_t_1()
                    .border_color(rgb(LINE))
                    .pt_3()
                    .child(self.button(
                        "test-primary",
                        action.label(l),
                        true,
                        Control::Test(action),
                        cx,
                    ))
                    .child(note(recording_length(length)))
                    .child(div().flex_1())
                    .when(self.test.clip.is_some(), |d| {
                        d.child(self.button(
                            "test-retake",
                            TestAction::Retake.label(l),
                            false,
                            Control::Test(TestAction::Retake),
                            cx,
                        ))
                    }),
            )
            .when(action == TestAction::FinishRecording, |d| {
                d.child(speech_prompt(l, (&self.test.reader).into()))
            })
    }

    fn adjustment_page(&self, screen: AdjustmentScreen, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        let mut panel = card().child(heading(
            l.text("Auto adjust", "自動調節"),
            "adjustments-horizontal.svg",
            TERRACOTTA,
        ));
        match screen {
            AdjustmentScreen::Room => {
                panel = panel
                    .child(note(self.device_name(Endpoint::Input)))
                    .child(div().text_lg().child(l.text(
                        "Stay quiet while Orvia samples the room",
                        "環境音を測っています。少し静かにしてください",
                    )))
                    .child(note(l.text(
                        "Speak at your usual distance when the numbers appear.",
                        "数字が表示されたら、普段の距離で話してください。",
                    )));
            }
            AdjustmentScreen::Phrase(take, cue) => {
                panel = panel
                    .child(note(format!(
                        "{} {} / 2",
                        l.text("Microphone reading", "マイクで読み上げ"),
                        take
                    )))
                    .child(speech_prompt(l, cue));
            }
            AdjustmentScreen::Analyzing => {
                panel = panel.child(note(
                    l.text("Analyzing voice and room noise…", "声と環境音を解析中…"),
                ))
            }
            AdjustmentScreen::Preview(cue) => {
                if let Some(cue) = cue {
                    panel = panel.child(speech_prompt(l, cue));
                } else {
                    panel = panel.child(note(
                        l.text("Preparing the virtual microphone…", "仮想マイクを準備中…"),
                    ));
                }
            }
            AdjustmentScreen::Restoring => {
                panel = panel.child(note(l.text("Preparing playback…", "録音を再生します…")));
            }
            AdjustmentScreen::Ready(result) => {
                let action = self.test.primary_action();
                let limit = if result.limit_db == Settings::UNLIMITED_ATTENUATION_DB {
                    l.text("Unlimited", "上限なし").into()
                } else {
                    format!("{} dB", result.limit_db)
                };
                let preview_label = if action == TestAction::StopPlayback {
                    l.text("Stop preview", "プレビューを停止")
                } else {
                    l.text("Replay preview", "もう一度聴く")
                };
                panel = panel
                    .child(
                        row()
                            .justify_between()
                            .child(l.text("Recommended limit", "おすすめの抑制上限"))
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(limit),
                            ),
                    )
                    .child(note(format!(
                        "{} {:.1} dB   ·   {} {:.1} dB",
                        l.text("Phrase onset loss", "声の出だしの減衰"),
                        result.onset_loss_db,
                        l.text("Room noise reduction", "環境音の低減"),
                        result.room_reduction_db
                    )))
                    .child(heading(
                        l.text("Virtual mic playback", "仮想マイクの録音を再生"),
                        "headphones.svg",
                        TERRACOTTA,
                    ))
                    .child(self.picker(Endpoint::TestSpeaker, cx))
                    .child(
                        row()
                            .child(self.button(
                                "adjust-preview",
                                preview_label,
                                false,
                                Control::Test(action),
                                cx,
                            ))
                            .child(self.button(
                                "adjust-apply",
                                l.text("Apply settings", "適用する"),
                                true,
                                Control::ApplyAdjustment,
                                cx,
                            )),
                    );
            }
            AdjustmentScreen::Failed(error) => panel = panel.child(note(error.to_owned())),
        }
        stack()
            .child(
                row()
                    .child(self.button(
                        "adjust-back",
                        l.text("Back to mixer", "ミキサーに戻る"),
                        false,
                        Control::Page(Page::Mixer),
                        cx,
                    ))
                    .when(
                        matches!(
                            screen,
                            AdjustmentScreen::Failed(_) | AdjustmentScreen::Ready(_)
                        ) && self.prefs.active_noise_method() == Some(NoiseMethod::DeepFilterNet3),
                        |r| {
                            r.child(self.button(
                                "adjust-retry",
                                l.text("Retake", "録り直す"),
                                false,
                                Control::AutoAdjust,
                                cx,
                            ))
                        },
                    ),
            )
            .child(panel)
    }

    fn devices(&self, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        stack()
            .child(
                card()
                    .child(heading(
                        l.text("Microphone input", "マイク入力"),
                        "microphone.svg",
                        TERRACOTTA,
                    ))
                    .child(self.picker(Endpoint::Input, cx)),
            )
            .child(
                card()
                    .child(heading(
                        l.text("Virtual microphone output", "仮想マイク出力"),
                        "route.svg",
                        TERRACOTTA,
                    ))
                    .child(self.picker(Endpoint::Output, cx))
                    .child(heading(
                        l.text(
                            "Virtual microphone recording endpoint",
                            "仮想マイクの録音側",
                        ),
                        "microphone.svg",
                        TERRACOTTA,
                    ))
                    .child(self.picker(Endpoint::Receiver, cx)),
            )
    }

    fn monitor(&self, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        let enabled = self.prefs.monitor_enabled;
        let status = self.status.as_ref();
        let warning = if !enabled {
            None
        } else if status.is_some_and(|s| s.monitor_error.is_some()) {
            Some(l.text(
                "Check receiver and headphones",
                "仮想マイクの録音側とヘッドホンを確認",
            ))
        } else if !status.is_some_and(|s| s.monitoring) {
            Some(l.text("Waiting for audio", "音声入力待ち"))
        } else {
            None
        };
        card()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(heading(
                        l.text("Headphone monitor", "ヘッドホンモニター"),
                        "headphones.svg",
                        TERRACOTTA,
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_right()
                            .when_some(warning, |d, message| d.child(note(message))),
                    )
                    .child(self.switch("monitor", enabled, Control::Monitor, cx)),
            )
            .child(self.picker(Endpoint::Monitor, cx))
    }

    fn preferences(&self, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        let mut langs = row();
        for (id, lang, name) in [
            ("system", Language::System, l.text("System", "システム設定")),
            ("english", Language::English, "English"),
            ("japanese", Language::Japanese, "日本語"),
        ] {
            langs = langs.child(self.button(id, name, l == lang, Control::Language(lang), cx));
        }
        stack()
            .child(
                card()
                    .child(heading(
                        l.text("Language", "表示言語"),
                        "language.svg",
                        TERRACOTTA,
                    ))
                    .child(langs),
            )
            .child(
                card()
                    .child(heading(
                        l.text("Application", "アプリ"),
                        "info-circle.svg",
                        TERRACOTTA,
                    ))
                    .child(note(format!("Orvia {}", self.installed)))
                    .child(
                        row()
                            .justify_between()
                            .child(l.text("Automatically check for updates", "更新を自動で確認"))
                            .child(self.switch(
                                "automatic-updates",
                                self.prefs.check_updates_automatically,
                                Control::AutomaticUpdates,
                                cx,
                            )),
                    )
                    .child(
                        row()
                            .child(self.button(
                                "updates",
                                l.text("Check for updates", "更新を確認"),
                                false,
                                Control::CheckUpdate,
                                cx,
                            ))
                            .child(self.button(
                                "releases",
                                l.text("Open releases", "リリースを開く"),
                                false,
                                Control::Releases,
                                cx,
                            )),
                    ),
            )
            .child(
                card()
                    .child(
                        row()
                            .justify_between()
                            .child(heading(
                                l.text("Audio engine", "音声エンジン"),
                                "activity.svg",
                                TERRACOTTA,
                            ))
                            .child(
                                row()
                                    .child(self.button(
                                        "view-log",
                                        l.text("Open log file", "ログファイルを開く"),
                                        false,
                                        Control::OpenLog,
                                        cx,
                                    ))
                                    .child(self.button(
                                        "restart-engine",
                                        l.text("Restart", "再起動"),
                                        false,
                                        Control::RestartEngine,
                                        cx,
                                    )),
                            ),
                    )
                    .child(self.engine_version()),
            )
    }

    fn engine_version(&self) -> gpui::Div {
        let l = self.prefs.language;
        let (icon, label, color) = if self.restarting {
            ("refresh.svg", l.text("Restarting", "再起動中"), MUTED)
        } else {
            match self.status.as_ref() {
                Some(s) if s.version == self.installed.to_string() => (
                    "check.svg",
                    l.text("Version matches", "バージョン一致"),
                    0x246345,
                ),
                Some(_) => (
                    "x.svg",
                    l.text("Version mismatch", "バージョン不一致"),
                    0xa03023,
                ),
                None => (
                    "info-circle.svg",
                    l.text("Awaiting engine", "エンジン応答待ち"),
                    MUTED,
                ),
            }
        };
        let version = self.status.as_ref().map_or("—", |s| s.version.as_str());
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(24.))
            .text_sm()
            .child(glyph(icon, 17., color))
            .child(div().text_color(rgb(color)).child(label))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_right()
                    .child(version.to_owned()),
            )
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.prefs.language;
        let toast = if !self.error.is_empty() {
            Some((
                updates::Notice::Error,
                self.error.clone(),
                Control::DismissError,
            ))
        } else if self.page == Page::Preferences && self.show_update_feedback {
            self.updates
                .status(l, &self.installed)
                .map(|(kind, message)| (kind, message, Control::DismissUpdate))
        } else {
            None
        };
        let tone_screen = self.tone_setup.as_ref().map(tone_setup::Setup::screen);
        #[cfg(test)]
        let tone_screen = self.tone_snapshot.or(tone_screen);
        let (title, content) = match self.page {
            Page::Mixer => match tone_screen {
                Some(screen) => (
                    l.text("Tone setup", "音質調整の自動設定"),
                    self.tone_page(screen, cx),
                ),
                None => (l.text("Mixer", "ミキサー"), self.mixer(window, cx)),
            },
            Page::MicTest => (l.text("Mic test", "マイクテスト"), self.mic_test_page(cx)),
            Page::Devices => (l.text("Devices", "デバイス"), self.devices(cx)),
            Page::Preferences => (l.text("Preferences", "設定"), self.preferences(cx)),
        };
        div()
            .id("workspace")
            .track_focus(&self.root)
            .key_context("Settings")
            .on_mouse_move(cx.listener(|p, event: &gpui::MouseMoveEvent, _, cx| {
                if p.dragging_slider.is_some() {
                    if event.pressed_button == Some(gpui::MouseButton::Left) {
                        p.move_slider(event.position.x, cx);
                    } else {
                        p.finish_slider();
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::release_slider))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::release_slider))
            .on_key_down(cx.listener(Self::picker_key))
            .on_action(cx.listener(|p, _: &NextControl, w, cx| {
                p.choosing = None;
                w.focus_next();
                cx.notify();
            }))
            .on_action(cx.listener(|p, _: &PreviousControl, w, cx| {
                p.choosing = None;
                w.focus_prev();
                cx.notify();
            }))
            .relative()
            .size_full()
            .text_color(rgb(INK))
            .bg(rgb(PAPER))
            .flex()
            .when(!self.test_only, |d| {
                d.child(
                    div()
                        .w(px(160.))
                        .flex_shrink_0()
                        .h_full()
                        .bg(rgb(INK))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            row()
                                .px_2()
                                .py_4()
                                .child(gpui::img(self.logo.clone()).size(px(30.)))
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xffffff))
                                        .child("Orvia"),
                                ),
                        )
                        .child(self.nav("nav-mixer", l.text("Mixer", "ミキサー"), Page::Mixer, cx))
                        .child(self.nav(
                            "nav-devices",
                            l.text("Devices", "デバイス"),
                            Page::Devices,
                            cx,
                        ))
                        .child(self.nav(
                            "nav-mic-test",
                            l.text("Mic test", "マイクテスト"),
                            Page::MicTest,
                            cx,
                        ))
                        .child(div().flex_1())
                        .child(self.nav(
                            "nav-prefs",
                            l.text("Preferences", "設定"),
                            Page::Preferences,
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .id("page-scroll")
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .on_scroll_wheel(cx.listener(|p, _, _, cx| {
                        if p.choosing.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .child(
                        stack()
                            .p_5()
                            .child(
                                div()
                                    .h(px(38.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_2xl()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(title),
                                    )
                                    .when_some(
                                        self.updates.available(&self.installed).filter(|_| {
                                            !self.test_only
                                                && self.prefs.check_updates_automatically
                                        }),
                                        |header, version| {
                                            header.child(self.button(
                                                "available-release",
                                                format!(
                                                    "{} · v{version}",
                                                    l.text("Update available", "更新があります")
                                                ),
                                                true,
                                                Control::Releases,
                                                cx,
                                            ))
                                        },
                                    ),
                            )
                            .child(content),
                    ),
            )
            .when_some(toast, |d, (kind, message, dismiss)| {
                let (icon, accent, background) = match kind {
                    updates::Notice::Error => ("alert-triangle.svg", TERRACOTTA, 0xffeadf),
                    updates::Notice::Success => ("check.svg", 0x246345, 0xe8f4ec),
                    updates::Notice::Progress => ("refresh.svg", 0x315e77, 0xedf4f7),
                    updates::Notice::Information => ("info-circle.svg", 0x315e77, 0xedf4f7),
                };
                d.child(
                    div()
                        .id("notification")
                        .absolute()
                        .bottom_2()
                        .left(if self.test_only { px(8.) } else { px(172.) })
                        .right_2()
                        .occlude()
                        .rounded_md()
                        .bg(rgb(background))
                        .border_1()
                        .border_color(rgb(accent))
                        .flex()
                        .items_center()
                        .p_2()
                        .gap_2()
                        .child(glyph(icon, 20., accent))
                        .child(div().flex_1().min_w_0().text_sm().child(message))
                        .child(self.button(
                            "dismiss-notification",
                            l.text("Dismiss", "閉じる"),
                            false,
                            dismiss,
                            cx,
                        )),
                )
            })
            .when_some(self.choosing, |workspace, side| {
                workspace.child(self.picker_menu(side, window, cx))
            })
    }
}
