use super::approval::{ApprovalDecision, ApprovalRequest};
use super::policy::{PolicyDecision, ToolPolicyEngine};
use super::{ToolCallContext, ToolCallOutcome, ToolCallRequest, format_args};
use crate::llm::ToolCall;
use crate::session::ToolStatus;
use crate::tools::ToolResult;

pub struct ToolCallEngine {
    policy: ToolPolicyEngine,
}

impl ToolCallEngine {
    pub fn new() -> Self {
        Self {
            policy: ToolPolicyEngine,
        }
    }

    pub async fn execute(
        &self,
        ctx: &ToolCallContext<'_>,
        turn_id: &str,
        call_index: usize,
        call: &ToolCall,
    ) -> ToolCallOutcome {
        let Some(entry) = ctx.registry.get(&call.name) else {
            return denied_outcome(format!("{} is not a registered tool", call.name));
        };

        if !entry.enabled {
            return denied_outcome(format!("{} is disabled", call.name));
        }

        let request = ToolCallRequest {
            request_id: format!("{turn_id}-{call_index}-{}", entry.descriptor.name),
            call: call.clone(),
            descriptor: entry.descriptor.clone(),
        };

        match self.policy.evaluate(ctx, &request) {
            PolicyDecision::Allow => execute_tool(entry.tool.as_ref(), &request).await,
            PolicyDecision::Deny(reason) => denied_outcome(reason),
            PolicyDecision::Ask(reason) => {
                let approval_request = ApprovalRequest {
                    id: request.request_id.clone(),
                    turn_id: turn_id.to_string(),
                    tool_name: request.descriptor.name.to_string(),
                    summary: format!(
                        "{}({})",
                        request.descriptor.name,
                        format_args(request.descriptor.name, &request.call.arguments)
                    ),
                    reason,
                };

                let Some(channel) = ctx.approval_channel.clone() else {
                    return needs_approval_outcome(&approval_request);
                };

                match channel.request_approval(approval_request.clone()).await {
                    ApprovalDecision::Approved => execute_tool(entry.tool.as_ref(), &request).await,
                    ApprovalDecision::Rejected { reason } => {
                        rejected_outcome(&approval_request, &reason)
                    }
                }
            }
        }
    }
}

async fn execute_tool(tool: &dyn crate::tools::Tool, request: &ToolCallRequest) -> ToolCallOutcome {
    let timeout = tokio::time::Duration::from_millis(tool.timeout_ms());
    match tokio::time::timeout(timeout, tool.execute(request.call.arguments.clone())).await {
        Ok(Ok(result)) => {
            let display = tool
                .format_result_for_display(&result)
                .unwrap_or_else(|| "  ok".into());
            ToolCallOutcome {
                result,
                status: ToolStatus::Ok,
                display,
                counts_as_error: false,
            }
        }
        Ok(Err(error)) => ToolCallOutcome {
            result: ToolResult::Text {
                source: "tool_error".into(),
                content: format!("Error: {error}"),
                truncated: false,
            },
            status: ToolStatus::Error,
            display: format!("Error: {error}"),
            counts_as_error: true,
        },
        Err(_) => ToolCallOutcome {
            result: ToolResult::Text {
                source: "tool_error".into(),
                content: format!("Error: {} timed out", request.descriptor.name),
                truncated: false,
            },
            status: ToolStatus::Error,
            display: format!("Error: {} timed out", request.descriptor.name),
            counts_as_error: true,
        },
    }
}

fn denied_outcome(reason: String) -> ToolCallOutcome {
    ToolCallOutcome {
        result: ToolResult::Text {
            source: "tool_denied".into(),
            content: format!("Denied: {reason}"),
            truncated: false,
        },
        status: ToolStatus::Cancelled,
        display: format!("Denied: {reason}"),
        counts_as_error: false,
    }
}

fn needs_approval_outcome(request: &ApprovalRequest) -> ToolCallOutcome {
    ToolCallOutcome {
        result: ToolResult::Text {
            source: "tool_needs_approval".into(),
            content: format!(
                "Approval required for {} but no approval channel is available.",
                request.tool_name
            ),
            truncated: false,
        },
        status: ToolStatus::Cancelled,
        display: format!("Needs approval: {}", request.summary),
        counts_as_error: false,
    }
}

fn rejected_outcome(request: &ApprovalRequest, reason: &str) -> ToolCallOutcome {
    ToolCallOutcome {
        result: ToolResult::Text {
            source: "tool_rejected".into(),
            content: format!("Rejected: {} ({reason})", request.tool_name),
            truncated: false,
        },
        status: ToolStatus::Cancelled,
        display: format!("Rejected: {}", request.summary),
        counts_as_error: false,
    }
}
