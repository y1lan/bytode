use crate::error::BytodeError;

#[derive(Debug, Clone)]
pub enum ToolRuntimeError {
    ToolFailed(String),
    TimedOut { tool_name: String },
}

impl ToolRuntimeError {
    pub fn tool_failed(error: BytodeError) -> Self {
        Self::ToolFailed(error.to_string())
    }

    pub fn timed_out(tool_name: impl Into<String>) -> Self {
        Self::TimedOut {
            tool_name: tool_name.into(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::ToolFailed(error) => error.clone(),
            Self::TimedOut { tool_name } => format!("tool '{tool_name}' timed out"),
        }
    }
}
