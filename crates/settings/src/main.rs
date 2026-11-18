//! GPUI controls for one user's resident Orvia engine.
#![cfg_attr(windows, windows_subsystem = "windows")]

use gpui::{
    Application, Bounds, Context, FocusHandle, KeyBinding, SharedString, Timer, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, relative, rgb, size,
};
use orvia_core::{EngineStatus, Mixing, Settings, data_dir, read_json, write_json};
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
        PreviousControl,
        /// Perform the focused control's primary action.
        Activate
    ]
);
const RELEASES: &str = "https://github.com/sya-ri/orvia/releases";

#[derive(Clone)]
enum Control {
    Inputs,
    Input(Option<String>),
    Mix(Mixing),
    Noise,
    Mute,
    Gain(i16),
    CheckUpdate,
    Releases,
}

struct Panel {
    dir: PathBuf,
    root: FocusHandle,
    prefs: Settings,
    inputs: Vec<(String, String)>,
    choosing_input: bool,
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
            error = format!("Cannot load settings: {e}");
            Settings {
                muted: true,
                ..Default::default()
            }
        });
        if let Err(e) = start_engine() {
            error = format!("Cannot start engine: {e}");
        }
        let poll_dir = dir.clone();
        cx.spawn(async move |this, cx| {
            let mut heartbeat = Instant::now() - Duration::from_secs(3);
            loop {
                if heartbeat.elapsed() >= Duration::from_secs(2) {
                    let _ = fs::write(poll_dir.join("observe"), []);
                    heartbeat = Instant::now();
                }
                let status = read_json::<EngineStatus>(&poll_dir.join("status.json"))
                    .ok()
                    .filter(|s| {
                        s.observed
                            .elapsed()
                            .is_ok_and(|age| age < Duration::from_secs(2))
                    });
                if this
                    .update(cx, |p, cx| {
                        p.status = status;
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
            prefs,
            inputs,
            choosing_input: false,
            status: None,
            error,
            update: String::new(),
            checking: false,
        }
    }

    fn activate(&mut self, control: Control, cx: &mut Context<Self>) {
        match control {
            Control::Inputs => {
                self.choosing_input = !self.choosing_input;
                if self.choosing_input {
                    match orvia_engine::inputs() {
                        Ok(inputs) => self.inputs = inputs,
                        Err(e) => self.error = e.to_string(),
                    }
                }
            }
            Control::CheckUpdate if !self.checking => {
                self.checking = true;
                self.update = "Checking releases…".into();
                let task = cx.background_executor().spawn(async { latest_release() });
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
                let result = Settings::load(&path).and_then(|mut settings| {
                    match change {
                        Control::Input(id) => {
                            settings.input = id;
                            self.choosing_input = false;
                        }
                        Control::Mix(mode) => settings.mixing = mode,
                        Control::Mute => settings.muted = !settings.muted,
                        Control::Noise => settings.suppress_noise = !settings.suppress_noise,
                        Control::Gain(delta) => {
                            settings.gain_percent =
                                settings.gain_percent.saturating_add_signed(delta).min(200)
                        }
                        _ => unreachable!(),
                    }
                    write_json(&path, &settings)?;
                    self.prefs = settings;
                    Ok(())
                });
                self.error = result
                    .err()
                    .map(|e| format!("Cannot save settings: {e}"))
                    .unwrap_or_default();
            }
        }
        cx.notify();
    }

    fn button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        selected: bool,
        control: Control,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let keyboard = control.clone();
        div()
            .id(id.into())
            .tab_index(0)
            .key_context("Control")
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .cursor_pointer()
            .border_color(rgb(if selected { 0x16745e } else { 0xb6c8c1 }))
            .bg(rgb(if selected { 0xd6f2e7 } else { 0xffffff }))
            .hover(|s| s.bg(rgb(0xe0eee7)))
            .focus(|s| s.border_2().border_color(rgb(0x215dba)))
            .on_action(cx.listener(move |p, _: &Activate, _, cx| p.activate(keyboard.clone(), cx)))
            .on_click(cx.listener(move |p, _, _, cx| p.activate(control.clone(), cx)))
            .child(label.into())
    }

    fn input_name(&self) -> String {
        self.prefs
            .input
            .as_ref()
            .map(|id| {
                self.inputs
                    .iter()
                    .find(|d| &d.0 == id)
                    .map(|d| d.1.clone())
                    .unwrap_or_else(|| "Disconnected microphone".into())
            })
            .unwrap_or_else(|| "Windows default microphone".into())
    }
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.as_ref();
        let peak = status.map_or(0., |s| s.peak.clamp(0., 1.));
        let db = if peak > 0.00001 {
            format!("{:.1} dBFS", 20. * peak.log10())
        } else {
            "Silence".into()
        };
        let row = || div().flex().flex_wrap().gap_2().items_center();
        let heading = |text| div().font_weight(gpui::FontWeight::SEMIBOLD).child(text);
        let mut view = div()
            .id("settings")
            .track_focus(&self.root)
            .key_context("Settings")
            .on_action(|_: &NextControl, w, _| w.focus_next())
            .on_action(|_: &PreviousControl, w, _| w.focus_prev())
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(0xf4f7f5))
            .text_color(rgb(0x182c27))
            .font_family("Segoe UI")
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_2xl().child("Microphone settings"))
            .child(
                div().child(
                    status
                        .map_or("Waiting for the audio engine", |s| &s.message)
                        .to_owned(),
                ),
            )
            .child(heading("Input"))
            .child(self.button(
                "choose-input",
                format!("{}  ▾", self.input_name()),
                self.choosing_input,
                Control::Inputs,
                cx,
            ));
        if self.choosing_input {
            let mut devices = div().flex().flex_col().gap_1().child(self.button(
                "default",
                "Windows default microphone",
                self.prefs.input.is_none(),
                Control::Input(None),
                cx,
            ));
            for (id, name) in &self.inputs {
                devices = devices.child(self.button(
                    id.clone(),
                    name.clone(),
                    self.prefs.input.as_ref() == Some(id),
                    Control::Input(Some(id.clone())),
                    cx,
                ));
            }
            view = view.child(devices);
        }
        let mut mix = row();
        for (mode, name) in [
            (Mixing::Stereo, "Stereo"),
            (Mixing::Average, "Average L + R"),
            (Mixing::Left, "Left only"),
            (Mixing::Right, "Right only"),
        ] {
            let selected = mode == self.prefs.mixing;
            let caption = if selected {
                format!("✓ {name}")
            } else {
                name.into()
            };
            mix = mix.child(self.button(name, caption, selected, Control::Mix(mode), cx));
        }
        view.child(heading("Channel mix"))
            .child(mix)
            .child(heading("Processing"))
            .child(
                row()
                    .child(self.button(
                        "noise",
                        if self.prefs.suppress_noise {
                            "Noise suppression · On"
                        } else {
                            "Noise suppression · Off"
                        },
                        self.prefs.suppress_noise,
                        Control::Noise,
                        cx,
                    ))
                    .child(self.button(
                        "mute",
                        if self.prefs.muted {
                            "Unmute microphone"
                        } else {
                            "Mute microphone"
                        },
                        self.prefs.muted,
                        Control::Mute,
                        cx,
                    )),
            )
            .child(heading("Output gain"))
            .child(
                row()
                    .child(self.button("quieter", "− 5%", false, Control::Gain(-5), cx))
                    .child(format!("{}%", self.prefs.gain_percent))
                    .child(self.button("louder", "+ 5%", false, Control::Gain(5), cx)),
            )
            .child(heading("Output level"))
            .child(
                row()
                    .child(
                        div()
                            .w(px(230.))
                            .h(px(10.))
                            .rounded_md()
                            .bg(rgb(0xcbd9d1))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(peak))
                                    .rounded_md()
                                    .bg(rgb(0x12775b)),
                            ),
                    )
                    .child(db),
            )
            .when_some(status, |v, s| {
                v.child(div().text_sm().child(format!(
                    "Engine {}   ·   Missing frames {}   ·   Dropped frames {}",
                    s.version, s.missing_frames, s.dropped_frames
                )))
            })
            .when(!self.error.is_empty(), |v| v.child(self.error.clone()))
            .child(
                row()
                    .child(self.button(
                        "updates",
                        "Check for updates",
                        false,
                        Control::CheckUpdate,
                        cx,
                    ))
                    .child(self.button("releases", "Open releases", false, Control::Releases, cx)),
            )
            .when(!self.update.is_empty(), |v| v.child(self.update.clone()))
    }
}

