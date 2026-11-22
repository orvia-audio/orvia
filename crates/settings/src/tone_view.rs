//! Tone control labels and the guided receiver recording.
use super::*;
use orvia_core::Tone;
use tone_setup::{Action, Screen};

pub(super) fn tone_labels(l: Language, tone: Tone) -> [(&'static str, String); 3] {
    let off = l.text("Off", "オフ");
    [
        (
            l.text("Low cut", "ローカット"),
            if tone.low_cut_hz == 0 {
                off.into()
            } else {
                format!("{} Hz", tone.low_cut_hz)
            },
        ),
        (l.text("Gain", "ゲイン"), format!("{}%", tone.gain_percent)),
        (
            l.text("Compressor", "コンプレッサー"),
            if tone.compressor_ratio == 1 {
                off.into()
            } else {
                format!("{}:1", tone.compressor_ratio)
            },
        ),
    ]
}

impl Panel {
    pub(super) fn tone_page(&self, screen: Screen<'_>, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        let button = |id, label, selected, action, cx: &mut Context<Self>| {
            self.button(id, label, selected, Control::Tone(action), cx)
                .into_any_element()
        };
        let body = card();
        let body = match screen {
            Screen::Intro => body
                .child(heading(l.text("Record your room and voice", "環境音と声を録音"), "microphone.svg", TERRACOTTA))
                .child(note(l.text("Make the low sound you want to reduce, then read aloud at your usual distance.", "抑えたい低音を出してから、いつもの距離で数字を読み上げます。")))
                .child(note(l.text("While recording, the virtual mic uses 100% gain with low cut and compression off.", "録音中の仮想マイクはゲイン100%になり、ローカットとコンプレッサーが一時的にオフになります。")))
                .child(row().child(button("tone-start", l.text("Start recording", "録音を始める"), true, Action::Start, cx))),
            Screen::Opening => body.child(note(l.text("Preparing the virtual microphone…", "仮想マイクを準備中…"))),
            Screen::Room => body
                .child(heading(l.text("Make the low sound you want to reduce", "抑えたい低音を出してください"), "wave-sine.svg", TERRACOTTA))
                .child(note(l.text("For example, tap your desk. For steady rumble, just listen for a moment.", "たとえば、机に軽く触れてみてください。常に鳴っている音なら、そのまま少し待ちます。")))
                .child(row().child(button("tone-read", l.text("Next · Read aloud", "次へ · 読み上げ"), true, Action::Read, cx))),
            Screen::Reading(cue) => body.child(speech_prompt(l, cue)),
            Screen::Analyzing => body.child(note(l.text("Analyzing your room and voice…", "環境音と声を解析中…"))),
            Screen::Ready(tone, playing) => {
                let mut table = stack().gap_1();
                for ((name, current), (_, recommended)) in tone_labels(l, self.prefs.tone()).into_iter().zip(tone_labels(l, tone)) {
                    table = table.child(row().justify_between().border_b_1().border_color(rgb(LINE)).py_2().child(name)
                        .child(row().child(note(current)).child(note("→")).child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(recommended))));
                }
                body.child(heading(l.text("Recommended settings", "おすすめの設定"), "adjustments-horizontal.svg", TERRACOTTA))
                    .child(table)
                    .child(row().child(button("tone-current", l.text("Current", "現在の音"), playing == Some(0), Action::Play(0), cx))
                        .child(button("tone-recommended", l.text("Recommended", "おすすめの音"), playing == Some(1), Action::Play(1), cx)))
                    .child(row().child(button("tone-apply", l.text("Apply settings", "適用する"), true, Action::Apply, cx)))
            }
            Screen::Failed(message) => body.child(heading(l.text("Recording unavailable", "録音を確認できませんでした"), "alert-triangle.svg", TERRACOTTA))
                .child(note(message.to_owned())).child(row().child(button("tone-retry", l.text("Try again", "録り直す"), true, Action::Start, cx))),
        };
        stack().child(body).child(row().child(self.button(
            "tone-back",
            l.text("Back to mixer", "ミキサーに戻る"),
            false,
            Control::Page(Page::Mixer),
            cx,
        )))
    }
}
