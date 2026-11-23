//! Deterministic sessions for the Linux screenshot runner in tests/ui.
use super::*;
use crate::view::{AdjustmentScreen, ReadingCue};
use orvia_core::{SignalHistory, calibration::Recommendation};
use orvia_engine::mic_test::Clip;

/// Open the production panel without starting an engine, update check, or microphone stream.
#[test]
#[ignore = "driven by tests/ui/snapshots.py under Xvfb"]
fn render_window() {
    let scene = std::env::var("ORVIA_SNAPSHOT_SCENE").expect("missing snapshot scene");
    let language = match std::env::var("ORVIA_SNAPSHOT_LANGUAGE").as_deref() {
        Ok("en") => Language::English,
        Ok("ja") => Language::Japanese,
        _ => panic!("unsupported snapshot language"),
    };
    let dir = PathBuf::from(std::env::var_os("ORVIA_SNAPSHOT_DIR").expect("missing fixture path"));
    eprintln!("Creating snapshot window for {scene}");
    Application::new().with_assets(icons::Icons).run(move |cx| {
        let height = if scene == "mixer-full" { 900. } else { 610. };
        let bounds = Bounds::centered(None, size(px(830.), px(height)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Orvia snapshot".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let mut panel =
                        Panel::empty(dir, Version::new(1, 0, 0), scene == "mic-standalone", cx);
                    window.focus(&panel.root);
                    populate(&mut panel, &scene, language);
                    panel
                })
            },
        )
        .expect("open the snapshot window");
        cx.activate(true);
        eprintln!("Snapshot window opened");
    });
}

fn device(id: &str, name: &str) -> (String, String) {
    (id.into(), name.into())
}

