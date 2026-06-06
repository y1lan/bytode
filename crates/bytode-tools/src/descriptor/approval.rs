#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovalKind {
    Never,
    OnRisk,
    Always,
}
