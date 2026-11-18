//! Layout and localized controls for the mixer, device routing and preferences.
use super::*;

#[path = "mixer.rs"]
mod mixer;

const INK: u32 = 0x242a2b;
const MUTED: u32 = 0x575f61;
const PAPER: u32 = 0xf7f5f1;
const LINE: u32 = 0xdedbd4;
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
fn heading(s: &'static str) -> gpui::Div {
    div()
        .text_base()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(s)
}
fn note(s: impl Into<SharedString>) -> gpui::Div {
    div().text_sm().text_color(rgb(MUTED)).child(s.into())
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
        div()
            .id(id.into())
            .tab_index(0)
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
            .focus(|s| s.border_2().border_color(rgb(0x215d91)))
            .on_click(cx.listener(move |p, _, _, cx| p.activate(control.clone(), cx)))
            .child(label.into())
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
            .on_click(cx.listener(move |p, _, _, cx| p.activate(control.clone(), cx)))
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
        div()
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
            .text_color(rgb(if active { 0xffc4a6 } else { 0xe1e4e0 }))
            .hover(|s| s.bg(rgb(0x394140)))
            .focus(|s| s.border_color(rgb(CORAL)))
            .on_click(cx.listener(move |p, _, _, cx| p.activate(Control::Page(page), cx)))
            .child(label)
    }

    fn device_name(&self, side: Endpoint) -> String {
        let l = self.prefs.language;
        let (saved, list, empty) = match side {
            Endpoint::Input => (
                &self.prefs.input,
                &self.inputs,
                l.text("Windows default microphone", "Windows の既定のマイク"),
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

    fn picker(&self, side: Endpoint, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.prefs.language;
        let (id, list, selected) = match side {
            Endpoint::Input => ("input", &self.inputs, &self.prefs.input),
            Endpoint::Output => ("output", &self.outputs, &self.prefs.output),
            Endpoint::Monitor => ("headphones", &self.monitors, &self.prefs.monitor),
        };
        let open = self.choosing == Some(side);
        let mut view = stack().gap_1().child(self.button(
            id,
            format!("{}  ▾", self.device_name(side)),
            open,
            Control::Choose(side),
            cx,
        ));
        if open {
            if side == Endpoint::Input {
                view = view.child(self.button(
                    "default-input",
                    l.text("Windows default microphone", "Windows の既定のマイク"),
                    selected.is_none(),
                    Control::Select(side, None),
                    cx,
                ));
            }
            if list.is_empty() {
                view = view.child(note(match side {
                    Endpoint::Output => l.text(
                        "Install a supported virtual cable, then reopen the list.",
                        "対応する仮想ケーブルを導入して一覧を開き直してください。",
                    ),
                    _ => l.text(
                        "Connect a device and reopen the list.",
                        "デバイスを接続して一覧を開き直してください。",
                    ),
                }));
            }
            for (id, name) in list {
                view = view.child(self.button(
                    id.clone(),
                    name.clone(),
                    selected.as_ref() == Some(id),
                    Control::Select(side, Some(id.clone())),
                    cx,
                ));
            }
        }
        view
    }

    fn state_label(&self) -> &'static str {
        let l = self.prefs.language;
        if self.prefs.muted {
            return l.text("Muted", "ミュート中");
        }
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

    fn devices(&self, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        let monitor_on = self.prefs.monitor_enabled;
        let status = self.status.as_ref();
        let message = if self.prefs.muted {
            l.text("Output muted", "出力はミュート中です")
        } else if status.is_some_and(|s| s.monitor_error.is_some()) {
            l.text(
                "Headphones unavailable. The virtual output is independent.",
                "ヘッドホンに接続できません。仮想出力は引き続き使用できます。",
            )
        } else if status.is_some_and(|s| s.monitoring) {
            l.text("Playing processed audio", "加工後の音声を再生中")
        } else {
            l.text("Waiting for the audio route", "音声経路の接続待ち")
        };
        stack()
            .child(
                card()
                    .child(heading(l.text("Microphone input", "マイク入力")))
                    .child(self.picker(Endpoint::Input, cx)),
            )
            .child(
                card()
                    .child(heading(
                        l.text("Virtual microphone output", "仮想マイク出力"),
                    ))
                    .child(self.picker(Endpoint::Output, cx))
                    .child(note(l.text(
                        "Choose the recording side of this cable in your game or recorder.",
                        "ゲームや録音アプリでは、このケーブルの録音側を選びます。",
                    ))),
            )
            .child(
                card()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(heading(l.text("Headphone monitor", "ヘッドホンモニター")))
                            .child(self.switch("monitor", monitor_on, Control::Monitor, cx)),
                    )
                    .child(self.picker(Endpoint::Monitor, cx))
                    .child(note(if monitor_on {
                        message
                    } else {
                        l.text(
                            "Listen to the processed microphone on a separate device.",
                            "加工後のマイク音を選んだヘッドホンで確認できます。",
                        )
                    })),
            )
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
                    .child(heading(l.text("Language", "表示言語")))
                    .child(langs),
            )
            .child(
                card()
                    .child(heading(l.text("Application", "アプリ")))
                    .child(note(format!("Orvia {}", env!("CARGO_PKG_VERSION"))))
                    .child(note(l.text(
                        "Closing the window keeps Orvia in the notification area.",
                        "画面を閉じても通知領域の Orvia は動作を続けます。",
                    )))
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
                    .when(!self.update.is_empty(), |v| {
                        v.child(note(self.update.clone()))
                    }),
            )
            .when_some(self.status.as_ref(), |v, s| {
                v.child(
                    card()
                        .child(heading(l.text("Audio diagnostics", "音声診断")))
                        .child(note(s.message.clone()))
                        .when_some(s.monitor_error.clone(), |v, e| v.child(note(e)))
                        .child(note(format!(
                            "{} {}  ·  {} {}",
                            l.text("Missing frames", "不足フレーム"),
                            s.missing_frames,
                            l.text("Dropped frames", "破棄フレーム"),
                            s.dropped_frames
                        ))),
                )
            })
    }
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let l = self.prefs.language;
        let (title, content) = match self.page {
            Page::Mixer => (l.text("Mixer", "ミキサー"), self.mixer(cx)),
            Page::Devices => (l.text("Devices", "デバイス"), self.devices(cx)),
            Page::Preferences => (l.text("Preferences", "設定"), self.preferences(cx)),
        };
        div()
            .id("workspace")
            .track_focus(&self.root)
            .key_context("Settings")
            .on_mouse_move(cx.listener(|p, event: &gpui::MouseMoveEvent, _, cx| {
                if p.dragging_gain {
                    if event.pressed_button == Some(gpui::MouseButton::Left) {
                        p.gain_at(event.position.x, cx);
                    } else {
                        p.dragging_gain = false;
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|p, event: &gpui::MouseUpEvent, _, cx| {
                    if p.dragging_gain {
                        p.gain_at(event.position.x, cx);
                        p.dragging_gain = false;
                    }
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|p, event: &gpui::MouseUpEvent, _, cx| {
                    if p.dragging_gain {
                        p.gain_at(event.position.x, cx);
                        p.dragging_gain = false;
                    }
                }),
            )
            .on_action(|_: &NextControl, w, _| w.focus_next())
            .on_action(|_: &PreviousControl, w, _| w.focus_prev())
            .size_full()
            .font_family("Segoe UI")
            .text_color(rgb(INK))
            .bg(rgb(PAPER))
            .flex()
            .child(
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
                    .child(div().flex_1())
                    .child(self.nav(
                        "nav-prefs",
                        l.text("Preferences", "設定"),
                        Page::Preferences,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("page-scroll")
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(
                        stack()
                            .p_5()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(content)
                            .when(!self.error.is_empty(), |v| {
                                v.child(note(self.error.clone()))
                            }),
                    ),
            )
    }
}
