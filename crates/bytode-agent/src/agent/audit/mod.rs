//! Append-only tool-call audit records.
//!
//! This module owns only the audit event model and JSONL storage. Engine
//! integration is expected to construct events and call `AuditRecorder::record`.

pub(crate) mod event;
mod jsonl;
mod recorder;
mod sink;

pub(crate) use event::{AuditEvent, AuditEventKind, AuditEventSummary, truncate_summary};
pub(crate) use recorder::AuditRecorder;
