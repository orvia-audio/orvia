//! Notification-area commands owned by the resident engine.
use anyhow::Result;
use orvia_core::Language;
use std::{
    path::Path,
    process::Command,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use tray_item::{IconSource, TrayItem};

pub(super) fn run(dir: &Path) -> Result<()> {
    let mut tray = Tray::new().ok();
    let mut attempted = Instant::now();
    super::supervise(dir, |language| {
        if tray.is_none() && attempted.elapsed() >= Duration::from_secs(10) {
            tray = Tray::new().ok();
            attempted = Instant::now();
        }
        if let Some(tray) = &mut tray {
            if let Some(language) = language {
                tray.refresh(language);
            }
            return matches!(tray.handle(dir), Ok(true));
        }
        false
    })
}

enum Action {
    Show,
    Quit,
}

struct Tray {
    icon: TrayItem,
    ids: [u32; 2],
    language: Option<Language>,
    events: Receiver<Action>,
}

impl Tray {
    fn new() -> Result<Self> {
        let (send, events) = mpsc::channel();
        #[cfg(windows)]
        let image = IconSource::Resource("ORVIA_ICON");
        #[cfg(target_os = "linux")]
        let image = IconSource::Data {
            width: 64,
            height: 64,
            data: include_bytes!(concat!(env!("OUT_DIR"), "/orvia.rgba"))
                .chunks_exact(4)
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect(),
        };
        let mut icon = TrayItem::new("Orvia", image)?;
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

    fn refresh(&mut self, l: Language) {
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

    fn handle(&self, dir: &Path) -> Result<bool> {
        for action in self.events.try_iter() {
            match action {
                Action::Show => {
                    Command::new(
                        std::env::current_exe()?
                            .with_file_name(format!("orvia{}", std::env::consts::EXE_SUFFIX)),
                    )
                    .spawn()?;
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
