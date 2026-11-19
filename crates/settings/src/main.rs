//! GPUI controls for one user's resident Orvia engine.
#![cfg_attr(windows, windows_subsystem = "windows")]

use gpui::{
    Application, Bounds, Context, FocusHandle, KeyBinding, SharedString, Timer, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use orvia_core::{EngineStatus, Language, Mixing, NoiseMethod, Settings, data_dir, read_json};
use serde::Deserialize;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

actions!(
    orvia,
    [
        /// Move keyboard focus to the next settings control.
        NextControl,
        /// Move keyboard focus to the preceding settings control.
        PreviousControl
    ]
);
const RELEASES: &str = "https://github.com/sya-ri/orvia/releases";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Input,
    Output,
    Monitor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Mixer,
    Devices,
    Preferences,
}

#[derive(Clone)]
enum Control {
    Page(Page),
    Language(Language),
    Monitor,
    Choose(Endpoint),
    Select(Endpoint, Option<String>),
    Mix(Mixing),
    Noise(Option<NoiseMethod>),
    Mute,
    Gain(u16),
    CheckUpdate,
    Releases,
}

/// Geometry and keyboard focus of a device selector in the scrolling page.
struct PickerAnchor {
    bounds: std::rc::Rc<std::cell::Cell<Bounds<gpui::Pixels>>>,
    focus: FocusHandle,
}

struct Panel {
    picker_anchors: [PickerAnchor; 3],
    picker_cursor: usize,
    picker_scroll: gpui::ScrollHandle,
    dir: PathBuf,
    root: FocusHandle,
    gain_focus: FocusHandle,
    gain_bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<gpui::Pixels>>>>,
    dragging_gain: bool,
    prefs: Settings,
    inputs: Vec<(String, String)>,
    outputs: Vec<(String, String)>,
    monitors: Vec<(String, String)>,
    page: Page,
    logo: std::sync::Arc<gpui::Image>,
    choosing: Option<Endpoint>,
    status: Option<EngineStatus>,
    error: String,
    update: String,
    checking: bool,
}

impl Panel {
    fn new(dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = cx.focus_handle();
        window.focus(&root);
        let mut error = String::new();
        let prefs = Settings::load(&dir.join("settings.json")).unwrap_or_else(|e| {
            error = format!(
                "{} · {e}",
                Language::System.text("Cannot load settings", "設定を読み込めません")
            );
            Settings {
                muted: true,
                ..Default::default()
            }
        });
        if let Err(e) = start_engine() {
            error = format!(
                "{} · {e}",
                prefs
                    .language
                    .text("Cannot start audio engine", "音声エンジンを起動できません")
            );
        }
        let poll_dir = dir.clone();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let mut heartbeat = Instant::now() - Duration::from_secs(3);
            loop {
                if fs::remove_file(poll_dir.join("close-window")).is_ok() {
                    let _ = handle.update(cx, |_, w, _| w.remove_window());
                    break;
                }
                if fs::remove_file(poll_dir.join("show-window")).is_ok() {
                    let _ = handle.update(cx, |_, w, app| {
                        app.activate(true);
                        w.activate_window();
                    });
                }
                if heartbeat.elapsed() >= Duration::from_secs(2) {
                    // An empty rewrite can leave the previous modification time on Windows.
                    let _ = fs::write(poll_dir.join("observe"), b"1");
                    heartbeat = Instant::now();
                }
                let status = read_json::<EngineStatus>(&poll_dir.join("status.json"))
                    .ok()
                    .filter(|s| {
                        s.observed
                            .elapsed()
                            .is_ok_and(|age| age < Duration::from_secs(2))
                    });
                let prefs = Settings::load(&poll_dir.join("settings.json")).ok();
                if this
                    .update(cx, |p, cx| {
                        p.status = status;
                        if let Some(prefs) = prefs {
                            p.prefs = prefs;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                Timer::after(Duration::from_millis(250)).await;
            }
        })
        .detach();
        let inputs = orvia_engine::inputs().unwrap_or_default();
        Self {
            dir,
            root,
            gain_focus: cx.focus_handle().tab_stop(true),
            picker_anchors: std::array::from_fn(|_| PickerAnchor {
                bounds: Default::default(),
                focus: cx.focus_handle().tab_stop(true),
            }),
            picker_cursor: 0,
            picker_scroll: gpui::ScrollHandle::new(),
            gain_bounds: Default::default(),
            dragging_gain: false,
            prefs,
            inputs,
            outputs: orvia_engine::outputs().unwrap_or_default(),
            monitors: orvia_engine::monitors().unwrap_or_default(),
            page: Page::Mixer,
            logo: std::sync::Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                include_bytes!(concat!(env!("OUT_DIR"), "/orvia.png")).to_vec(),
            )),
            choosing: None,
            status: None,
            error,
            update: String::new(),
            checking: false,
        }
    }

    fn activate(&mut self, control: Control, cx: &mut Context<Self>) {
        match control {
            Control::Page(page) => {
                self.page = page;
                self.choosing = None;
            }
            Control::Monitor if self.prefs.monitor.is_none() => {
                self.page = Page::Devices;
                self.activate(Control::Choose(Endpoint::Monitor), cx);
            }
            Control::Choose(side) => {
                self.choosing = (self.choosing != Some(side)).then_some(side);
                if self.choosing.is_some() {
                    let (list, devices) = match side {
                        Endpoint::Input => (&mut self.inputs, orvia_engine::inputs()),
                        Endpoint::Output => (&mut self.outputs, orvia_engine::outputs()),
                        Endpoint::Monitor => (&mut self.monitors, orvia_engine::monitors()),
                    };
                    match devices {
                        Ok(devices) => *list = devices,
                        Err(e) => self.error = e.to_string(),
                    }
                    self.reset_picker_cursor(side);
                }
            }
            Control::CheckUpdate if !self.checking => {
                self.checking = true;
                self.update = self
                    .prefs
                    .language
                    .text("Checking releases…", "更新を確認中…")
                    .into();
                let language = self.prefs.language;
                let task = cx
                    .background_executor()
                    .spawn(async move { latest_release(language) });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |p, cx| {
                        p.update = result;
                        p.checking = false;
                        cx.notify();
                    });
                })
                .detach();
            }
            Control::CheckUpdate => {}
            Control::Releases => cx.open_url(RELEASES),
            change => {
                let path = self.dir.join("settings.json");
                let result = Settings::update(&path, |settings| match change {
                    Control::Select(side, id) => {
                        match side {
                            Endpoint::Input => settings.input = id,
                            Endpoint::Output => settings.output = id,
                            Endpoint::Monitor => {
                                settings.monitor_enabled = id.is_some();
                                settings.monitor = id;
                            }
                        }
                        self.choosing = None;
                    }
                    Control::Mix(mode) => settings.mixing = mode,
                    Control::Language(l) => {
                        settings.language = l;
                        self.update.clear();
                    }
                    Control::Monitor => settings.monitor_enabled = !settings.monitor_enabled,
                    Control::Mute => settings.muted = !settings.muted,
                    Control::Noise(method) => {
                        settings.suppress_noise = method.is_some();
                        if let Some(method) = method {
                            settings.noise_method = method;
                        }
                    }
                    Control::Gain(percent) => settings.gain_percent = percent,
                    _ => unreachable!(),
                })
                .map(|saved| self.prefs = saved);
                self.error = result
                    .err()
                    .map(|e| {
                        format!(
                            "{} · {e}",
                            self.prefs
                                .language
                                .text("Cannot save settings", "設定を保存できません")
                        )
                    })
                    .unwrap_or_default();
            }
        }
        cx.notify();
    }
}

