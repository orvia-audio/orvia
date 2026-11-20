//! Layout and localized controls for the mixer, device routing and preferences.
use super::*;
use crate::icons::glyph;

#[path = "mixer.rs"]
mod mixer;
#[path = "picker.rs"]
mod picker;

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

/// Show the complete count at a fixed position and mark numbers heard in this take.
fn counting_prompt(l: Language, progress: usize) -> gpui::Div {
    stack()
        .gap_1()
        .child(note(l.text(
            "Count aloud from 1 to 5",
            "1から5まで声に出して数えてください",
        )))
        .child(
            row()
                .gap_4()
                .children((1..=reading::LAST_NUMBER).map(|number| {
                    div()
                        .text_xl()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .when(number <= progress, |d| {
                            d.text_color(rgb(TERRACOTTA)).underline()
                        })
                        .child(number.to_string())
                })),
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
            Control::AutoAdjust => Some("adjustments-horizontal.svg"),
            Control::BeginReading | Control::Test(TestAction::Record) => Some("microphone.svg"),
            Control::Test(TestAction::Play) => Some("player-play.svg"),
            Control::Test(TestAction::FinishRecording | TestAction::StopPlayback) => {
                Some("player-stop.svg")
            }
            Control::Test(TestAction::Retake) => Some("refresh.svg"),
            Control::RestartEngine => Some("refresh.svg"),
            Control::Page(Page::Log) => Some("activity.svg"),
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
        let primary = matches!(control, Control::BeginReading)
            || matches!(control, Control::Test(action) if action != TestAction::Retake);
        div()
            .id(id.into())
            .flex()
            .items_center()
            .gap_2()
            .when(wide, |d| d.w_full())
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
        let active = self.page == page || (self.page == Page::Log && page == Page::Preferences);
        let icon = match page {
            Page::Mixer => "adjustments-horizontal.svg",
            Page::MicTest => "microphone.svg",
            Page::Devices => "plug-connected.svg",
            Page::Preferences | Page::Log => "settings.svg",
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
        if let Some(adjust) = &self.test.adjust {
            return self.adjustment_page(adjust, cx);
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
        stack()
            .child(div().h(px(20.)).when_some(self.status.as_ref(), |d, s| {
                d.child(note(format!(
                    "{} v{}",
                    l.text("Running engine", "稼働中のエンジン"),
                    s.version
                )))
            }))
            .child(
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
                    .child(counting_prompt(
                        l,
                        if recording.is_some() {
                            self.test.reader.progress()
                        } else {
                            0
                        },
                    ))
                    .child(note(match action {
                        TestAction::FinishRecording => l.text(
                            "Recording stops after five.",
                            "5まで数えると録音が止まります。",
                        ),
                        TestAction::Record => l.text(
                            "Press Record to begin.",
                            "準備ができたら録音を押してください。",
                        ),
                        _ => "",
                    }))
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
                            .child(note(format!("{:.1} s", length.as_secs_f32())))
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
                    ),
            )
    }

    fn reading_prompt(
        &self,
        active: bool,
        reader: &reading::Reader,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let l = self.prefs.language;
        let mut body = stack().child(counting_prompt(l, reader.progress()));
        if active {
            body = body
                .child(heading(
                    l.text("Recording", "録音中"),
                    "microphone.svg",
                    TERRACOTTA,
                ))
                .child(note(l.text(
                    "Recording stops after five.",
                    "5まで数えると録音が止まります。",
                )));
        } else {
            body = body.child(row().child(self.button(
                "begin-reading",
                l.text("Start reading", "録音を始める"),
                true,
                Control::BeginReading,
                cx,
            )));
        }
        body
    }

    fn adjustment_page(
        &self,
        adjust: &auto_adjust::AutoAdjust,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        use auto_adjust::Stage;

        let l = self.prefs.language;
        let mut panel = card().child(heading(
            l.text("Auto adjust", "自動調節"),
            "adjustments-horizontal.svg",
            TERRACOTTA,
        ));
        match &adjust.stage {
            Stage::Room(_, _) => {
                panel = panel
                    .child(note(self.device_name(Endpoint::Input)))
                    .child(div().text_lg().child(l.text(
                        "Stay quiet while Orvia samples the room",
                        "環境音を測っています。少し静かにしてください",
                    )))
                    .child(note(l.text(
                        "Next, count twice at your usual pace.",
                        "このあと、普段の速さで2回数えます。",
                    )))
                    .child(counting_prompt(l, 0));
            }
            Stage::Phrase {
                first,
                recording,
                reader,
                ..
            } => {
                panel = panel
                    .child(note(format!(
                        "{} {} / 2",
                        l.text("Microphone reading", "マイクで読み上げ"),
                        if first.is_some() { 2 } else { 1 }
                    )))
                    .child(self.reading_prompt(recording.is_some(), reader, cx));
            }
            Stage::Analyzing(_) => {
                panel = panel.child(note(l.text(
                    "Finding a suppression limit…",
                    "声の出だしと環境音を解析中…",
                )))
            }
            Stage::PreviewReady(_) => {
                panel = panel
                    .child(note(
                        l.text("Virtual microphone preview", "仮想マイクのプレビュー"),
                    ))
                    .child(self.reading_prompt(false, &Default::default(), cx));
            }
            Stage::Auditioning {
                recording, reader, ..
            } => {
                panel = panel.child(note(
                    l.text("Virtual microphone preview", "仮想マイクのプレビュー"),
                ));
                if recording.is_some() {
                    panel = panel.child(self.reading_prompt(true, reader, cx));
                } else {
                    panel = panel.child(counting_prompt(l, 0)).child(note(l.text(
                        "Opening the recording endpoint…",
                        "仮想マイクの録音側を準備中…",
                    )));
                }
            }
            Stage::Restoring { .. } => {
                panel = panel.child(note(l.text("Preparing playback…", "録音を再生します…")));
            }
            Stage::Ready(result) => {
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
                                l.text("Apply", "適用"),
                                true,
                                Control::ApplyAdjustment,
                                cx,
                            )),
                    );
            }
            Stage::Failed(error) => panel = panel.child(note(error.clone())),
        }
        stack().child(panel).child(
            row()
                .child(self.button(
                    "adjust-back",
                    l.text("Back to mixer", "ミキサーに戻る"),
                    false,
                    Control::Page(Page::Mixer),
                    cx,
                ))
                .when(
                    matches!(adjust.stage, Stage::Failed(_) | Stage::Ready(_))
                        && self.prefs.active_noise_method() == Some(NoiseMethod::DeepFilterNet3),
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
                            .child(note(warning.unwrap_or(""))),
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
                    .child(note(format!("Orvia {}", env!("CARGO_PKG_VERSION"))))
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
                    )
                    .child(
                        div()
                            .id("update-result")
                            .h(px(24.))
                            .overflow_y_scroll()
                            .child(note(self.updates.status(l, &self.installed))),
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
                                        l.text("View log", "ログを表示"),
                                        false,
                                        Control::Page(Page::Log),
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
                Some(s) if s.version == env!("CARGO_PKG_VERSION") => (
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

    fn log_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        use orvia_core::EngineEvent;
        let l = self.prefs.language;
        let mut list = card().gap_0().py_1();
        for entry in self.log.entries.iter().rev() {
            let (level, color) = match entry.event {
                EngineEvent::RouteError(_) | EngineEvent::MonitorError(_) => {
                    (l.text("Error", "エラー"), 0xa03023)
                }
                EngineEvent::Reconnecting => (l.text("Warning", "警告"), 0x835313),
                _ => (l.text("Info", "情報"), MUTED),
            };
            list = list.child(
                stack()
                    .gap_0()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(LINE))
                    .child(
                        row()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(entry.timestamp.clone())
                            .child(div().text_color(rgb(color)).child(level)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(engine_log::description(&entry.event, l)),
                    ),
            );
        }
        stack()
            .child(row().child(self.button(
                "back-to-prefs",
                l.text("Back to preferences", "設定に戻る"),
                false,
                Control::Page(Page::Preferences),
                cx,
            )))
            .when_some(self.log.error.as_ref(), |d, e| {
                d.child(note(format!(
                    "{} · {e}",
                    l.text("Cannot read log", "ログを読み込めません")
                )))
            })
            .when(
                self.log.entries.is_empty() && self.log.error.is_none(),
                |d| d.child(note(l.text("No events yet", "ログはまだありません"))),
            )
            .when(!self.log.entries.is_empty(), |d| d.child(list))
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.prefs.language;
        let (title, content) = match self.page {
            Page::Mixer => (l.text("Mixer", "ミキサー"), self.mixer(cx)),
            Page::MicTest => (l.text("Mic test", "マイクテスト"), self.mic_test_page(cx)),
            Page::Log => (l.text("Engine log", "エンジンログ"), self.log_page(cx)),
            Page::Devices => (l.text("Devices", "デバイス"), self.devices(cx)),
            Page::Preferences => (l.text("Preferences", "設定"), self.preferences(cx)),
        };
        div()
            .id("workspace")
            .track_focus(&self.root)
            .key_context("Settings")
            .on_mouse_move(cx.listener(|p, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some(slider) = p.dragging_slider {
                    if event.pressed_button == Some(gpui::MouseButton::Left) {
                        p.slide_at(slider, event.position.x, cx);
                    } else {
                        p.dragging_slider = None;
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|p, event: &gpui::MouseUpEvent, _, cx| {
                    if let Some(slider) = p.dragging_slider.take() {
                        p.slide_at(slider, event.position.x, cx);
                    }
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|p, event: &gpui::MouseUpEvent, _, cx| {
                    if let Some(slider) = p.dragging_slider.take() {
                        p.slide_at(slider, event.position.x, cx);
                    }
                }),
            )
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
                    .id(match self.page {
                        Page::Log => "log-scroll",
                        _ => "page-scroll",
                    })
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
            .when(!self.error.is_empty(), |d| {
                d.child(
                    div()
                        .id("panel-error")
                        .absolute()
                        .bottom_2()
                        .left(if self.test_only { px(8.) } else { px(172.) })
                        .right_2()
                        .occlude()
                        .rounded_md()
                        .bg(rgb(0xffeadf))
                        .border_1()
                        .border_color(rgb(TERRACOTTA))
                        .flex()
                        .items_center()
                        .p_2()
                        .gap_2()
                        .child(div().flex_1().min_w_0().text_sm().child(self.error.clone()))
                        .child(self.button(
                            "dismiss-error",
                            l.text("Dismiss", "閉じる"),
                            false,
                            Control::DismissError,
                            cx,
                        )),
                )
            })
            .when_some(self.choosing, |workspace, side| {
                workspace.child(self.picker_menu(side, window, cx))
            })
    }
}