fn populate(panel: &mut Panel, scene: &str, language: Language) {
    panel.inputs = vec![
        device("mic", "Studio microphone"),
        device("usb", "USB microphone"),
    ];
    panel.outputs = vec![device("send", "Virtual cable input")];
    panel.receivers = vec![device("receive", "Virtual cable output")];
    panel.monitors = vec![device("phones", "Headphones")];
    panel.prefs.language = language;
    panel.prefs.input = Some("mic".into());
    panel.prefs.output = Some("send".into());
    panel.prefs.receiver = Some("receive".into());
    panel.prefs.monitor = Some("phones".into());
    panel.prefs.monitor_enabled = true;
    panel.prefs.deep_filter_limit_db = 12;
    panel.test.sources.clone_from(&panel.receivers);
    panel.test.source.clone_from(&panel.prefs.receiver);
    panel.test.speaker.clone_from(&panel.prefs.monitor);
    panel.status = Some(EngineStatus {
        spectrum: None,
        observed: std::time::SystemTime::UNIX_EPOCH,
        version: panel.installed.to_string(),
        message: "Streaming".into(),
        signal: SignalHistory {
            peaks: (0..200)
                .map(|i| {
                    let level = 12 + (i % 35_u8).abs_diff(15) + (i % 9);
                    [level, level - 8]
                })
                .collect(),
            attenuation_db: Some(8.0),
        },
        monitoring: true,
        monitor_error: None,
        audition: None,
        peak: 0.125,
        missing_frames: 0,
        dropped_frames: 0,
    });
    panel.page = match scene {
        "mixer" | "mixer-full" => Page::Mixer,
        "mixer-spectrum" | "mixer-spectrum-silent" | "mixer-spectrum-inspect" => {
            use orvia_core::spectrum::Spectrum;
            panel.show_spectrum = true;
            let silent = scene == "mixer-spectrum-silent";
            panel.spectrum_band = (scene == "mixer-spectrum-inspect").then_some(32);
            panel.status.as_mut().unwrap().spectrum = Some(Spectrum {
                bands: (0..Spectrum::BANDS)
                    .map(|i| {
                        if silent {
                            return 0;
                        }
                        [(32, 67_u8), (39, 49), (43, 38), (12, 22)]
                            .into_iter()
                            .map(|(bin, level)| {
                                level.saturating_sub((i.abs_diff(bin) as u8).saturating_mul(6))
                            })
                            .max()
                            .unwrap()
                            .max(5 + (i % 4) as u8)
                    })
                    .collect(),
                peak_hz: (!silent).then_some(440),
            });
            Page::Mixer
        }
        "mixer-bypass" => {
            panel.prefs.suppress_noise = false;
            Page::Mixer
        }
        name if name.starts_with("tone-") => {
            use tone_setup::Screen;
            let tone = orvia_core::Tone {
                low_cut_hz: 65,
                gain_percent: 135,
                compressor_ratio: 2,
            };
            panel.tone_snapshot = Some(match name {
                "tone-setup" => Screen::Intro,
                "tone-opening" => Screen::Opening,
                "tone-room" => Screen::Room,
                "tone-measuring" => Screen::Reading(ReadingCue::Measuring),
                "tone-read" => Screen::Reading(ReadingCue::Read),
                "tone-analyzing" => Screen::Analyzing,
                "tone-ready" => Screen::Ready(tone, None),
                "tone-playing" => Screen::Ready(tone, Some(1)),
                "tone-failed" => Screen::Failed(language.text(
                    "The mix changed. Record again.",
                    "ミックス設定が変わりました。録り直してください。",
                )),
                _ => panic!("unknown tone setup scene {name}"),
            });
            Page::Mixer
        }
        "devices" => Page::Devices,
        "devices-picker" => {
            panel.choosing = Some(Endpoint::Input);
            Page::Devices
        }
        "preferences" => Page::Preferences,
        "preferences-current" | "preferences-available" => {
            panel.show_update_feedback = true;
            let version = if scene == "preferences-available" {
                Version::new(1, 1, 0)
            } else {
                panel.installed.clone()
            };
            panel.updates.finish(Ok(updates::Check::new(Some(version))));
            Page::Preferences
        }
        "preferences-error" => {
            panel.show_update_feedback = true;
            panel.updates.finish(Err(updates::CheckError::Network));
            Page::Preferences
        }
        "mic-error" => {
            panel.error = language
                .text(
                    "Microphone disconnected or stalled.",
                    "マイクから音声が届いていません。",
                )
                .into();
            Page::MicTest
        }
        "mic-offline" => {
            panel.status = None;
            Page::MicTest
        }
        "preferences-checking" => {
            panel.show_update_feedback = true;
            panel.updates.start();
            Page::Preferences
        }
        "mic" | "mic-standalone" => Page::MicTest,
        "mic-recorded" | "mic-playing" | "adjust-ready" => {
            panel.test.clip = Some(microphone_test::Take::Preview(
                Clip::load(&panel.dir.join("take.wav")).expect("load the synthetic take"),
            ));
            if scene == "mic-playing" {
                panel.test.snapshot_action = Some(TestAction::StopPlayback);
            } else if scene == "adjust-ready" {
                panel.adjustment_snapshot = Some(AdjustmentScreen::Ready(Recommendation {
                    limit_db: 12,
                    onset_loss_db: 1.5,
                    room_reduction_db: 9.0,
                }));
            }
            Page::MicTest
        }
        "mic-recording" => {
            for _ in 0..30 {
                panel.test.reader.observe(1e-7, false);
            }
            for _ in 0..10 {
                panel.test.reader.observe(0.02, true);
            }
            panel.test.snapshot_action = Some(TestAction::FinishRecording);
            Page::MicTest
        }
        _ => {
            panel.adjustment_snapshot = Some(match scene {
                "adjust-room" => AdjustmentScreen::Room,
                "adjust-measuring" => AdjustmentScreen::Phrase(1, ReadingCue::Measuring),
                "adjust-read" => AdjustmentScreen::Phrase(1, ReadingCue::Read),
                "adjust-recording" => AdjustmentScreen::Phrase(2, ReadingCue::Read),
                "adjust-analyzing" => AdjustmentScreen::Analyzing,
                "adjust-opening" => AdjustmentScreen::Preview(None),
                "adjust-preview" => AdjustmentScreen::Preview(Some(ReadingCue::Read)),
                "adjust-restoring" => AdjustmentScreen::Restoring,
                "adjust-failed" => AdjustmentScreen::Failed(language.text(
                    "Microphone disconnected or stalled.",
                    "マイクから音声が届いていません。",
                )),
                _ => panic!("unknown snapshot scene {scene}"),
            });
            Page::MicTest
        }
    };
}
