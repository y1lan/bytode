use crate::tools::ToolCapability;

pub const PLAN_MODE_DENIED_CAPABILITIES: &[ToolCapability] = &[
    ToolCapability::WriteProjectFile,
    ToolCapability::RunBuild,
    ToolCapability::RunProjectCommand,
    ToolCapability::NetworkAccess,
    ToolCapability::VcsWrite,
    ToolCapability::UnknownExternal,
];
