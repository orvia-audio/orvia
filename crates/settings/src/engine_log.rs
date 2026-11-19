//! Log-file observation and localized descriptions for the preferences viewer.
use orvia_core::{ENGINE_LOG, EngineEvent, Language, LogEntry, NoiseMethod, read_json};
use std::{fs, io, path::Path, time::SystemTime};

#[derive(Default)]
pub(super) struct LogView {
    modified: Option<SystemTime>,
    pub(super) entries: Vec<LogEntry>,
    pub(super) error: Option<String>,
}

impl LogView {
    /// Load a changed snapshot, retaining the last readable event history on a sharing error.
    pub(super) fn refresh(&mut self, dir: &Path) -> bool {
        let path = dir.join(ENGINE_LOG);
        let read = || -> io::Result<_> {
            let modified = fs::metadata(&path)?.modified()?;
            if self.modified == Some(modified) {
                return Ok(None);
            }
            Ok(Some((modified, read_json(&path)?)))
        };
        match read() {
            Ok(Some((modified, entries))) => {
                self.modified = Some(modified);
                self.entries = entries;
                self.error = None;
                true
            }
            Ok(None) => self.error.take().is_some(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let changed = self.modified.take().is_some() || self.error.is_some();
                self.entries.clear();
                self.error = None;
                changed
            }
            Err(e) => {
                let msg = e.to_string();
                let changed = self.error.as_ref() != Some(&msg);
                self.error = Some(msg);
                changed
            }
        }
    }
}

pub(super) fn description(event: &EngineEvent, l: Language) -> String {
    match event {
        EngineEvent::Started(v) => format!("{} · {v}", l.text("Engine started", "エンジンを起動")),
        EngineEvent::Stopped => l.text("Engine stopped", "エンジンを停止").into(),
        EngineEvent::Streaming(route) => {
            format!("{} · {route}", l.text("Streaming", "音声出力を開始"))
        }
        EngineEvent::Unknown => l.text("Unrecognized event", "不明なイベント").into(),
        EngineEvent::Reconnecting => l.text("Reconnecting microphone", "マイクを再接続中").into(),
        EngineEvent::RouteError(e) => format!(
            "{} · {e}",
            l.text("Audio route unavailable", "音声経路を開けません")
        ),
        EngineEvent::Noise(method) => {
            let name = match method {
                None => l.text("Off", "オフ"),
                Some(NoiseMethod::Rnnoise) => "RNNoise",
                Some(NoiseMethod::Spectral) => l.text("Spectral subtraction", "スペクトル減算"),
            };
            format!("{} · {name}", l.text("Noise suppression", "ノイズ抑制"))
        }
        EngineEvent::Monitoring(true) => l
            .text("Headphone monitor started", "ヘッドホンモニターを開始")
            .into(),
        EngineEvent::Monitoring(false) => l
            .text("Headphone monitor stopped", "ヘッドホンモニターを停止")
            .into(),
        EngineEvent::MonitorError(e) => format!(
            "{} · {e}",
            l.text(
                "Headphone monitor unavailable",
                "ヘッドホンモニターを開けません"
            )
        ),
    }
}
