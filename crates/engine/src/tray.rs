//! Notification-area commands owned by the resident engine.
use anyhow::Result;
use orvia_core::{Language, Settings};
use std::{
    path::Path,
    process::Command,
    sync::mpsc::{self, Receiver},
};
use tray_item::{IconSource, TrayItem};

pub(super) enum Action {
    Show,
    Mute,
    Quit,
}

pub(super) struct Tray {
    icon: TrayItem,
    ids: [u32; 3],
    labels: Option<(Language, bool)>,
    events: Receiver<Action>,
}

impl Tray {
    pub(super) fn new() -> Result<Self> {
        let (send, events) = mpsc::channel();
        let mut icon = TrayItem::new("Orvia", IconSource::Resource("ORVIA_ICON"))?;
        let show = send.clone();
        let mute = send.clone();
        let menu = icon.inner_mut();
        let ids = [
            menu.add_menu_item_with_id("Open Orvia", move || {
                let _ = show.send(Action::Show);
            })?,
            menu.add_menu_item_with_id("Mute output", move || {
                let _ = mute.send(Action::Mute);
            })?,
            menu.add_menu_item_with_id("Quit Orvia", move || {
                let _ = send.send(Action::Quit);
            })?,
        ];
        Ok(Self {
            icon,
            ids,
            labels: None,
            events,
        })
    }

    pub(super) fn refresh(&mut self, s: &Settings) {
        let state = (s.language, s.muted);
        if self.labels == Some(state) {
            return;
        }
        let l = s.language;
        let labels = [
            l.text("Open Orvia", "Orvia を開く"),
            if s.muted {
                l.text("Unmute output", "出力のミュートを解除")
            } else {
                l.text("Mute output", "出力をミュート")
            },
            l.text("Quit Orvia", "Orvia を終了"),
        ];
        for (id, text) in self.ids.into_iter().zip(labels) {
            if self.icon.inner_mut().set_menu_item_label(text, id).is_err() {
                return;
            }
        }
        let tooltip = if s.muted {
            l.text("Orvia · Muted", "Orvia · ミュート中")
        } else {
            "Orvia"
        };
        let _ = self.icon.inner_mut().set_tooltip(tooltip);
        self.labels = Some(state);
    }

    pub(super) fn handle(&self, dir: &Path) -> Result<bool> {
        for action in self.events.try_iter() {
            match action {
                Action::Show => {
                    Command::new(std::env::current_exe()?.with_file_name("orvia.exe")).spawn()?;
                }
                Action::Mute => {
                    Settings::update(&dir.join("settings.json"), |s| s.muted = !s.muted)?;
                }
                Action::Quit => {
                    std::fs::write(dir.join("close-window"), b"1")?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
