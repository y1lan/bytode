use crate::llm::ToolCall;
use crate::tools::ToolDescriptor;

#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub request_id: String,
    pub call: ToolCall,
    pub descriptor: ToolDescriptor,
}
