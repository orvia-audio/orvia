//! Deterministic sessions for the Linux screenshot runner in tests/ui.
use super::*;
use crate::view::{AdjustmentScreen, ReadingCue};
use orvia_core::{EngineEvent, LogEntry, SignalHistory, calibration::Recommendation};
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
        "mixer-bypass" => {
            panel.prefs.suppress_noise = false;
            Page::Mixer
        }
        "devices" => Page::Devices,
        "devices-picker" => {
            panel.choosing = Some(Endpoint::Input);
            Page::Devices
        }
        "preferences" => Page::Preferences,
        "preferences-checking" => {
            panel.updates.start();
            Page::Preferences
        }
        "log" => {
            panel.log.entries = vec![
                LogEntry {
                    timestamp: "10:24:00".into(),
                    event: EngineEvent::Started(panel.installed.to_string()),
                },
                LogEntry {
                    timestamp: "10:24:01".into(),
                    event: EngineEvent::Streaming("Studio microphone".into()),
                },
                LogEntry {
                    timestamp: "10:25:00".into(),
                    event: EngineEvent::Reconnecting,
                },
            ];
            Page::Log
        }
        "mic" | "mic-standalone" => Page::MicTest,
        "mic-recorded" | "mic-playing" | "adjust-ready" => {
            panel.test.clip =
                Some(Clip::load(&panel.dir.join("take.wav")).expect("load the synthetic take"));
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
                "adjust-recording" => AdjustmentScreen::Phrase(2, ReadingCue::Recording),
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
