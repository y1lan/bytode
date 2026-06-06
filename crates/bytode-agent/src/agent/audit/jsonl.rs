use crate::agent::audit::event::AuditEvent;
use crate::agent::audit::sink::AuditSink;
use crate::error::Result;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub const AUDIT_LOG_FILE: &str = "audit.jsonl";

pub struct JsonlAuditSink {
    file: File,
}

impl JsonlAuditSink {
    pub fn open(session_root: impl AsRef<Path>) -> Result<Self> {
        let session_root = session_root.as_ref();
        std::fs::create_dir_all(session_root)?;
        let path = session_root.join(AUDIT_LOG_FILE);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(JsonlAuditSink { file })
    }
}

impl AuditSink for JsonlAuditSink {
    fn append(&mut self, event: &AuditEvent) -> Result<()> {
        let line = serde_json::to_string(event)?;
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        Ok(())
    }
}
