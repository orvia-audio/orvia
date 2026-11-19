//! Device selectors share a window-level popup so opening a list never reflows a card.
use super::*;

impl Panel {
    fn devices_for(&self, side: Endpoint) -> (&[(String, String)], Option<&str>) {
        match side {
            Endpoint::Input => (&self.inputs, self.prefs.input.as_deref()),
            Endpoint::Output => (&self.outputs, self.prefs.output.as_deref()),
            Endpoint::Monitor => (&self.monitors, self.prefs.monitor.as_deref()),
            Endpoint::TestSource => (&self.test.sources, self.test.source.as_deref()),
            Endpoint::TestSpeaker => (&self.monitors, self.test.speaker.as_deref()),
        }
    }

    fn choices(&self, side: Endpoint) -> impl Iterator<Item = (Option<&str>, &str)> {
        let l = self.prefs.language;
        let default = match side {
            Endpoint::Input => Some((
                None,
                l.text("System default microphone", "システムの既定のマイク"),
            )),
            Endpoint::TestSpeaker => Some((
                None,
                l.text("System default output", "システムの既定の出力"),
            )),
            _ => None,
        };
        default.into_iter().chain(
            self.devices_for(side)
                .0
                .iter()
                .map(|(id, label)| (Some(id.as_str()), label.as_str())),
        )
    }

    pub(crate) fn reset_picker_cursor(&mut self, side: Endpoint) {
        let selected = self.devices_for(side).1;
        let cursor = self
            .choices(side)
            .position(|(id, _)| id == selected)
            .unwrap_or(0);
        self.picker_cursor = cursor;
        self.picker_scroll = gpui::ScrollHandle::new();
        self.picker_scroll.scroll_to_item(self.picker_cursor);
    }

    pub(super) fn confirm_picker(
        &mut self,
        side: Endpoint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choice = self
            .choices(side)
            .nth(self.picker_cursor)
            .map(|(id, _)| id.map(str::to_owned));
        if let Some(id) = choice {
            self.activate(Control::Select(side, id), cx);
        }
        window.focus(&self.picker_anchors[side as usize].focus);
    }

    pub(super) fn picker_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let Some(side) = self.choosing else {
            if matches!(key, "down" | "up")
                && let Some(side) = [
                    Endpoint::Input,
                    Endpoint::Output,
                    Endpoint::Monitor,
                    Endpoint::TestSource,
                    Endpoint::TestSpeaker,
                ]
                .into_iter()
                .find(|s| self.picker_anchors[*s as usize].focus.is_focused(window))
            {
                self.activate(Control::Choose(side), cx);
                cx.stop_propagation();
            }
            return;
        };
        let last = self.choices(side).count().saturating_sub(1);
        match key {
            "escape" => {
                self.choosing = None;
                window.focus(&self.picker_anchors[side as usize].focus);
            }
            "down" => self.picker_cursor = (self.picker_cursor + 1).min(last),
            "up" => self.picker_cursor = self.picker_cursor.saturating_sub(1),
            "home" => self.picker_cursor = 0,
            "end" => self.picker_cursor = last,
            _ => return,
        }
        self.picker_scroll.scroll_to_item(self.picker_cursor);
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn picker(&self, side: Endpoint, cx: &mut Context<Self>) -> impl IntoElement {
        let id = match side {
            Endpoint::Input => "input",
            Endpoint::Output => "output",
            Endpoint::Monitor => "headphones",
            Endpoint::TestSource => "test-source",
            Endpoint::TestSpeaker => "test-speaker",
        };
        let open = self.choosing == Some(side);
        let measured = self.picker_anchors[side as usize].bounds.clone();
        div()
            .w_full()
            .child(self.button(id, self.device_name(side), open, Control::Choose(side), cx))
            .on_children_prepainted(move |children, w, _| {
                if let Some(&rect) = children.first()
                    && measured.replace(rect) != rect
                    && open
                {
                    w.refresh();
                }
            })
    }

    pub(super) fn picker_menu(
        &self,
        side: Endpoint,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let anchor = self.picker_anchors[side as usize].bounds.get();
        let viewport = window.viewport_size();
        let gap = px(4.);
        let below = (viewport.height - anchor.bottom() - gap * 2.).max(px(0.));
        let above = (anchor.top() - gap * 2.).max(px(0.));
        let count = self.choices(side).count();
        let desired = px((count.max(1) as f32 * 36. + 10.).min(248.));
        let upwards = desired > below && above > below;
        let selected = self.devices_for(side).1;
        let mut menu = div()
            .id("device-options")
            .absolute()
            .occlude()
            .left(anchor.left())
            .w(anchor.size.width)
            .when(upwards, |d| d.bottom(viewport.height - anchor.top() + gap))
            .when(!upwards, |d| d.top(anchor.bottom() + gap))
            .max_h(if upwards { above } else { below }.min(px(248.)))
            .flex()
            .flex_col()
            .p_1()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(LINE))
            .rounded_md()
            .shadow_lg()
            .overflow_y_scroll()
            .track_scroll(&self.picker_scroll)
            .on_mouse_down_out(cx.listener(move |p, event: &gpui::MouseDownEvent, _, cx| {
                if !p.picker_anchors[side as usize]
                    .bounds
                    .get()
                    .contains(&event.position)
                {
                    p.choosing = None;
                    cx.notify();
                }
            }));
        for (index, (id, label)) in self.choices(side).enumerate() {
            let checked = id == selected;
            let id = id.map(str::to_owned);
            menu = menu.child(
                div()
                    .id(("device-option", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_shrink_0()
                    .min_h(px(32.))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded_sm()
                    .text_sm()
                    .cursor_pointer()
                    .when(index == self.picker_cursor, |d| d.bg(rgb(0xffe4d7)))
                    .hover(|d| d.bg(rgb(0xffddca)))
                    .child(div().min_w_0().flex_1().child(label.to_owned()))
                    .when(checked, |d| d.child(glyph("check.svg", 16., INK)))
                    .on_click(cx.listener(move |p, _, w, cx| {
                        p.activate(Control::Select(side, id.clone()), cx);
                        w.focus(&p.picker_anchors[side as usize].focus);
                    })),
            );
        }
        if count == 0 {
            let l = self.prefs.language;
            menu = menu.child(div().p_2().child(note(match side {
                Endpoint::Output => l.text(
                    "No supported virtual output available",
                    "利用可能な仮想出力がありません",
                ),
                _ => l.text("No device available", "利用可能なデバイスがありません"),
            })));
        }
        menu
    }
}