mod icons;
mod view;

fn start_engine() -> io::Result<()> {
    let engine = std::env::current_exe()?.with_file_name("orvia-engine.exe");
    Command::new(engine).spawn().map(|_| ())
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

fn latest_release(l: Language) -> String {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent();
    let result = agent
        .get("https://api.github.com/repos/sya-ri/orvia/releases/latest")
        .header("User-Agent", "Orvia")
        .call();
    let mut response = match result {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => {
            return l
                .text("No public release available.", "公開リリースはありません。")
                .into();
        }
        Err(e) => {
            return format!(
                "{} · {e}",
                l.text("Update check failed", "更新を確認できません")
            );
        }
    };
    let Ok(release) = response.body_mut().read_json::<Release>() else {
        return l
            .text(
                "Unrecognized release information",
                "リリース情報を読み取れません",
            )
            .into();
    };
    let latest = semver::Version::parse(release.tag_name.trim_start_matches('v'));
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"));
    match (latest, current) {
        (Ok(v), Ok(installed)) if v > installed => {
            format!(
                "{} · {v}",
                l.text("Update available", "アップデートがあります")
            )
        }
        (Ok(_), Ok(_)) => l.text("Up to date", "最新バージョンです").into(),
        _ => l
            .text("Unrecognized release version", "バージョンを読み取れません")
            .into(),
    }
}

fn main() -> io::Result<()> {
    let dir = data_dir()?;
    fs::create_dir_all(&dir)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("window.lock"))?;
    if let Err(e) = fs2::FileExt::try_lock_exclusive(&lock) {
        if e.raw_os_error() == Some(33) || e.kind() == io::ErrorKind::WouldBlock {
            return fs::write(dir.join("show-window"), b"1");
        }
        return Err(e);
    }
    let _ = fs::remove_file(dir.join("close-window"));
    run(&dir);
    Ok(())
}

fn run(dir: &Path) {
    let dir = dir.to_owned();
    Application::new().with_assets(icons::Icons).run(move |cx| {
        cx.bind_keys([
            KeyBinding::new("tab", NextControl, Some("Settings")),
            KeyBinding::new("shift-tab", PreviousControl, Some("Settings")),
        ]);
        let bounds = Bounds::centered(None, size(px(830.), px(610.)), cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let opened = cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Orvia".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(590.), px(440.))),
                ..Default::default()
            },
            |w, cx| cx.new(|cx| Panel::new(dir, w, cx)),
        );
        if opened.is_err() {
            cx.quit();
        } else {
            cx.activate(true);
        }
    });
}
