//! Bounded event history, written only by the engine's control thread.
use orvia_core::{ENGINE_LOG, EngineEvent, LogEntry, read_json, write_json};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};
use time::OffsetDateTime;

const LIMIT: usize = 160;

pub(super) struct EngineLog {
    path: PathBuf,
    entries: VecDeque<LogEntry>,
}

impl EngineLog {
    pub(super) fn open(dir: &Path) -> Self {
        let path = dir.join(ENGINE_LOG);
        let entries = read_json(&path).unwrap_or_default();
        Self { path, entries }
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
        self.entries.push_back(LogEntry { timestamp, event });
        while self.entries.len() > LIMIT {
            self.entries.pop_front();
        }
        // A busy disk must not tear down a running stream. The next event retries the write.
        let _ = write_json(&self.path, &self.entries);
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
        assert_eq!(
            reopened.entries.front().unwrap().event,
            EngineEvent::Started("1".into())
        );
        let last = reopened.entries.back().unwrap();
        assert_eq!(last.event, EngineEvent::Stopped);
        assert!(!last.timestamp.is_empty());
        std::fs::remove_file(reopened.path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
