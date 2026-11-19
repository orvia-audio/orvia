//! Resident audio process. Closing the settings window leaves its streams running.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod log;
mod stream;
mod tray;

use anyhow::Result;
use fs2::FileExt;
use orvia_core::{EngineEvent, EngineStatus, Settings, data_dir, write_json};
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
    // Windows releases this per-user lock on both clean and abnormal exits.
    if let Err(e) = lock.try_lock_exclusive() {
        if e.raw_os_error() == Some(33) || e.kind() == io::ErrorKind::WouldBlock {
            return Ok(());
        }
        return Err(e.into());
    }
    supervise(&dir)
}

fn supervise(dir: &Path) -> Result<()> {
    let stop_request = dir.join(format!("stop-engine-{}", std::process::id()));
    let _ = fs::remove_file(&stop_request);
    let mut log = log::EngineLog::open(dir);
    log.record(EngineEvent::Started(env!("CARGO_PKG_VERSION").into()));
    let mut last_monitor = (false, None);
    let mut running: Option<stream::Route> = None;
    let mut previous: Option<Settings> = None;
    let mut tray = tray::Tray::new().ok();
    let mut tray_attempt = Instant::now();
    let mut last_attempt = Instant::now() - Duration::from_secs(2);
    let mut last_default_check = last_attempt;
    let mut message = String::new();
    let mut was_observed = false;
    loop {
        if stop_request.exists() {
            break;
        }
        let now = Instant::now();
        if tray.is_none() && tray_attempt.elapsed() >= Duration::from_secs(10) {
            tray = tray::Tray::new().ok();
            tray_attempt = now;
        }
        if let Some(tray) = &tray
            && matches!(tray.handle(dir), Ok(true))
        {
            break;
        }
        let settings = Settings::load(&dir.join("settings.json"));
        if let Ok(s) = &settings
            && let Some(tray) = &mut tray
        {
            tray.refresh(s);
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
                log.record(EngineEvent::Noise(new.active_noise_method()));
            }
            previous = settings.as_ref().ok().cloned();
        }
        match settings {
            Err(e) => {
                running = None;
                let error = format!("Cannot read settings: {e}");
                if message != error {
                    log.record(EngineEvent::RouteError(error.clone()));
                    message = error;
                }
            }
            Ok(s) if s.muted => {
                running = None;
                if message != "Muted" {
                    log.record(EngineEvent::Muted);
                    message = "Muted".into();
                }
            }
            Ok(s) => {
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
                    route.update_controls(&s);
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
        let observed = fs::metadata(dir.join("observe"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|a| a < Duration::from_secs(5));
        if observed {
            let (peak, missing_frames, dropped_frames) = running
                .as_ref()
                .map(stream::Route::metrics)
                .unwrap_or_default();
            let status = EngineStatus {
                signal: running
                    .as_ref()
                    .map(stream::Route::envelopes)
                    .unwrap_or_default(),
                monitoring: last_monitor.0,
                monitor_error: last_monitor.1.clone(),
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
        thread::sleep(Duration::from_millis(200));
    }
    log.record(EngineEvent::Stopped);
    Ok(())
}
