//! Log-frequency bars and band inspection for the processed output.
use super::*;
use gpui::{MouseButton, canvas, fill, point};
use orvia_core::spectrum::Spectrum;

impl Panel {
    pub(super) fn spectrum_chart(
        &self,
        card: gpui::Div,
        data: Option<&Spectrum>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let l = self.prefs.language;
        let selected = self.spectrum_band;
        let reading = match selected.and_then(|i| Some((i, *data?.bands.get(i)?))) {
            Some((i, level)) => {
                let hz = Spectrum::frequency_at((i as f32 + 0.5) / Spectrum::BANDS as f32);
                let db = i16::from(level) - i16::from(Spectrum::RANGE_DB);
                format!(
                    "{hz:.0} Hz  ·  {}{db} dBFS",
                    if level == 0 { "≤ " } else { "" }
                )
            }
            None if data.is_none()
                && self
                    .status
                    .as_ref()
                    .is_some_and(|s| s.message.starts_with("Streaming")) =>
            {
                l.text("Waiting for spectrum", "スペクトル待機中").into()
            }
            None => {
                let peak = data
                    .and_then(|s| s.peak_hz)
                    .map_or_else(|| "—".into(), |hz| format!("{hz} Hz"));
                format!("{}  {peak}", l.text("Peak", "ピーク"))
            }
        };
        let bands = data.map(|s| s.bands.clone()).unwrap_or_default();
        let measured = self.spectrum_anchor.bounds.clone();
        let bars = canvas(
            move |bounds, _, _| measured.set(bounds),
            move |bounds, _, window, _| {
                let w = bounds.size.width;
                let h = bounds.size.height;
                let start = bounds.origin;
                for fraction in [0., 0.5, 1.] {
                    let y = start.y + (h - px(1.)) * fraction;
                    window.paint_quad(fill(
                        Bounds::new(point(start.x, y), size(w, px(1.))),
                        rgb(0x495253),
                    ));
                }
                for step in 0..=3 {
                    let x = start.x + (w - px(1.)) * (step as f32 / 3.);
                    window.paint_quad(fill(
                        Bounds::new(point(x, start.y), size(px(1.), h)),
                        rgb(0x495253),
                    ));
                }
                let stride = w / Spectrum::BANDS as f32;
                if let Some(i) = selected {
                    window.paint_quad(fill(
                        Bounds::new(point(start.x + stride * i as f32, start.y), size(stride, h)),
                        rgb(0x535f60),
                    ));
                }
                for (i, level) in bands.iter().take(Spectrum::BANDS).enumerate() {
                    let height =
                        h * (f32::from(*level) / f32::from(Spectrum::RANGE_DB)).clamp(0., 1.);
                    if height > px(0.) {
                        let bar = Bounds::new(
                            point(start.x + stride * i as f32, start.y + h - height),
                            size((stride - px(1.)).max(px(1.)), height),
                        );
                        window.paint_quad(fill(
                            bar,
                            rgb(if Some(i) == selected { 0xffffff } else { CORAL }),
                        ));
                    }
                }
            },
        )
        .w_full()
        .h(px(62.));
        let chart = div()
            .id("spectrum-chart")
            .track_focus(&self.spectrum_anchor.focus)
            .border_1()
            .border_color(rgb(INK))
            .focus(|s| s.border_color(rgb(CORAL)))
            .on_mouse_move(cx.listener(|p, event: &gpui::MouseMoveEvent, _, cx| {
                let bounds = p.spectrum_anchor.bounds.get();
                if bounds.size.width > px(0.) {
                    let fraction =
                        ((event.position.x - bounds.origin.x) / bounds.size.width).clamp(0., 1.);
                    let band =
                        ((fraction * Spectrum::BANDS as f32) as usize).min(Spectrum::BANDS - 1);
                    if p.spectrum_band != Some(band) {
                        p.spectrum_band = Some(band);
                        cx.notify();
                    }
                }
            }))
            .on_hover(cx.listener(|p, hovered: &bool, _, cx| {
                if !hovered && p.spectrum_band.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|p, _, w, _| w.focus(&p.spectrum_anchor.focus)),
            )
            .on_key_down(cx.listener(|p, event: &gpui::KeyDownEvent, _, cx| {
                let current = p.spectrum_band.unwrap_or(Spectrum::BANDS / 2);
                p.spectrum_band = match event.keystroke.key.as_str() {
                    "left" => Some(current.saturating_sub(1)),
                    "right" => Some((current + 1).min(Spectrum::BANDS - 1)),
                    "home" => Some(0),
                    "end" => Some(Spectrum::BANDS - 1),
                    "escape" => None,
                    _ => return,
                };
                cx.stop_propagation();
                cx.notify();
            }))
            .child(bars);
        card.child(
            row()
                .justify_between()
                .text_xs()
                .child(
                    row()
                        .child(div().size(px(8.)).rounded_full().bg(rgb(CORAL)))
                        .child(l.text("Output mix", "ミックス出力"))
                        .child(div().text_color(rgb(0xc6cfcd)).child("−80 … 0 dBFS")),
                )
                .child(reading),
        )
        .child(chart)
        .child(
            div()
                .flex()
                .justify_between()
                .text_xs()
                .text_color(rgb(0xd5dcd9))
                .children(["20 Hz", "200 Hz", "2 kHz", "20 kHz"].map(|label| div().child(label))),
        )
    }
}
