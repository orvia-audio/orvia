//! Lifecycle controls for the audio engine installed beside this window.
use orvia_core::{EngineStatus, read_json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime},
};

fn executable() -> io::Result<PathBuf> {
    Ok(std::env::current_exe()?
        .with_file_name(format!("orvia-engine{}", std::env::consts::EXE_SUFFIX)))
}

pub(super) fn start() -> io::Result<Child> {
    Command::new(executable()?).spawn()
}

#[cfg(windows)]
fn stop(dir: &Path, engine: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
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
        .env("ORVIA_ENGINE_PATH", engine)
        .env("ORVIA_STATE_DIR", dir)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn stop(dir: &Path, _: &Path) -> io::Result<()> {
    use fs2::FileExt;
    let path = dir.join("engine.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut requested = None;
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Audio engine did not stop",
            ));
        }
        if let Ok(pid) = fs::read_to_string(&path)?.trim().parse::<u32>()
            && requested != Some(pid)
        {
            fs::write(dir.join(format!("stop-engine-{pid}")), b"1")?;
            requested = Some(pid);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Replace the current user's running engine and wait for its first fresh observation.
pub(super) fn restart(dir: &Path) -> io::Result<()> {
    let engine = executable()?;
    stop(dir, &engine)?;
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
