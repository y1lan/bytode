mod approval;
mod capability;
mod risk;

pub use approval::ApprovalKind;
pub use capability::ToolCapability;
pub use risk::RiskLevel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    ReadOnly,
    Modification,
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderMeta {
    Mcp(McpToolMeta),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolMeta {
    pub server_name: &'static str,
    pub remote_tool_name: &'static str,
    pub transport: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub provider_id: &'static str,
    pub provider_meta: Option<ProviderMeta>,
    pub category: ToolCategory,
    pub capabilities: Vec<ToolCapability>,
    pub default_risk: RiskLevel,
    pub approval: ApprovalKind,
}
