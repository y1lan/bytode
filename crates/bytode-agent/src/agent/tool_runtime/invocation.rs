use super::ToolTimeout;
use crate::tools::ToolDescriptor;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub execution_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub provider_id: String,
    pub arguments: Value,
    pub descriptor: ToolDescriptor,
    pub working_dir: PathBuf,
    pub timeout: ToolTimeout,
}
