//! Append-only `events/log.jsonl`. The log is the single source of truth;
//! history entries are never rewritten, only appended.

use crate::error::{BytodeError, Result};
use crate::session::model::SessionEntry;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Appends entries to `log.jsonl`, one JSON object per line, flushing each.
pub struct SessionLogWriter {
    file: File,
}

impl SessionLogWriter {
    pub fn open(log_path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        Ok(SessionLogWriter { file })
    }

    /// Append one entry and flush. Compact entries are appended here too — the
    /// log is never rewritten in place.
    pub fn append(&mut self, entry: &SessionEntry) -> Result<()> {
        let line = serde_json::to_string(entry)?;
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        Ok(())
    }
}

/// Replays `log.jsonl` line by line.
pub struct SessionLogReader {
    log_path: PathBuf,
}

impl SessionLogReader {
    pub fn new(log_path: PathBuf) -> Self {
        SessionLogReader { log_path }
    }

    /// Replay every entry. A single corrupt line yields a diagnostic error
    /// (with its line number) rather than a panic.
    pub fn read_all(&self) -> Result<Vec<SessionEntry>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.log_path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for (idx, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: SessionEntry = serde_json::from_str(&line).map_err(|e| {
                BytodeError::Session(format!(
                    "corrupt log line {} in {}: {e}",
                    idx + 1,
                    self.log_path.display()
                ))
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }
}
