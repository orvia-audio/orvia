//! Resident audio process. Closing the settings window leaves its streams running.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod log;
mod spectrum;
mod stream;
#[cfg(not(target_os = "macos"))]
mod tray;
#[cfg(target_os = "macos")]
#[path = "menu_bar.rs"]
mod tray;

use anyhow::Result;
use fs2::FileExt;
use orvia_core::{
    AUDITION_FILE, Audition, EngineEvent, EngineStatus, Settings, data_dir, read_json, write_json,
};
use std::{
    fs, io,
    path::Path,
    thread,
    time::{Duration, Instant, SystemTime},
};

fn main() -> Result<()> {
    let dir = data_dir()?;
    fs::create_dir_all(&dir)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("engine.lock"))?;
    // The OS releases this per-user lock on both clean and abnormal exits.
    if let Err(e) = lock.try_lock_exclusive() {
        if e.raw_os_error() == Some(33) || e.kind() == io::ErrorKind::WouldBlock {
            return Ok(());
        }
        return Err(e.into());
    }
    let _ = fs::remove_file(stop_request(&dir));
    #[cfg(unix)]
    {
        use std::io::Write;
        lock.set_len(0)?;
        writeln!(&lock, "{}", std::process::id())?;
    }
    tray::run(&dir)
}

fn stop_request(dir: &Path) -> std::path::PathBuf {
    dir.join(format!("stop-engine-{}", std::process::id()))
}

fn recently_observed(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(5))
}

fn supervise(dir: &Path, mut menu: impl FnMut(Option<orvia_core::Language>) -> bool) -> Result<()> {
    let stop_request = stop_request(dir);
    let mut log = log::EngineLog::open(dir);
    log.record(EngineEvent::Started(env!("CARGO_PKG_VERSION").into()));
    let mut last_monitor = (false, None);
    let mut running: Option<stream::Route> = None;
    let mut previous: Option<Settings> = None;
    let mut last_attempt = Instant::now() - Duration::from_secs(2);
    let mut last_default_check = last_attempt;
    let mut message = String::new();
    let mut was_observed = false;
    loop {
        if stop_request.exists() {
            let _ = fs::remove_file(&stop_request);
            break;
        }
        let now = Instant::now();
        let observed = recently_observed(&dir.join("observe"));
        let settings = Settings::load(&dir.join("settings.json"));
        if menu(settings.as_ref().ok().map(|s| s.language)) {
            break;
        }
        if !matches!((&previous, &settings), (Some(old), Ok(new)) if old.same_signal_path(new)) {
            running = None;
            last_attempt = now - Duration::from_secs(2);
        }
        if previous.as_ref() != settings.as_ref().ok() {
            if let Ok(new) = &settings
                && previous.as_ref().map(Settings::active_noise_method)
                    != Some(new.active_noise_method())
            {
                log.record(EngineEvent::Noise(new.active_noise_method().map(
                    |method| match method {
                        orvia_core::NoiseMethod::Rnnoise => "rnnoise".into(),
                        orvia_core::NoiseMethod::WebRtc => "webrtc".into(),
                        orvia_core::NoiseMethod::DeepFilterNet3 => "deepfilternet3".into(),
                    },
                )));
            }
            previous = settings.as_ref().ok().cloned();
        }
        let mut audition = None;
        match settings {
            Err(e) => {
                running = None;
                let error = format!("Cannot read settings: {e}");
                if message != error {
                    log.record(EngineEvent::RouteError(error.clone()));
                    message = error;
                }
            }
            Ok(mut s) => {
                let default_changed = if s.input.is_none()
                    && last_default_check.elapsed() >= Duration::from_secs(2)
                {
                    last_default_check = now;
                    running
                        .as_ref()
                        .is_some_and(|r| Some(&r.input_id) != stream::default_input_id().as_ref())
                } else {
                    false
                };
                if running.as_ref().is_some_and(|r| !r.healthy()) || default_changed {
                    running = None;
                    log.record(EngineEvent::Reconnecting);
                    message = "Reconnecting microphone".into();
                }
                if running.is_none() && last_attempt.elapsed() >= Duration::from_secs(1) {
                    last_attempt = now;
                    match stream::Route::open(&s) {
                        Ok(route) => {
                            log.record(EngineEvent::Streaming(route.name.clone()));
                            message = format!("Streaming · {}", route.name);
                            running = Some(route);
                        }
                        Err(e) => {
                            let error = format!("{e:#}");
                            if error != message {
                                log.record(EngineEvent::RouteError(error.clone()));
                                message = error;
                            }
                        }
                    }
                }
                if let Some(route) = &mut running {
                    let request = read_json::<Audition>(&dir.join(AUDITION_FILE))
                        .ok()
                        .filter(|r| r.active(&s, SystemTime::now()));
                    if let Some(r) = &request {
                        r.target.apply(&mut s);
                    }
                    route.update_controls(&s, observed);
                    audition = request
                        .filter(|_| route.controls_applied(&s))
                        .map(|r| r.started);
                }
            }
        }
        let monitor = running
            .as_ref()
            .map(stream::Route::monitor_status)
            .unwrap_or_default();
        if monitor != last_monitor {
            if let Some(error) = &monitor.1 {
                log.record(EngineEvent::MonitorError(error.clone()));
            } else if monitor.0 || last_monitor.0 {
                log.record(EngineEvent::Monitoring(monitor.0));
            }
            last_monitor = monitor;
        }
        // Persist diagnostics only while a settings window is observing the engine.
        let spectrum = running.as_mut().and_then(|route| {
            route
                .spectrum
                .poll(observed && recently_observed(&dir.join(orvia_core::spectrum::OBSERVER_FILE)))
        });
        if observed {
            let (peak, missing_frames, dropped_frames) = running
                .as_ref()
                .map(stream::Route::metrics)
                .unwrap_or_default();
            let status = EngineStatus {
                spectrum,
                signal: running
                    .as_ref()
                    .map(stream::Route::envelopes)
                    .unwrap_or_default(),
                monitoring: last_monitor.0,
                monitor_error: last_monitor.1.clone(),
                audition,
                observed: SystemTime::now(),
                version: env!("CARGO_PKG_VERSION").into(),
                message: message.clone(),
                peak,
                missing_frames,
                dropped_frames,
            };
            // A busy diagnostics file must not interrupt the audio streams.
            let _ = write_json(&dir.join("status.json"), &status);
        } else if was_observed {
            let _ = fs::remove_file(dir.join("status.json"));
        }
        was_observed = observed;
        let idle = running.is_none() && !observed;
        thread::sleep(Duration::from_millis(if idle { 500 } else { 200 }));
    }
    log.record(EngineEvent::Stopped);
    Ok(())
}
