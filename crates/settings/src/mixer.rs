//! Mixer controls and the two-second, pre-gain peak envelope display.
use super::*;
use gpui::{MouseButton, Pixels, canvas, fill, point};

const INPUT_COLOR: u32 = 0xb2d3dd;
const OUTPUT_COLOR: u32 = 0xffaa82;
const HISTORY_BLOCKS: usize = 200;

fn history(levels: Vec<[u8; 2]>, filtered: bool) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let w = bounds.size.width;
            let h = bounds.size.height;
            for fraction in [0., 0.5, 1.] {
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x, origin.y + (h - px(1.)) * fraction),
                        size(w, px(1.)),
                    ),
                    rgb(0x495253),
                ));
            }
            let stride = w / HISTORY_BLOCKS as f32;
            if levels.len() < 2 {
                return;
            }
            let start = origin.x + w - stride * levels.len() as f32;
            let bottom = origin.y + h - px(1.);
            for (channel, color) in [INPUT_COLOR, OUTPUT_COLOR]
                .into_iter()
                .take(if filtered { 2 } else { 1 })
                .enumerate()
            {
                let mut trace = gpui::PathBuilder::stroke(px(if channel == 0 { 2.5 } else { 1.5 }));
                let mut area = gpui::PathBuilder::fill();
                area.move_to(point(start, bottom));
                for (i, block) in levels.iter().enumerate() {
                    let x = start + stride * i as f32;
                    let y = bottom - (h - px(2.)) * (f32::from(block[channel]) / 60.);
                    let p = point(x, y);
                    if i == 0 {
                        trace.move_to(p);
                    } else {
                        trace.line_to(p);
                    }
                    if channel == 0 {
                        area.line_to(p);
                    }
                }
                if channel == 0 {
                    area.line_to(point(origin.x + w - stride, bottom));
                    area.close();
                    if let Ok(path) = area.build() {
                        window.paint_path(path, gpui::rgba(0xb2d3dd23));
                    }
                }
                if let Ok(path) = trace.build() {
                    window.paint_path(path, rgb(color));
                }
            }
        },
    )
    .w_full()
    .h(px(62.))
}

fn legend(label: &'static str, color: u32) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .child(div().size(px(8.)).rounded_full().bg(rgb(color)))
        .child(label)
}

impl Panel {
    pub(super) fn gain_at(&mut self, x: Pixels, cx: &mut Context<Self>) {
        if let Some(bounds) = self.gain_bounds.get().filter(|b| b.size.width > px(0.)) {
            let ratio = ((x - bounds.origin.x) / bounds.size.width).clamp(0., 1.);
            let value = (ratio * 200.).round() as u16;
            if value != self.prefs.gain_percent {
                self.activate(Control::Gain(value), cx);
            }
        }
    }

