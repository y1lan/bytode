use crate::tools::ToolResult;
use std::time::Duration;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionStatus {
    Success,
    Failed,
    TimedOut,
    Denied,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub execution_id: String,
    pub status: ToolExecutionStatus,
    pub output: Option<ToolResult>,
    pub error: Option<String>,
    pub duration: Duration,
    pub artifacts: Vec<String>,
    pub effects: Vec<String>,
}

impl ToolExecutionResult {
    pub fn success(execution_id: String, output: ToolResult, duration: Duration) -> Self {
        Self {
            execution_id,
            status: ToolExecutionStatus::Success,
            output: Some(output),
            error: None,
            duration,
            artifacts: Vec::new(),
            effects: Vec::new(),
        }
    }

    pub fn failed(execution_id: String, error: impl Into<String>, duration: Duration) -> Self {
        Self {
            execution_id,
            status: ToolExecutionStatus::Failed,
            output: None,
            error: Some(error.into()),
            duration,
            artifacts: Vec::new(),
            effects: Vec::new(),
        }
    }

    pub fn timed_out(execution_id: String, error: impl Into<String>, duration: Duration) -> Self {
        Self {
            execution_id,
            status: ToolExecutionStatus::TimedOut,
            output: None,
            error: Some(error.into()),
            duration,
            artifacts: Vec::new(),
            effects: Vec::new(),
        }
    }
}
