//! Mixer slider gestures and their bounded writes to the engine's settings file.
use super::*;
use gpui::{MouseUpEvent, Pixels, Task};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Slider {
    Gain,
    Suppression,
    LowCut,
    Compression,
}

impl Slider {
    pub(super) fn max(self) -> u16 {
        match self {
            Self::Gain => 200,
            Self::LowCut => 150,
            Self::Compression => 3,
            Self::Suppression => Settings::UNLIMITED_ATTENUATION_DB,
        }
    }

    fn value(self, prefs: &Settings) -> u16 {
        match self {
            Self::Gain => prefs.gain_percent,
            Self::LowCut => prefs.low_cut_hz,
            Self::Compression => prefs.compressor_ratio.saturating_sub(1),
            Self::Suppression => prefs.deep_filter_limit_db,
        }
        .min(self.max())
    }

    pub(super) fn set(self, settings: &mut Settings, value: u16) {
        match self {
            Self::Gain => settings.gain_percent = value,
            Self::LowCut => settings.low_cut_hz = value,
            Self::Compression => settings.compressor_ratio = value + 1,
            Self::Suppression => settings.deep_filter_limit_db = value,
        }
    }
}

/// A pointer gesture has at most one unsaved value and one scheduled write.
pub(super) struct SliderDrag {
    slider: Slider,
    pending: Option<u16>,
    last_attempt: Option<Instant>,
    timer: Option<Task<()>>,
}

impl SliderDrag {
    const INTERVAL: Duration = Duration::from_millis(100);

    fn new(slider: Slider) -> Self {
        Self {
            slider,
            pending: None,
            last_attempt: None,
            timer: None,
        }
    }

    fn delay(&self, now: Instant) -> Duration {
        self.last_attempt.map_or(Duration::ZERO, |last| {
            Self::INTERVAL.saturating_sub(now.duration_since(last))
        })
    }

    fn save(&mut self, dir: &Path, now: Instant) -> Option<io::Result<Settings>> {
        let value = self.pending?;
        self.last_attempt = Some(now);
        let saved = Settings::update(&dir.join("settings.json"), |s| self.slider.set(s, value));
        if saved.is_ok() {
            self.pending = None;
        }
        Some(saved)
    }

    fn displayed(&self, slider: Slider, prefs: &Settings) -> u16 {
        if self.slider == slider
            && let Some(value) = self.pending
        {
            value
        } else {
            slider.value(prefs)
        }
    }

    fn move_to(&mut self, value: u16, prefs: &Settings) -> bool {
        if value == self.displayed(self.slider, prefs) {
            return false;
        }
        self.pending = Some(value);
        true
    }
}

impl Panel {
    pub(super) fn slider_value(&self, slider: Slider) -> u16 {
        match &self.dragging_slider {
            Some(drag) => drag.displayed(slider, &self.prefs),
            None => slider.value(&self.prefs),
        }
    }

    pub(super) fn start_slider(&mut self, slider: Slider, x: Pixels, cx: &mut Context<Self>) {
        self.finish_slider();
        self.dragging_slider = Some(SliderDrag::new(slider));
        self.move_slider(x, cx);
    }

    /// Keep the draft separate from polled settings so another writer's edits stay visible.
    fn update_slider_position(&mut self, x: Pixels) -> Option<&mut SliderDrag> {
        let drag = self.dragging_slider.as_mut()?;
        let bounds = self.slider_anchors[drag.slider as usize].bounds.get();
        if bounds.size.width <= px(0.) {
            return None;
        }
        let ratio = ((x - bounds.origin.x) / bounds.size.width).clamp(0., 1.);
        let value = (ratio * f32::from(drag.slider.max())).round() as u16;
        drag.move_to(value, &self.prefs).then_some(drag)
    }

