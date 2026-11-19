//! Lifecycle controls for the audio engine installed beside this window.
use orvia_core::{EngineStatus, read_json};
use std::{
    fs, io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime},
};

fn executable() -> io::Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name("orvia-engine.exe"))
}

pub(super) fn start() -> io::Result<Child> {
    Command::new(executable()?).spawn()
}

/// Restart just this copy of the engine in the current Windows session.
/// Complete once the relaunched process publishes a fresh diagnostic snapshot.
pub(super) fn restart(dir: &Path) -> io::Result<()> {
    let engine = executable()?;
    let powershell = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Windows system directory is unavailable"))?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let output = Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("stop-engine.ps1"),
        ])
        .env("ORVIA_ENGINE_PATH", &engine)
        .env("ORVIA_STATE_DIR", dir)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    fs::write(dir.join("observe"), b"1")?;
    let launched = SystemTime::now();
    let mut child = Command::new(engine).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = dir.join("status.json");
    while Instant::now() < deadline {
        if let Some(exit) = child.try_wait()? {
            return Err(io::Error::other(format!("Audio engine exited ({exit})")));
        }
        if read_json::<EngineStatus>(&status)
            .is_ok_and(|s| s.observed >= launched && s.version == env!("CARGO_PKG_VERSION"))
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Audio engine has not reported its status",
    ))
}
