//! Resident audio process. Closing the settings window leaves its streams running.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod stream;

use anyhow::Result;
use fs2::FileExt;
use orvia_core::{EngineStatus, Settings, data_dir, write_json};
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
    let mut running: Option<stream::Route> = None;
    let mut previous = None;
    let mut last_attempt = Instant::now() - Duration::from_secs(2);
    let mut last_default_check = last_attempt;
    let mut message = String::new();
    let mut was_observed = false;
    loop {
        let now = Instant::now();
        let settings = Settings::load(&dir.join("settings.json"));
        if previous.as_ref() != settings.as_ref().ok() {
            running = None;
            last_attempt = now - Duration::from_secs(2);
            previous = settings.as_ref().ok().cloned();
        }
        match settings {
            Err(e) => {
                running = None;
                message = format!("Cannot read settings: {e}");
            }
            Ok(s) if s.muted => {
                running = None;
                message = "Muted".into();
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
                    message = "Reconnecting microphone".into();
                }
                if running.is_none() && last_attempt.elapsed() >= Duration::from_secs(1) {
                    last_attempt = now;
                    match stream::Route::open(&s) {
                        Ok(route) => {
                            message = format!("Streaming · {}", route.name);
                            running = Some(route);
                        }
                        Err(e) => message = format!("{e:#}"),
                    }
                }
            }
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
                signal: Default::default(),
                monitoring: false,
                monitor_error: None,
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
}
