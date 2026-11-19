//! GPUI controls for one user's resident Orvia engine.
#![cfg_attr(windows, windows_subsystem = "windows")]

use gpui::{
    Application, Bounds, Context, FocusHandle, KeyBinding, SharedString, Timer, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use microphone_test::TestAction;
use orvia_core::{EngineStatus, Language, Mixing, NoiseMethod, Settings, data_dir, read_json};
use semver::Version;
use std::{
    fs, io,
    path::{Path, PathBuf},
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
const RELEASES: &str = "https://github.com/sya-ri/orvia/releases/latest";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Input,
    Output,
    Monitor,
    TestSource,
    TestSpeaker,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Mixer,
    MicTest,
    Devices,
    Preferences,
    Log,
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
    Test(TestAction),
    TestTake(microphone_test::Take),
    RestartEngine,
    DismissError,
    Gain(u16),
    AutomaticUpdates,
    CheckUpdate,
    Releases,
}

/// Geometry and keyboard focus of a device selector in the scrolling page.
struct PickerAnchor {
    bounds: std::rc::Rc<std::cell::Cell<Bounds<gpui::Pixels>>>,
    focus: FocusHandle,
}

struct Panel {
    picker_anchors: [PickerAnchor; 5],
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
    test: microphone_test::MicTest,
    test_primary_focus: FocusHandle,
    page: Page,
    logo: std::sync::Arc<gpui::Image>,
    choosing: Option<Endpoint>,
    status: Option<EngineStatus>,
    log: engine_log::LogView,
    error: String,
    updates: updates::Updates,
    installed: Version,
    restarting: bool,
    test_only: bool,
}

impl Panel {
    fn new(
        dir: PathBuf,
        installed: Version,
        test_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let root = cx.focus_handle();
        window.focus(&root);
        let mut error = String::new();
        let prefs = Settings::load(&dir.join("settings.json")).unwrap_or_else(|e| {
            error = format!(
                "{} · {e}",
                Language::System.text("Cannot load settings", "設定を読み込めません")
            );
            Settings::default()
        });
        if !test_only && let Err(e) = engine_control::start() {
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
                if !test_only && fs::remove_file(poll_dir.join("close-window")).is_ok() {
                    let _ = handle.update(cx, |_, w, _| w.remove_window());
                    break;
                }
                if !test_only && fs::remove_file(poll_dir.join("show-window")).is_ok() {
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
                        p.status = status.or_else(|| {
                            p.status.take().filter(|s| {
                                s.observed
                                    .elapsed()
                                    .is_ok_and(|age| age < Duration::from_secs(2))
                            })
                        });
                        let language = p.prefs.language;
                        if let Some(prefs) = prefs {
                            p.prefs = prefs;
                        }
                        if p.page == Page::MicTest {
                            p.poll_mic_test();
                        }
                        if !p.test_only && p.prefs.check_updates_automatically && p.updates.due() {
                            p.check_for_updates(cx);
                        }
                        if p.page != Page::Log
                            || p.log.refresh(&poll_dir)
                            || p.prefs.language != language
                        {
                            cx.notify();
                        }
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
        let mut panel = Self {
            updates: updates::Updates::load(&dir),
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
            test: Default::default(),
            test_primary_focus: cx.focus_handle().tab_stop(true),
            page: Page::Mixer,
            logo: std::sync::Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                include_bytes!(concat!(env!("OUT_DIR"), "/orvia.png")).to_vec(),
            )),
            choosing: None,
            status: None,
            log: Default::default(),
            error,
            installed,
            restarting: false,
            test_only,
        };
        if test_only {
            panel.page = Page::MicTest;
            panel.open_mic_test();
        }
        panel
    }

    fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.updates.start();
        let dir = self.dir.clone();
        let task = cx
            .background_executor()
            .spawn(async move { updates::fetch(&dir) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |panel, cx| {
                panel.updates.finish(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn activate(&mut self, control: Control, cx: &mut Context<Self>) {
        match control {
            Control::Page(page) => {
                if self.page != page {
                    if self.page == Page::MicTest {
                        self.finish_test_recording();
                        self.test = Default::default();
                    }
                    if page == Page::MicTest {
                        self.open_mic_test();
                    }
                }
                self.page = page;
                if page == Page::Log {
                    self.log.refresh(&self.dir);
                }
                self.choosing = None;
            }
            Control::Monitor if self.prefs.monitor.is_none() => {
                self.activate(Control::Choose(Endpoint::Monitor), cx);
            }
            Control::Choose(side) => {
                if matches!(side, Endpoint::TestSource | Endpoint::TestSpeaker) && self.test.busy()
                {
                    return;
                }
                self.choosing = (self.choosing != Some(side)).then_some(side);
                if self.choosing.is_some() {
                    let (list, devices) = match side {
                        Endpoint::Input => (&mut self.inputs, orvia_engine::inputs()),
                        Endpoint::Output => (&mut self.outputs, orvia_engine::outputs()),
                        Endpoint::Monitor | Endpoint::TestSpeaker => {
                            (&mut self.monitors, orvia_engine::monitors())
                        }
                        Endpoint::TestSource => {
                            (&mut self.test.sources, orvia_engine::mic_test::sources())
                        }
                    };
                    match devices {
                        Ok(devices) => *list = devices,
                        Err(e) => self.error = e.to_string(),
                    }
                    self.reset_picker_cursor(side);
                }
            }
            Control::Select(Endpoint::TestSource, id) => {
                self.test.source = id;
                self.choosing = None;
            }
            Control::Select(Endpoint::TestSpeaker, id) => {
                self.test.speaker = id;
                self.choosing = None;
            }
            Control::Test(action) => self.run_test_action(action),
            Control::TestTake(take) => self.select_take(take),
            Control::RestartEngine if !self.restarting => {
                self.restarting = true;
                self.error.clear();
                let dir = self.dir.clone();
                let task = cx
                    .background_executor()
                    .spawn(async move { engine_control::restart(&dir) });
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    let _ = this.update(cx, |p, cx| {
                        p.restarting = false;
                        if let Err(e) = result {
                            p.error = format!(
                                "{} · {e}",
                                p.prefs.language.text("Restart failed", "再起動できません")
                            );
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Control::RestartEngine => {}
            Control::DismissError => self.error.clear(),
            Control::CheckUpdate if !self.updates.checking => self.check_for_updates(cx),
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
                            Endpoint::TestSource | Endpoint::TestSpeaker => unreachable!(),
                        }
                        self.choosing = None;
                    }
                    Control::Mix(mode) => settings.mixing = mode,
                    Control::Language(l) => {
                        settings.language = l;
                    }
                    Control::AutomaticUpdates => {
                        settings.check_updates_automatically = !settings.check_updates_automatically
                    }
                    Control::Monitor => settings.monitor_enabled = !settings.monitor_enabled,
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

mod engine_control;
mod engine_log;
mod icons;
mod microphone_test;
mod view;

mod updates;

fn main() -> io::Result<()> {
    let test_only = std::env::args_os().skip(1).any(|arg| arg == "--mic-test");
    let dir = data_dir()?;
    fs::create_dir_all(&dir)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(if test_only {
            "mic-test-window.lock"
        } else {
            "window.lock"
        }))?;
    if let Err(e) = fs2::FileExt::try_lock_exclusive(&lock) {
        if e.raw_os_error() == Some(33) || e.kind() == io::ErrorKind::WouldBlock {
            if test_only {
                return Ok(());
            }
            return fs::write(dir.join("show-window"), b"1");
        }
        return Err(e);
    }
    if !test_only {
        let _ = fs::remove_file(dir.join("close-window"));
    }
    let installed = Version::parse(env!("CARGO_PKG_VERSION")).map_err(io::Error::other)?;
    run(&dir, installed, test_only);
    Ok(())
}

fn run(dir: &Path, installed: Version, test_only: bool) {
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
                    title: Some(
                        if test_only {
                            "Orvia · Mic test"
                        } else {
                            "Orvia"
                        }
                        .into(),
                    ),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(590.), px(440.))),
                ..Default::default()
            },
            |w, cx| cx.new(|cx| Panel::new(dir, installed, test_only, w, cx)),
        );
        if opened.is_err() {
            cx.quit();
        } else {
            cx.activate(true);
        }
    });
}
