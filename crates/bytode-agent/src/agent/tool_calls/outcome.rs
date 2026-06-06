use crate::session::ToolStatus;
use crate::tools::ToolResult;

#[derive(Debug, Clone)]
pub struct ToolCallOutcome {
    pub result: ToolResult,
    pub status: ToolStatus,
    pub display: String,
    pub counts_as_error: bool,
}