    pub(super) fn move_slider(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = self.update_slider_position(x) else {
            return;
        };
        let delay = drag.delay(Instant::now());
        if delay.is_zero() {
            self.flush_slider();
        } else if drag.timer.is_none() {
            // A single trailing write also updates the engine if the pointer stops while held.
            drag.timer = Some(cx.spawn(async move |panel, cx| {
                Timer::after(delay).await;
                let _ = panel.update(cx, |p, cx| {
                    p.flush_slider();
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn flush_slider(&mut self) -> bool {
        let Some(drag) = &mut self.dragging_slider else {
            return true;
        };
        drag.timer.take();
        let saved = drag.save(&self.dir, Instant::now());
        saved.is_none_or(|result| self.settings_saved(result))
    }

    /// Commit the final value and discard the gesture even if the disk write failed.
    pub(super) fn finish_slider(&mut self) {
        if !self.flush_slider() {
            eprintln!("{}", self.error);
        }
        self.dragging_slider = None;
    }

    pub(super) fn release_slider(
        &mut self,
        event: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dragging_slider.is_some() {
            self.update_slider_position(event.position.x);
            self.finish_slider();
            cx.notify();
        }
    }

    pub(super) fn observe_slider_focus(
        anchors: &[ControlAnchor; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (index, anchor) in anchors.iter().enumerate() {
            cx.on_blur(&anchor.focus, window, move |p, _, cx| {
                if p.dragging_slider
                    .as_ref()
                    .is_some_and(|d| d.slider as usize == index)
                {
                    p.finish_slider();
                    cx.notify();
                }
            })
            .detach();
        }
        cx.observe_window_activation(window, |p, window, cx| {
            if !window.is_window_active() && p.dragging_slider.is_some() {
                p.finish_slider();
                cx.notify();
            }
        })
        .detach();
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.finish_slider();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_drag_shows_each_position_and_saves_its_release_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let mut prefs = Settings::default();
        let start = Instant::now();
        let mut drag = SliderDrag::new(Slider::Gain);
        let mut saved_values = Vec::new();
        for event in 1..=120 {
            let now = start + Duration::from_micros(u64::from(event) * 1_000_000 / 120);
            assert!(drag.move_to(event, &prefs));
            assert!(!drag.move_to(event, &prefs));
            assert_eq!(drag.displayed(Slider::Gain, &prefs), event);
            if drag.delay(now).is_zero() {
                prefs = drag.save(dir.path(), now).unwrap().unwrap();
                saved_values.push(prefs.gain_percent);
            }
        }
        prefs = drag
            .save(dir.path(), start + Duration::from_secs(1))
            .unwrap()
            .unwrap();
        saved_values.push(prefs.gain_percent);
        assert_eq!(saved_values, [1, 13, 25, 37, 49, 61, 73, 85, 97, 109, 120]);
        assert!(
            drag.save(dir.path(), start + Duration::from_secs(1))
                .is_none()
        );
    }

    #[test]
    fn external_settings_remain_visible_and_survive_a_pending_slider_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let start = Instant::now();
        let mut drag = SliderDrag::new(Slider::Compression);
        assert!(drag.move_to(1, &Settings::default()));
        let prefs = drag.save(dir.path(), start).unwrap().unwrap();
        assert_eq!(prefs.compressor_ratio, 2);
        assert!(drag.move_to(2, &prefs));

        let external = Settings::update(&path, |s| {
            s.gain_percent = 35;
            s.compressor_ratio = 1;
            s.monitor_enabled = true;
        })
        .unwrap();
        assert_eq!(drag.displayed(Slider::Compression, &external), 2);
        assert_eq!(drag.displayed(Slider::Gain, &external), 35);
        let saved = drag
            .save(dir.path(), start + Duration::from_millis(100))
            .unwrap()
            .unwrap();
        assert_eq!(
            saved,
            Settings {
                compressor_ratio: 3,
                ..external
            }
        );
    }

    #[test]
    fn failed_intermediate_write_keeps_the_draft_for_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"invalid").unwrap();
        let mut drag = SliderDrag::new(Slider::LowCut);
        let start = Instant::now();
        assert!(drag.move_to(40, &Settings::default()));
        assert!(drag.save(dir.path(), start).unwrap().is_err());
        assert_eq!(drag.delay(start), SliderDrag::INTERVAL);
        assert_eq!(drag.displayed(Slider::LowCut, &Settings::default()), 40);
        fs::remove_file(&path).unwrap();
        let saved = drag.save(dir.path(), start).unwrap().unwrap();
        assert_eq!(saved.low_cut_hz, 40);
    }
}
