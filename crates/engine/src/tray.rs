//! Notification-area commands owned by the resident engine.
use anyhow::Result;
use orvia_core::Language;
use std::{
    path::Path,
    process::Command,
    sync::mpsc::{self, Receiver},
};
use tray_item::{IconSource, TrayItem};

pub(super) enum Action {
    Show,
    Quit,
}

pub(super) struct Tray {
    icon: TrayItem,
    ids: [u32; 2],
    language: Option<Language>,
    events: Receiver<Action>,
}

impl Tray {
    pub(super) fn new() -> Result<Self> {
        let (send, events) = mpsc::channel();
        let mut icon = TrayItem::new("Orvia", IconSource::Resource("ORVIA_ICON"))?;
        let show = send.clone();
        let menu = icon.inner_mut();
        let ids = [
            menu.add_menu_item_with_id("Open Orvia", move || {
                let _ = show.send(Action::Show);
            })?,
            menu.add_menu_item_with_id("Quit Orvia", move || {
                let _ = send.send(Action::Quit);
            })?,
        ];
        Ok(Self {
            icon,
            ids,
            language: None,
            events,
        })
    }

    pub(super) fn refresh(&mut self, l: Language) {
        if self.language == Some(l) {
            return;
        }
        let labels = [
            l.text("Open Orvia", "Orvia を開く"),
            l.text("Quit Orvia", "Orvia を終了"),
        ];
        for (id, text) in self.ids.into_iter().zip(labels) {
            if self.icon.inner_mut().set_menu_item_label(text, id).is_err() {
                return;
            }
        }
        self.language = Some(l);
    }

    pub(super) fn handle(&self, dir: &Path) -> Result<bool> {
        for action in self.events.try_iter() {
            match action {
                Action::Show => {
                    Command::new(std::env::current_exe()?.with_file_name("orvia.exe")).spawn()?;
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
