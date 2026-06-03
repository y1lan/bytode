//! Serializable session log entries. These are the only facts written to
//! `events/log.jsonl`; everything else (overlay, context view) is derived.

use super::artifact::ArtifactRef;
use crate::session::compact::MicroCompactPolicy;
use serde::{Deserialize, Serialize};

/// Stable session identifier, e.g. `project-<hash>` or `project-<hash>-<secs>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable log entry identifier. Derived from the monotonic `seq`, never from a
/// provider or LLM, so it is reproducible across replays.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryId(pub String);

impl EntryId {
    pub fn from_seq(seq: u64) -> Self {
        EntryId(format!("e{seq:08}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Common header for every entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryMeta {
    pub id: EntryId,
    pub seq: u64,
    pub created_at: i64,
}

/// One append-only fact in the session log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub meta: EntryMeta,
    pub kind: SessionEntryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionEntryKind {
    UserMessage(UserMessageEntry),
    AssistantMessage(AssistantMessageEntry),
    ToolCall(ToolCallEntry),
    ToolResult(ToolResultEntry),
    MicroCompact(MicroCompactEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessageEntry {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessageEntry {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEntry {
    pub tool_name: String,
    pub inline_args: Option<serde_json::Value>,
    pub arg_artifacts: Vec<ArtifactRef>,
    pub parent_assistant_entry_id: Option<EntryId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultEntry {
    pub call_entry_id: EntryId,
    pub status: ToolStatus,
    pub inline_content: Option<String>,
    pub artifacts: Vec<ArtifactRef>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    Ok,
    Error,
    Cancelled,
}

/// Audit record of a deterministic micro compact pass. Appended (never
/// overwrites the entries it compacts) so the log stays the single source of
/// truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicroCompactEntry {
    pub compacted_entry_ids: Vec<EntryId>,
    pub archived_artifacts: Vec<ArtifactRef>,
    pub policy_snapshot: MicroCompactPolicy,
    pub operation_digest: String,
}

/// Return value of a micro compact pass (not persisted; the `MicroCompactEntry`
/// is the persisted fact).
#[derive(Debug, Clone)]
pub struct MicroCompactResult {
    pub compact_entry_id: EntryId,
    pub compact_entry_seq: u64,
    pub archived_artifacts: Vec<ArtifactRef>,
    pub compacted_entry_ids: Vec<EntryId>,
    pub saved_bytes_estimate: usize,
    pub operation_digest: String,
}

/// Derived view describing how a compacted entry should appear in context.
/// Built from the log; never persisted as authoritative data.
#[derive(Debug, Clone)]
pub struct CompactedEntryView {
    pub original_entry_id: EntryId,
    pub replacement_preview: String,
    pub artifact_refs: Vec<ArtifactRef>,
    pub compact_entry_id: EntryId,
}
