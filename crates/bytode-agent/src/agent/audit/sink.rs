use crate::agent::audit::event::AuditEvent;
use crate::error::Result;

pub trait AuditSink {
    fn append(&mut self, event: &AuditEvent) -> Result<()>;
}
