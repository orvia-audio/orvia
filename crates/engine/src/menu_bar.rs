//! Keep AppKit's status item on the main thread while the audio supervisor runs.
use anyhow::Result;
use orvia_core::Language;
use std::{path::Path, process::Command, sync::mpsc, thread};
use tao::{
    event::{Event, StartCause},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::{
        macos::{ActivationPolicy, EventLoopExtMacOS},
        run_return::EventLoopExtRunReturn,
    },
};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

enum Message {
    Language(Language),
    Action(MenuEvent),
    Stopped,
}

pub(super) fn run(dir: &Path) -> Result<()> {
    let mut events = EventLoopBuilder::<Message>::with_user_event().build();
    events.set_activation_policy(ActivationPolicy::Accessory);
    events.set_activate_ignoring_other_apps(false);
    let menu = Menu::new();
    let show = MenuItem::new("Open Orvia", true, None);
    let quit = MenuItem::new("Quit Orvia", true, None);
    menu.append_items(&[&show, &quit])?;
    let icon = Icon::from_rgba(
        include_bytes!(concat!(env!("OUT_DIR"), "/orvia.rgba")).to_vec(),
        64,
        64,
    )?;
    let mut builder = Some(
        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon),
    );
    let mut item = None;
    let actions = events.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = actions.send_event(Message::Action(event));
    }));
    let updates = events.create_proxy();
    let (stop, stopping) = mpsc::channel();
    thread::scope(|threads| {
        let engine = threads.spawn(move || {
            let mut last = None;
            let result = super::supervise(dir, |language| {
                if language != last {
                    if let Some(l) = language {
                        let _ = updates.send_event(Message::Language(l));
                    }
                    last = language;
                }
                stopping.try_recv().is_ok()
            });
            let _ = updates.send_event(Message::Stopped);
            result
        });
        events.run_return(|event, _, flow| {
            *flow = ControlFlow::Wait;
            match event {
                Event::NewEvents(StartCause::Init) => {
                    item = builder.take().and_then(|b| b.build().ok());
                }
                Event::UserEvent(Message::Language(l)) => {
                    show.set_text(l.text("Open Orvia", "Orvia を開く"));
                    quit.set_text(l.text("Quit Orvia", "Orvia を終了"));
                }
                Event::UserEvent(Message::Action(ev)) if ev.id == *show.id() => {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = Command::new(exe.with_file_name("orvia")).spawn();
                    }
                }
                Event::UserEvent(Message::Action(ev)) if ev.id == *quit.id() => {
                    let _ = std::fs::write(dir.join("close-window"), b"1");
                    let _ = stop.send(());
                }
                Event::UserEvent(Message::Stopped) => *flow = ControlFlow::Exit,
                _ => {}
            }
        });
        drop(item);
        let _ = stop.send(());
        engine
            .join()
            .map_err(|_| anyhow::anyhow!("Audio supervisor terminated"))?
    })
}
