use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_SUMMARY_LIMIT: usize = 2_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuditEvent {
    pub event_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub timestamp: i64,
    pub kind: AuditEventKind,
    #[serde(default, flatten)]
    pub summary: AuditEventSummary,
}

impl AuditEvent {
    pub fn new(
        event_id: impl Into<String>,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        tool_call_id: impl Into<String>,
        kind: AuditEventKind,
    ) -> Self {
        AuditEvent {
            event_id: event_id.into(),
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            tool_call_id: tool_call_id.into(),
            timestamp: unix_timestamp(),
            kind,
            summary: AuditEventSummary::default(),
        }
    }

    pub fn with_summary(mut self, summary: AuditEventSummary) -> Self {
        self.summary = summary;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum AuditEventKind {
    ToolCallReceived,
    PolicyEvaluated,
    ApprovalRequested,
    ApprovalResolved,
    ToolExecutionStarted,
    ToolExecutionFinished,
    ToolExecutionFailed,
    ToolExecutionDenied,
    ToolExecutionRejected,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AuditEventSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_summary: Option<String>,
}

impl AuditEventSummary {
    pub fn with_result_summary(mut self, value: impl AsRef<str>) -> Self {
        self.result_summary = Some(truncate_summary(value));
        self
    }

    pub fn with_error_summary(mut self, value: impl AsRef<str>) -> Self {
        self.error_summary = Some(truncate_summary(value));
        self
    }

    pub fn with_policy_decision(mut self, value: impl AsRef<str>) -> Self {
        self.policy_decision = Some(truncate_summary(value));
        self
    }

    pub fn with_approval_id(mut self, value: impl AsRef<str>) -> Self {
        self.approval_id = Some(truncate_summary(value));
        self
    }

    pub fn with_approval_decision(mut self, value: impl AsRef<str>) -> Self {
        self.approval_decision = Some(truncate_summary(value));
        self
    }

    pub fn with_started_at(mut self, value: i64) -> Self {
        self.started_at = Some(value);
        self
    }

    pub fn with_finished_at(mut self, value: i64) -> Self {
        self.finished_at = Some(value);
        self
    }

    pub fn with_duration_ms(mut self, value: u64) -> Self {
        self.duration_ms = Some(value);
        self
    }
}

pub fn truncate_summary(value: impl AsRef<str>) -> String {
    truncate_summary_to(value.as_ref(), DEFAULT_SUMMARY_LIMIT)
}

pub fn truncate_summary_to(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...[truncated]");
    truncated
}

pub fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
