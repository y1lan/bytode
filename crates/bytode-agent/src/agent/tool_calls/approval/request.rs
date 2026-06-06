#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: String,
    pub turn_id: String,
    pub tool_name: String,
    pub summary: String,
    pub reason: String,
}