    fn slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let measured = self.gain_bounds.clone();
        let fraction = f32::from(self.prefs.gain_percent.min(200)) / 200.;
        let track = canvas(
            move |bounds, _, _| measured.set(Some(bounds)),
            move |bounds, _, window, _| {
                let x = bounds.origin.x;
                let y = bounds.center().y;
                let w = bounds.size.width;
                window.paint_quad(fill(
                    Bounds::new(point(x, y - px(2.)), size(w, px(4.))),
                    rgb(LINE),
                ));
                window.paint_quad(fill(
                    Bounds::new(point(x, y - px(2.)), size(w * fraction, px(4.))),
                    rgb(0xa9472c),
                ));
                window.paint_quad(fill(
                    Bounds::new(point(x + w * 0.5, y - px(4.)), size(px(1.), px(8.))),
                    rgb(MUTED),
                ));
                let mut thumb = fill(
                    Bounds::new(
                        point(x + w * fraction - px(8.), y - px(8.)),
                        size(px(16.), px(16.)),
                    ),
                    rgb(0x9d4029),
                );
                thumb.corner_radii = px(8.).into();
                window.paint_quad(thumb);
            },
        )
        .w_full()
        .h(px(25.));
        div()
            .id("gain-slider")
            .track_focus(&self.gain_focus)
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xffffff))
            .focus(|s| s.border_color(rgb(0x215d91)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|p, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&p.gain_focus);
                    p.dragging_gain = true;
                    p.gain_at(event.position.x, cx);
                }),
            )
            .on_key_down(cx.listener(|p, event: &gpui::KeyDownEvent, _, cx| {
                let value = p.prefs.gain_percent.min(200);
                let step = if event.keystroke.modifiers.shift {
                    10
                } else {
                    1
                };
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => value.saturating_sub(step),
                    "right" | "up" => (value + step).min(200),
                    "home" => 0,
                    "end" => 200,
                    _ => return,
                };
                p.activate(Control::Gain(next), cx);
                cx.stop_propagation();
            }))
            .child(track)
    }

    pub(super) fn mixer(&self, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        let live = self
            .status
            .as_ref()
            .filter(|s| !self.prefs.muted && s.message.starts_with("Streaming"));
        let outdated = live
            .is_some_and(|s| s.signal.peaks.is_empty() && s.version != env!("CARGO_PKG_VERSION"));
        let levels = live.map(|s| s.signal.peaks.as_slice()).unwrap_or_default();
        let peak = live.map_or(0., |s| s.peak);
        let output = if peak > 0.00001 {
            format!("{:.1} dBFS", 20. * peak.log10())
        } else {
            "−∞ dBFS".into()
        };
        let change = live
            .and_then(|s| s.signal.attenuation_db)
            .map_or_else(|| "—".into(), |v| format!("{:.1} dB", v));
        let selected = self.prefs.active_noise_method();
        let filtered = selected.is_some();
        let mut methods = row();
        for (id, label, method) in [
            ("bypass", l.text("Off", "オフ"), None),
            ("rnnoise", "RNNoise", Some(NoiseMethod::Rnnoise)),
            (
                "spectral",
                l.text("Spectral subtraction", "スペクトル減算"),
                Some(NoiseMethod::Spectral),
            ),
        ] {
            methods = methods.child(self.button(
                id,
                label,
                selected == method,
                Control::Noise(method),
                cx,
            ));
        }
        let mut mix = row();
        for (id, label, mode) in [
            ("stereo", l.text("Stereo", "ステレオ"), Mixing::Stereo),
            ("mono", l.text("Mono", "モノラル"), Mixing::Average),
        ] {
            mix = mix.child(self.button(
                id,
                label,
                self.prefs.mixing == mode,
                Control::Mix(mode),
                cx,
            ));
        }
        let summary = stack()
            .gap_2()
            .p_4()
            .rounded_xl()
            .bg(rgb(INK))
            .text_color(rgb(0xffffff))
            .child(
                row()
                    .justify_between()
                    .child(heading(
                        l.text("Signal history", "信号の推移"),
                        "wave-sine.svg",
                        CORAL,
                    ))
                    .child(div().text_sm().text_color(rgb(CORAL)).child(if outdated {
                        l.text("Restart engine in Preferences", "設定でエンジンを再起動")
                    } else {
                        self.state_label()
                    })),
            )
            .child(
                row()
                    .justify_between()
                    .child(
                        row()
                            .child(legend(
                                if filtered {
                                    l.text("Before", "抑制前")
                                } else {
                                    l.text("Input", "入力")
                                },
                                INPUT_COLOR,
                            ))
                            .when(filtered, |d| {
                                d.child(legend(l.text("After", "抑制後"), OUTPUT_COLOR))
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xd5dcd9))
                            .child(l.text("2 s   ·   −60 to 0 dBFS", "2 秒   ·   −60 ～ 0 dBFS")),
                    ),
            )
            .child(history(levels.to_vec(), filtered))
            .child(
                row()
                    .justify_between()
                    .when(filtered, |d| {
                        d.child(div().text_sm().child(format!(
                            "{}  {change}",
                            l.text("Pre-gain attenuation", "ゲイン適用前の減衰量")
                        )))
                    })
                    .child(
                        div()
                            .text_sm()
                            .child(format!("{}  {output}", l.text("Output", "出力"))),
                    ),
            );
        let signal = card()
            .gap_1()
            .child(
                row()
                    .justify_between()
                    .child(heading(
                        l.text("Output gain", "出力ゲイン"),
                        "volume.svg",
                        TERRACOTTA,
                    ))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("{}%", self.prefs.gain_percent)),
                    ),
            )
            .child(self.slider(cx))
            .child(
                div()
                    .px_2()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("0%")
                    .child("100%")
                    .child("200%"),
            );
        stack()
            .gap_2()
            .child(summary)
            .child(
                card()
                    .gap_2()
                    .child(
                        row()
                            .justify_between()
                            .child(heading(
                                l.text("Noise suppression", "ノイズ抑制"),
                                "filter.svg",
                                TERRACOTTA,
                            ))
                            .child(methods),
                    )
                    .child(
                        row()
                            .justify_between()
                            .child(heading(
                                l.text("Channels", "チャンネル"),
                                "arrows-split-2.svg",
                                TERRACOTTA,
                            ))
                            .child(mix),
                    ),
            )
            .child(signal)
            .child(self.monitor(cx))
    }
}
