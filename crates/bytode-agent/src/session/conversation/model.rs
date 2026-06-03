//! Canonical conversation model — persistent, boundary-correct facts for
//! provider request construction. Never derived from flat `SessionEntry` replay.

use crate::session::model::ArtifactRef;
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------
// ID newtypes — never bare String
// ------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResponseId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CanonicalMessageId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

// ------------------------------------------------------------------
// Top-level record
// ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalRecord {
    pub meta: CanonicalMeta,
    pub kind: CanonicalRecordKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CanonicalMeta {
    pub seq: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CanonicalRecordKind {
    TurnStarted(CanonicalTurnStarted),
    AssistantResponse(CanonicalAssistantResponse),
    ToolResults(CanonicalToolResults),
    TurnFinished(CanonicalTurnFinished),
    CompactReplacement(CanonicalCompactReplacement),
}

// ------------------------------------------------------------------
// Turn boundary
// ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalTurnStarted {
    pub turn_id: TurnId,
    pub user_message_id: CanonicalMessageId,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalTurnFinished {
    pub turn_id: TurnId,
}

// ------------------------------------------------------------------
// Assistant response
// ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalAssistantResponse {
    pub turn_id: TurnId,
    pub response_id: ResponseId,
    pub message_id: CanonicalMessageId,
    pub parts: Vec<CanonicalAssistantPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CanonicalAssistantPart {
    Text {
        content: String,
    },
    ToolCall {
        tool_call_id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
    },
}

// ------------------------------------------------------------------
// Tool results
// ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalToolResults {
    pub turn_id: TurnId,
    pub response_id: ResponseId,
    pub results: Vec<CanonicalToolResultPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalToolResultPart {
    pub tool_call_id: ToolCallId,
    pub status: CanonicalToolStatus,
    pub content: CanonicalContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CanonicalContent {
    Inline(String),
    Artifact(ArtifactRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CanonicalToolStatus {
    Ok,
    Error,
    Cancelled,
}

// ------------------------------------------------------------------
// Compact
// ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalCompactReplacement {
    pub source: CanonicalSpan,
    pub content: String,
    pub evidence_pack_ref: ArtifactRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalSpan {
    pub start_seq: u64,
    pub end_seq_exclusive: u64,
}
