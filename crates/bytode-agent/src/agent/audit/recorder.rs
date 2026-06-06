use crate::agent::audit::event::AuditEvent;
use crate::agent::audit::jsonl::JsonlAuditSink;
use crate::agent::audit::sink::AuditSink;
use crate::error::Result;
use std::path::Path;

pub struct AuditRecorder<S = JsonlAuditSink> {
    sink: S,
}

impl AuditRecorder<JsonlAuditSink> {
    pub fn open(_session_id: impl Into<String>, session_root: impl AsRef<Path>) -> Result<Self> {
        let sink = JsonlAuditSink::open(session_root)?;
        Ok(AuditRecorder { sink })
    }
}

impl<S> AuditRecorder<S>
where
    S: AuditSink,
{
    pub fn record(&mut self, event: AuditEvent) -> Result<()> {
        self.sink.append(&event)
    }
}
