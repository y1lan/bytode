#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCapability {
    ReadProjectFile,
    WriteProjectFile,
    SearchProject,
    ReadDiagnostics,
    RunBuild,
    RunProjectCommand,
    NetworkAccess,
    VcsRead,
    VcsWrite,
    SessionState,
    UnknownExternal,
}
