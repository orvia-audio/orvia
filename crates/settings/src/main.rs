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
    Receiver,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Mixer,
    MicTest,
    Devices,
    Preferences,
}

#[derive(Clone)]
enum Control {
    Page(Page),
    ShowSpectrum(bool),
    Language(Language),
    Monitor,
    Choose(Endpoint),
    Select(Endpoint, Option<String>),
    Mix(Mixing),
    Noise(Option<NoiseMethod>),
    Test(TestAction),
    AutoAdjust,
    ApplyAdjustment,
    Tone(tone_setup::Action),
    RestartEngine,
    OpenLog,
    DismissError,
    DismissUpdate,
    Slide(Slider, u16),
    AutomaticUpdates,
    CheckUpdate,
    Releases,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slider {
    Gain,
    Suppression,
    LowCut,
    Compression,
}

/// Geometry and keyboard focus of a measured control in the scrolling page.
struct ControlAnchor {
    bounds: std::rc::Rc<std::cell::Cell<Bounds<gpui::Pixels>>>,
    focus: FocusHandle,
}

impl ControlAnchor {
    fn new(cx: &Context<Panel>) -> Self {
        Self {
            bounds: Default::default(),
            focus: cx.focus_handle().tab_stop(true),
        }
    }
}

struct Panel {
    show_spectrum: bool,
    spectrum_anchor: ControlAnchor,
    spectrum_band: Option<usize>,
    picker_anchors: [ControlAnchor; 6],
    picker_cursor: usize,
    picker_scroll: gpui::ScrollHandle,
    dir: PathBuf,
    root: FocusHandle,
    slider_anchors: [ControlAnchor; 4],
    dragging_slider: Option<Slider>,
    prefs: Settings,
    inputs: Vec<(String, String)>,
    outputs: Vec<(String, String)>,
    monitors: Vec<(String, String)>,
    receivers: Vec<(String, String)>,
    test: microphone_test::MicTest,
    tone_setup: Option<tone_setup::Setup>,
    #[cfg(test)]
    tone_snapshot: Option<tone_setup::Screen<'static>>,
    test_primary_focus: FocusHandle,
    page: Page,
    logo: std::sync::Arc<gpui::Image>,
    choosing: Option<Endpoint>,
    status: Option<EngineStatus>,
    error: String,
    updates: updates::Updates,
    show_update_feedback: bool,
    installed: Version,
    restarting: bool,
    test_only: bool,
    #[cfg(test)]
    adjustment_snapshot: Option<view::AdjustmentScreen<'static>>,
}

impl Panel {
    fn new(
        dir: PathBuf,
        installed: Version,
        test_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
            let mut last_poll = heartbeat;
            loop {
                // A newly started recording or live graph picks up on the next 250 ms tick.
                // Static pages only need an occasional settings and engine liveness check.
                if last_poll.elapsed() < Duration::from_secs(1) {
                    let Ok(live) = this.read_with(cx, |p, _| {
                        p.polling_audio()
                            || (p.page == Page::Mixer
                                && p.status
                                    .as_ref()
                                    .is_some_and(|s| s.message.starts_with("Streaming")))
                    }) else {
                        break;
                    };
                    if !live {
                        Timer::after(Duration::from_millis(250)).await;
                        continue;
                    }
                }
                last_poll = Instant::now();
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
                let refresh = heartbeat.elapsed() >= Duration::from_secs(2);
                if refresh {
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
                        if refresh
                            && p.page == Page::Mixer
                            && p.tone_setup.is_none()
                            && p.show_spectrum
                        {
                            let _ =
                                fs::write(poll_dir.join(orvia_core::spectrum::OBSERVER_FILE), b"1");
                        }
                        let retain = status.is_none()
                            && p.status.as_ref().is_some_and(|s| {
                                s.observed
                                    .elapsed()
                                    .is_ok_and(|age| age < Duration::from_secs(2))
                            });
                        let mut redraw = false;
                        if !retain {
                            redraw = p.status_changed(status.as_ref());
                            p.status = status;
                        }
                        if let Some(prefs) = prefs
                            && p.prefs != prefs
                        {
                            p.prefs = prefs;
                            redraw = true;
                        }
                        redraw |= p.polling_audio();
                        p.poll_tone_setup(cx);
                        if p.page == Page::MicTest {
                            p.poll_mic_test(cx);
                        }
                        if !p.test_only && p.prefs.check_updates_automatically && p.updates.due() {
                            p.check_for_updates(cx);
                            redraw = true;
                        }
                        if redraw {
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
        let mut panel = Self::empty(dir, installed, test_only, cx);
        window.focus(&panel.root);
        panel.updates = updates::Updates::load(&panel.dir);
        panel.prefs = prefs;
        panel.error = error;
        if test_only {
            panel.page = Page::MicTest;
            panel.open_mic_test();
        } else {
            panel.monitors = orvia_engine::monitors().unwrap_or_default();
        }
        panel
    }

    fn polling_audio(&self) -> bool {
        self.tone_setup.is_some() || self.test.busy() || self.test.adjust.is_some()
    }

    /// Compare the diagnostics drawn on this page. Heartbeats update liveness without
    /// repainting a static page or redrawing an unchanged signal.
    fn status_changed(&self, next: Option<&EngineStatus>) -> bool {
        let (Some(old), Some(new)) = (&self.status, next) else {
            return self.status.is_some() != next.is_some();
        };
        match self.page {
            Page::Devices => false,
            Page::Preferences => old.version != new.version,
            Page::MicTest => old.message != new.message || old.version != new.version,
            Page::Mixer => {
                old.message != new.message
                    || old.version != new.version
                    || old.monitor_error != new.monitor_error
                    || old.monitoring != new.monitoring
                    || if self.show_spectrum {
                        old.spectrum != new.spectrum
                    } else {
                        old.signal != new.signal || old.peak != new.peak
                    }
            }
        }
    }

    /// Construct the window state before attaching audio devices and background tasks.
    fn empty(dir: PathBuf, installed: Version, test_only: bool, cx: &Context<Self>) -> Self {
        Self {
            updates: Default::default(),
            show_spectrum: false,
            spectrum_anchor: ControlAnchor::new(cx),
            spectrum_band: None,
            show_update_feedback: false,
            dir,
            root: cx.focus_handle(),
            slider_anchors: std::array::from_fn(|_| ControlAnchor::new(cx)),
            picker_anchors: std::array::from_fn(|_| ControlAnchor::new(cx)),
            picker_cursor: 0,
            picker_scroll: gpui::ScrollHandle::new(),
            dragging_slider: None,
            prefs: Settings::default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            receivers: Vec::new(),
            monitors: Vec::new(),
            test: Default::default(),
            tone_setup: None,
            #[cfg(test)]
            tone_snapshot: None,
            test_primary_focus: cx.focus_handle().tab_stop(true),
            page: Page::Mixer,
            logo: std::sync::Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                include_bytes!(concat!(env!("OUT_DIR"), "/orvia.png")).to_vec(),
            )),
            choosing: None,
            status: None,
            error: String::new(),
            installed,
            restarting: false,
            test_only,
            #[cfg(test)]
            adjustment_snapshot: None,
        }
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
            Control::ShowSpectrum(on) => {
                self.show_spectrum = on;
                self.spectrum_band = None;
                if on {
                    let _ = fs::write(self.dir.join(orvia_core::spectrum::OBSERVER_FILE), b"1");
                }
            }
            Control::Page(page) => {
                self.tone_setup = None;
                if self.page != page || self.test.adjust.is_some() {
                    if self.page == Page::MicTest {
                        self.finish_test_recording();
                        self.test = Default::default();
                    }
                    if page == Page::MicTest {
                        self.open_mic_test();
                    } else if page == Page::Devices {
                        self.inputs = orvia_engine::inputs().unwrap_or_default();
                        self.outputs = orvia_engine::outputs().unwrap_or_default();
                        self.receivers = orvia_engine::mic_test::sources().unwrap_or_default();
                    }
                }
                self.show_update_feedback = false;
                self.page = page;
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
                        Endpoint::Receiver => {
                            (&mut self.receivers, orvia_engine::mic_test::sources())
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
            Control::Tone(action) => self.tone_action(action, cx),
            Control::AutoAdjust => self.start_adjustment(),
            Control::ApplyAdjustment => self.apply_adjustment(cx),
            Control::Test(action) => self.run_test_action(action),
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
            Control::DismissUpdate => self.show_update_feedback = false,
            Control::CheckUpdate if !self.updates.checking => {
                self.show_update_feedback = true;
                self.check_for_updates(cx);
            }
            Control::CheckUpdate => {}
            Control::OpenLog => {
                let path = self.dir.join(orvia_core::ENGINE_LOG_TEXT);
                match fs::File::open(&path) {
                    Ok(_) => {
                        self.error.clear();
                        cx.open_with_system(&path);
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        self.error = self
                            .prefs
                            .language
                            .text("No engine log yet", "エンジンログはまだありません")
                            .into();
                    }
                    Err(e) => {
                        self.error = format!(
                            "{} · {e}",
                            self.prefs
                                .language
                                .text("Cannot open the log file", "ログファイルを開けません")
                        );
                    }
                }
            }
            Control::Releases => cx.open_url(RELEASES),
            change => {
                let path = self.dir.join("settings.json");
                let result = Settings::update(&path, |settings| match change {
                    Control::Select(side, id) => {
                        match side {
                            Endpoint::Input => settings.input = id,
                            Endpoint::Output => {
                                if settings.output != id {
                                    settings.receiver = None;
                                }
                                settings.output = id;
                            }
                            Endpoint::Receiver => settings.receiver = id,
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
                    Control::Slide(Slider::Gain, percent) => settings.gain_percent = percent,
                    Control::Slide(Slider::LowCut, hz) => settings.low_cut_hz = hz,
                    Control::Slide(Slider::Compression, step) => {
                        settings.compressor_ratio = step + 1
                    }
                    Control::Slide(Slider::Suppression, db) => settings.deep_filter_limit_db = db,
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

mod auto_adjust;
mod engine_control;
mod icons;
mod microphone_test;
mod reading;
mod tone_setup;
mod view;

mod updates;

#[cfg(test)]
mod snapshots;

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
        if let Err(error) = opened {
            eprintln!("Could not open the Orvia window: {error:#}");
            cx.quit();
        } else {
            cx.activate(true);
        }
    });
}
