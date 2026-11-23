//! Mixer controls and switchable signal displays.
use super::*;
use gpui::{MouseButton, canvas, fill, point};

const INPUT_COLOR: u32 = 0xb2d3dd;
const OUTPUT_COLOR: u32 = 0xffaa82;
const HISTORY_BLOCKS: usize = 200;

fn needs_restart(status: &EngineStatus, spectrum: bool) -> bool {
    status.version != env!("CARGO_PKG_VERSION")
        && if spectrum {
            status.spectrum.is_none()
        } else {
            status.signal.peaks.is_empty()
        }
}

fn output_label(peak: f32) -> String {
    if peak > 0.00001 {
        format!("{:.1} dBFS", 20. * peak.log10())
    } else {
        "−∞ dBFS".into()
    }
}

fn attenuation_label(db: Option<f32>) -> String {
    db.map_or_else(|| "—".into(), |db| format!("{db:.1} dB"))
}

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
    /// Compare the current mixer display, including its visible traces and rounded meter labels.
    pub(crate) fn mixer_status_changed(
        old: &EngineStatus,
        new: &EngineStatus,
        spectrum: bool,
        filtered: bool,
    ) -> bool {
        if old.message != new.message
            || old.version != new.version
            || old.monitor_error != new.monitor_error
            || old.monitoring != new.monitoring
        {
            return true;
        }
        if !new.message.starts_with("Streaming") {
            return false;
        }
        if spectrum {
            return old.spectrum != new.spectrum;
        }
        if needs_restart(old, false) != needs_restart(new, false) {
            return true;
        }
        let before = &old.signal.peaks;
        let after = &new.signal.peaks;
        if before.len() >= 2 || after.len() >= 2 {
            let changed = if filtered {
                before != after
            } else {
                before.iter().map(|p| p[0]).ne(after.iter().map(|p| p[0]))
            };
            if changed {
                return true;
            }
        }
        let old_db = old.signal.attenuation_db;
        let new_db = new.signal.attenuation_db;
        (old.peak != new.peak && output_label(old.peak) != output_label(new.peak))
            || (filtered
                && old_db.map(f32::to_bits) != new_db.map(f32::to_bits)
                && attenuation_label(old_db) != attenuation_label(new_db))
    }

    fn slider(&self, slider: Slider, cx: &mut Context<Self>) -> impl IntoElement {
        let anchor = &self.slider_anchors[slider as usize];
        let measured = anchor.bounds.clone();
        let fraction = f32::from(self.slider_value(slider)) / f32::from(slider.max());
        let track = canvas(
            move |bounds, _, _| measured.set(bounds),
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
                if slider == Slider::Gain {
                    window.paint_quad(fill(
                        Bounds::new(point(x + w * 0.5, y - px(4.)), size(px(1.), px(8.))),
                        rgb(MUTED),
                    ));
                }
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
            .id(match slider {
                Slider::Gain => "gain-slider",
                Slider::LowCut => "low-cut-slider",
                Slider::Compression => "compression-slider",
                Slider::Suppression => "suppression-slider",
            })
            .track_focus(&anchor.focus)
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0xffffff))
            .focus(|s| s.border_color(rgb(0x215d91)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |p, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&p.slider_anchors[slider as usize].focus);
                    p.start_slider(slider, event.position.x, cx);
                }),
            )
            .on_key_down(cx.listener(move |p, event: &gpui::KeyDownEvent, _, cx| {
                let value = p.slider_value(slider);
                let step = if event.keystroke.modifiers.shift {
                    10
                } else {
                    1
                };
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => value.saturating_sub(step),
                    "right" | "up" => (value + step).min(slider.max()),
                    "home" => 0,
                    "end" => slider.max(),
                    _ => return,
                };
                p.activate(Control::Slide(slider, next), cx);
                cx.stop_propagation();
            }))
            .child(track)
    }

    pub(super) fn mixer(&self, cx: &mut Context<Self>) -> gpui::Div {
        let l = self.prefs.language;
        let live = self
            .status
            .as_ref()
            .filter(|s| s.message.starts_with("Streaming"));
        let outdated = live.is_some_and(|s| needs_restart(s, self.show_spectrum));
        let levels = live.map(|s| s.signal.peaks.as_slice()).unwrap_or_default();
        let selected = self.prefs.active_noise_method();
        let filtered = selected.is_some();
        let mut methods = row();
        for (id, label, method) in [
            ("bypass", l.text("Off", "オフ"), None),
            (
                "deepfilternet3",
                "DeepFilterNet3",
                Some(NoiseMethod::DeepFilterNet3),
            ),
            ("rnnoise", "RNNoise", Some(NoiseMethod::Rnnoise)),
            ("webrtc", "WebRTC", Some(NoiseMethod::WebRtc)),
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
                    .child(
                        row()
                            .gap_1()
                            .child(glyph("wave-sine.svg", 20., CORAL))
                            .children(
                                [
                                    (false, l.text("Signal history", "信号の推移")),
                                    (true, l.text("Spectrum", "スペクトル")),
                                ]
                                .into_iter()
                                .map(|(spectrum, label)| {
                                    let selected = self.show_spectrum == spectrum;
                                    div()
                                        .id(if spectrum {
                                            "show-spectrum"
                                        } else {
                                            "show-history"
                                        })
                                        .tab_index(0)
                                        .h(px(24.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(if selected { 0x747f7e } else { INK }))
                                        .bg(rgb(if selected { 0x404a4a } else { INK }))
                                        .text_sm()
                                        .text_color(rgb(if selected { 0xffffff } else { 0xc6cfcd }))
                                        .hover(|s| s.bg(rgb(0x3e4747)))
                                        .focus(|s| s.border_color(rgb(CORAL)))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |p, _, _, cx| {
                                            p.activate(Control::ShowSpectrum(spectrum), cx)
                                        }))
                                        .child(label)
                                }),
                            ),
                    )
                    .child(div().text_sm().text_color(rgb(CORAL)).child(if outdated {
                        l.text("Restart engine in Preferences", "設定でエンジンを再起動")
                    } else {
                        self.state_label()
                    })),
            );
        let summary =
            if self.show_spectrum {
                self.spectrum_chart(summary, live.and_then(|s| s.spectrum.as_ref()), cx)
            } else {
                summary
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
                            .child(div().text_xs().text_color(rgb(0xd5dcd9)).child(
                                l.text("2 s   ·   −60 to 0 dBFS", "2 秒   ·   −60 ～ 0 dBFS"),
                            )),
                    )
                    .child(history(levels.to_vec(), filtered))
                    .child(
                        row()
                            .justify_between()
                            .child(div().flex_1().when(filtered, |d| {
                                let change =
                                    attenuation_label(live.and_then(|s| s.signal.attenuation_db));
                                d.text_sm().child(format!(
                                    "{}  {change}",
                                    l.text("Pre-gain attenuation", "ゲイン適用前の減衰量")
                                ))
                            }))
                            .child(div().text_sm().child(format!(
                                "{}  {}",
                                l.text("Output", "出力"),
                                output_label(live.map_or(0., |s| s.peak))
                            ))),
                    )
            };
        let tone = orvia_core::Tone {
            low_cut_hz: self.slider_value(Slider::LowCut),
            gain_percent: self.slider_value(Slider::Gain),
            compressor_ratio: self.slider_value(Slider::Compression) + 1,
        };
        let mut controls = stack().gap_1();
        for ((label, value), slider) in tone_labels(l, tone).into_iter().zip([
            Slider::LowCut,
            Slider::Gain,
            Slider::Compression,
        ]) {
            controls = controls.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(div().w(px(110.)).flex_shrink_0().child(label))
                    .child(div().flex_1().min_w_0().child(self.slider(slider, cx)))
                    .child(
                        div()
                            .w(px(55.))
                            .flex_shrink_0()
                            .text_right()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(value),
                    ),
            );
        }
        let signal = card()
            .gap_2()
            .child(
                row()
                    .justify_between()
                    .child(heading(
                        l.text("Tone", "音質調整"),
                        "adjustments-horizontal.svg",
                        TERRACOTTA,
                    ))
                    .child(self.button(
                        "tone-setup",
                        l.text("Auto setup", "自動設定"),
                        false,
                        Control::Tone(tone_setup::Action::Open),
                        cx,
                    )),
            )
            .child(controls);
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
                    .when(selected == Some(NoiseMethod::DeepFilterNet3), |card| {
                        let db = self.slider_value(Slider::Suppression);
                        let limit = if db >= Settings::UNLIMITED_ATTENUATION_DB {
                            l.text("Unlimited", "上限なし").to_owned()
                        } else {
                            format!("{db} dB")
                        };
                        card.child(
                            row()
                                .justify_between()
                                .text_sm()
                                .child(l.text("Suppression limit", "抑制の上限"))
                                .child(
                                    row()
                                        .child(
                                            div()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .child(limit),
                                        )
                                        .child(self.button(
                                            "auto-adjust",
                                            l.text("Auto adjust", "自動調節"),
                                            false,
                                            Control::AutoAdjust,
                                            cx,
                                        )),
                                ),
                        )
                        .child(self.slider(Slider::Suppression, cx))
                    })
                    .child(
                        row()
                            .justify_between()
                            .border_t_1()
                            .border_color(rgb(LINE))
                            .pt_2()
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

#[cfg(test)]
mod tests {
    use super::*;
    use orvia_core::{SignalHistory, spectrum::Spectrum};

    fn live() -> EngineStatus {
        EngineStatus {
            signal: SignalHistory {
                peaks: vec![[28, 22]; HISTORY_BLOCKS],
                attenuation_db: Some(6.),
            },
            spectrum: None,
            monitoring: false,
            monitor_error: None,
            audition: None,
            observed: std::time::SystemTime::UNIX_EPOCH,
            version: env!("CARGO_PKG_VERSION").into(),
            message: "Streaming".into(),
            peak: 0.02,
            missing_frames: 0,
            dropped_frames: 0,
        }
    }

    #[test]
    fn steady_waveform_repaints_at_the_first_visible_change() {
        let mut previous = live();
        for tick in 1..=80 {
            let mut next = previous.clone();
            let jitter = (tick as f32 * 0.1).sin() * 0.0001;
            next.peak = 0.02 * (1. + jitter);
            next.signal.attenuation_db = Some(6. + jitter);
            assert!(!Panel::mixer_status_changed(&previous, &next, false, true));
            previous = next;
        }
        let mut next = previous.clone();
        next.peak = 0.021;
        assert!(Panel::mixer_status_changed(&previous, &next, false, true));
        next.peak = previous.peak;
        next.signal.peaks[125][0] += 1;
        assert!(Panel::mixer_status_changed(&previous, &next, false, false));
    }

    #[test]
    fn bypass_ignores_only_the_filtered_trace_and_attenuation() {
        let old = live();
        let mut new = old.clone();
        new.signal.peaks[99][1] += 1;
        assert!(!Panel::mixer_status_changed(&old, &new, false, false));
        assert!(Panel::mixer_status_changed(&old, &new, false, true));
        new.signal.peaks.clone_from(&old.signal.peaks);
        new.signal.attenuation_db = None;
        assert!(!Panel::mixer_status_changed(&old, &new, false, false));
        assert!(Panel::mixer_status_changed(&old, &new, false, true));
    }

    #[test]
    fn meter_labels_follow_display_rounding_and_the_output_floor() {
        let mut old = live();
        let mut new = old.clone();
        for (a, b, changes) in [
            (0.25, 0.24, false),
            (0.25, 0.26, true),
            (-0.01, -0., false),
            (-0., 0., true),
        ] {
            old.signal.attenuation_db = Some(a);
            new.signal.attenuation_db = Some(b);
            assert_eq!(
                Panel::mixer_status_changed(&old, &new, false, true),
                changes
            );
        }
        old.peak = 0.;
        new.peak = 0.00001;
        assert!(!Panel::mixer_status_changed(&old, &new, false, false));
        new.peak = 0.000011;
        assert!(Panel::mixer_status_changed(&old, &new, false, false));
    }

    #[test]
    fn first_peak_clears_the_legacy_restart_notice_before_a_trace_is_drawn() {
        let mut old = live();
        old.signal.peaks.clear();
        let mut new = old.clone();
        new.signal.peaks.push([1, 2]);
        assert!(!Panel::mixer_status_changed(&old, &new, false, true));
        old.version = "0.0.1".into();
        new.version.clone_from(&old.version);
        assert!(Panel::mixer_status_changed(&old, &new, false, true));
    }

    #[test]
    fn spectrum_and_offline_mixer_ignore_unused_waveform_data() {
        let mut old = live();
        let mut new = old.clone();
        new.peak = 1.;
        new.signal.peaks.clear();
        assert!(!Panel::mixer_status_changed(&old, &new, true, true));
        new.spectrum = Some(Spectrum {
            bands: vec![0; Spectrum::BANDS],
            peak_hz: None,
        });
        assert!(Panel::mixer_status_changed(&old, &new, true, true));
        new.message = "Device disconnected".into();
        assert!(Panel::mixer_status_changed(&old, &new, false, true));
        old.message.clone_from(&new.message);
        assert!(!Panel::mixer_status_changed(&old, &new, false, true));
        assert!(!Panel::mixer_status_changed(&old, &new, true, true));
    }
}
