//! Bounded event history, written only by the engine's control thread.
use orvia_core::{ENGINE_LOG, ENGINE_LOG_TEXT, EngineEvent, LogEntry, read_json, write_json};
use std::{
    collections::VecDeque,
    io::{self, Write},
    path::{Path, PathBuf},
};
use time::OffsetDateTime;

const LIMIT: usize = 160;

pub(super) struct EngineLog {
    dir: PathBuf,
    entries: VecDeque<LogEntry>,
}

impl EngineLog {
    pub(super) fn open(dir: &Path) -> Self {
        let path = dir.join(ENGINE_LOG);
        let mut entries: VecDeque<LogEntry> = read_json(&path).unwrap_or_default();
        entries.drain(..entries.len().saturating_sub(LIMIT));
        entries.shrink_to(LIMIT);
        Self {
            dir: dir.to_owned(),
            entries,
        }
    }

    pub(super) fn record(&mut self, event: EngineEvent) {
        let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
        let offset = now.offset().whole_minutes();
        let timestamp = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}{:02}:{:02}",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
            if offset < 0 { '-' } else { '+' },
            offset.abs() / 60,
            offset.abs() % 60
        );
        if self.entries.len() == LIMIT {
            self.entries.pop_front();
        } else if self.entries.len() == self.entries.capacity() && self.entries.len() > LIMIT / 2 {
            self.entries.reserve_exact(LIMIT - self.entries.len());
        }
        self.entries.push_back(LogEntry { timestamp, event });
        // A busy disk must not tear down a running stream. The next event retries the write.
        let _ = write_json(&self.dir.join(ENGINE_LOG), &self.entries);
        let _ = self.write_text();
    }

    /// Replace the readable history atomically so an editor can reload a complete file.
    fn write_text(&self) -> io::Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        let mut writer = io::BufWriter::new(file.as_file_mut());
        for entry in &self.entries {
            let (level, message, detail) = match &entry.event {
                EngineEvent::Started(v) => ("INFO", "Engine started", Some(v.as_str())),
                EngineEvent::Stopped => ("INFO", "Engine stopped", None),
                EngineEvent::Streaming(route) => ("INFO", "Streaming", Some(route.as_str())),
                EngineEvent::Reconnecting => ("WARN", "Reconnecting microphone", None),
                EngineEvent::RouteError(e) => {
                    ("ERROR", "Audio route unavailable", Some(e.as_str()))
                }
                EngineEvent::Noise(method) => (
                    "INFO",
                    "Noise suppression",
                    Some(method.as_deref().unwrap_or("off")),
                ),
                EngineEvent::Monitoring(true) => ("INFO", "Headphone monitor started", None),
                EngineEvent::Monitoring(false) => ("INFO", "Headphone monitor stopped", None),
                EngineEvent::MonitorError(e) => {
                    ("ERROR", "Headphone monitor unavailable", Some(e.as_str()))
                }
                EngineEvent::Unknown => ("INFO", "Unrecognized event", None),
            };
            write!(writer, "{}  {level:<5}  {message}", entry.timestamp)?;
            if let Some(detail) = detail {
                write!(
                    writer,
                    ": {}",
                    detail.replace("\r\n", " ").replace(['\r', '\n'], " ")
                )?;
            }
            writeln!(writer)?;
        }
        writer.flush()?;
        drop(writer);
        file.persist(self.dir.join(ENGINE_LOG_TEXT))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_engine_can_resume_the_bounded_history() {
        let dir = std::env::temp_dir().join(format!("orvia-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut log = EngineLog::open(&dir);
        log.entries.clear();
        for i in 0..LIMIT {
            log.entries.push_back(LogEntry {
                timestamp: String::new(),
                event: EngineEvent::Started(i.to_string()),
            });
        }
        log.record(EngineEvent::Stopped);
        let reopened = EngineLog::open(&dir);
        assert_eq!(reopened.entries.len(), LIMIT);
        assert_eq!(reopened.entries.capacity(), LIMIT);
        assert_eq!(
            reopened.entries.front().unwrap().event,
            EngineEvent::Started("1".into())
        );
        let last = reopened.entries.back().unwrap();
        assert_eq!(last.event, EngineEvent::Stopped);
        assert!(!last.timestamp.is_empty());
        let text = std::fs::read_to_string(dir.join(ENGINE_LOG_TEXT)).unwrap();
        assert_eq!(text.lines().count(), LIMIT);
        assert!(
            text.lines()
                .last()
                .unwrap()
                .ends_with("INFO   Engine stopped")
        );
        std::fs::remove_file(dir.join(ENGINE_LOG)).unwrap();
        std::fs::remove_file(dir.join(ENGINE_LOG_TEXT)).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn reopening_an_oversized_history_keeps_only_the_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<_> = (0..LIMIT + 7)
            .map(|i| LogEntry {
                timestamp: i.to_string(),
                event: EngineEvent::Reconnecting,
            })
            .collect();
        write_json(&dir.path().join(ENGINE_LOG), &entries).unwrap();
        let log = EngineLog::open(dir.path());
        assert_eq!(log.entries.capacity(), LIMIT);
        assert_eq!(log.entries.front().unwrap().timestamp, "7");
    }

    #[test]
    fn text_log_includes_existing_events_and_keeps_multiline_errors_on_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let old = LogEntry {
            timestamp: "12:01:02".into(),
            event: EngineEvent::Reconnecting,
        };
        write_json(&dir.path().join(ENGINE_LOG), &[old]).unwrap();
        let mut log = EngineLog::open(dir.path());
        log.record(EngineEvent::RouteError("USB マイク\r\ndisconnected".into()));
        let text = std::fs::read_to_string(dir.path().join(ENGINE_LOG_TEXT)).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "12:01:02  WARN   Reconnecting microphone");
        assert!(lines[1].ends_with("ERROR  Audio route unavailable: USB マイク disconnected"));
    }
}
