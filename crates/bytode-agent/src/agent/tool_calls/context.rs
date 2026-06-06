use super::approval::ApprovalChannel;
use crate::agent::AgentMode;
use crate::project::ProjectProfile;
use crate::tools::ToolRegistry;
use std::sync::Arc;

pub struct ToolCallContext<'a> {
    pub registry: &'a ToolRegistry,
    pub mode: AgentMode,
    pub profile: &'a ProjectProfile,
    pub forbidden_write_patterns: &'a [String],
    pub approval_channel: Option<Arc<dyn ApprovalChannel>>,
}