fn start_engine() -> io::Result<()> {
    let engine = std::env::current_exe()?.with_file_name("orvia-engine.exe");
    Command::new(engine).spawn().map(|_| ())
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

fn latest_release() -> String {
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
            return "No public release is available. Open releases for private builds.".into();
        }
        Err(e) => return format!("Update check failed: {e}"),
    };
    let Ok(release) = response.body_mut().read_json::<Release>() else {
        return "Unrecognized release information".into();
    };
    let latest = semver::Version::parse(release.tag_name.trim_start_matches('v'));
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"));
    match (latest, current) {
        (Ok(v), Ok(installed)) if v > installed => {
            format!("Version {v} is available. Open releases to update.")
        }
        (Ok(_), Ok(_)) => "You have the latest version.".into(),
        _ => "Unrecognized release version".into(),
    }
}

fn main() -> io::Result<()> {
    let dir = data_dir()?;
    fs::create_dir_all(&dir)?;
    run(&dir);
    Ok(())
}

fn run(dir: &Path) {
    let dir = dir.to_owned();
    Application::new().run(move |cx| {
        cx.bind_keys([
            KeyBinding::new("tab", NextControl, Some("Settings")),
            KeyBinding::new("shift-tab", PreviousControl, Some("Settings")),
            KeyBinding::new("space", Activate, Some("Control")),
            KeyBinding::new("enter", Activate, Some("Control")),
        ]);
        let bounds = Bounds::centered(None, size(px(540.), px(630.)), cx);
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
