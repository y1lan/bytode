use super::approval::{ApprovalDecision, ApprovalRequest};
use super::policy::{PolicyDecision, ToolPolicyEngine};
use super::{ToolCallContext, ToolCallOutcome, ToolCallRequest, format_args};
use crate::agent::audit::event::unix_timestamp;
use crate::agent::audit::{AuditEvent, AuditEventKind, AuditEventSummary, AuditRecorder};
use crate::error::Result;
use crate::llm::ToolCall;
use crate::session::ToolStatus;
use crate::tools::{ToolDescriptor, ToolResult};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct ToolCallEngine {
    policy: ToolPolicyEngine,
    audit: Mutex<AuditRecorder>,
    audit_event_prefix: String,
    audit_event_seq: AtomicU64,
    session_id: String,
}

impl ToolCallEngine {
    pub fn new(session_id: impl Into<String>, session_root: impl AsRef<Path>) -> Result<Self> {
        let session_id = session_id.into();
        Ok(Self {
            policy: ToolPolicyEngine,
            audit: Mutex::new(AuditRecorder::open(&session_id, session_root)?),
            audit_event_prefix: format!("{}-{}", std::process::id(), unix_timestamp_nanos()),
            audit_event_seq: AtomicU64::new(0),
            session_id,
        })
    }

    pub async fn execute(
        &self,
        ctx: &ToolCallContext<'_>,
        turn_id: &str,
        call_index: usize,
        call: &ToolCall,
    ) -> ToolCallOutcome {
        let audit_tool_call_id = audit_tool_call_id(turn_id, call_index, call);
        self.record_audit(
            turn_id,
            &audit_tool_call_id,
            AuditEventKind::ToolCallReceived,
            call_summary(call, None),
        );

        let Some(entry) = ctx.registry.get(&call.name) else {
            let reason = format!("{} is not a registered tool", call.name);
            self.record_audit(
                turn_id,
                &audit_tool_call_id,
                AuditEventKind::PolicyEvaluated,
                call_summary(call, None)
                    .with_policy_decision(format!("deny: {reason}"))
                    .with_error_summary(&reason),
            );
            self.record_audit(
                turn_id,
                &audit_tool_call_id,
                AuditEventKind::ToolExecutionDenied,
                call_summary(call, None).with_error_summary(&reason),
            );
            return denied_outcome(reason);
        };

        if !entry.enabled {
            let reason = format!("{} is disabled", call.name);
            self.record_audit(
                turn_id,
                &audit_tool_call_id,
                AuditEventKind::PolicyEvaluated,
                tool_summary(&entry.descriptor, &call.arguments)
                    .with_policy_decision(format!("deny: {reason}"))
                    .with_error_summary(&reason),
            );
            self.record_audit(
                turn_id,
                &audit_tool_call_id,
                AuditEventKind::ToolExecutionDenied,
                tool_summary(&entry.descriptor, &call.arguments).with_error_summary(&reason),
            );
            return denied_outcome(reason);
        }

        let request = ToolCallRequest {
            request_id: format!("{turn_id}-{call_index}-{}", entry.descriptor.name),
            call: call.clone(),
            descriptor: entry.descriptor.clone(),
        };

        let policy_decision = self.policy.evaluate(ctx, &request);
        self.record_audit(
            turn_id,
            &audit_tool_call_id,
            AuditEventKind::PolicyEvaluated,
            tool_summary(&request.descriptor, &request.call.arguments)
                .with_policy_decision(policy_decision_summary(&policy_decision)),
        );

        match policy_decision {
            PolicyDecision::Allow => {
                self.execute_approved_tool(
                    entry.tool.as_ref(),
                    &request,
                    turn_id,
                    &audit_tool_call_id,
                )
                .await
            }
            PolicyDecision::Deny(reason) => {
                self.record_audit(
                    turn_id,
                    &audit_tool_call_id,
                    AuditEventKind::ToolExecutionDenied,
                    tool_summary(&request.descriptor, &request.call.arguments)
                        .with_error_summary(&reason),
                );
                denied_outcome(reason)
            }
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

                self.record_audit(
                    turn_id,
                    &audit_tool_call_id,
                    AuditEventKind::ApprovalRequested,
                    tool_summary(&request.descriptor, &request.call.arguments)
                        .with_approval_id(&approval_request.id)
                        .with_error_summary(&approval_request.reason),
                );

                let Some(channel) = ctx.approval_channel.clone() else {
                    let reason = "approval channel is unavailable";
                    self.record_audit(
                        turn_id,
                        &audit_tool_call_id,
                        AuditEventKind::ApprovalResolved,
                        tool_summary(&request.descriptor, &request.call.arguments)
                            .with_approval_id(&approval_request.id)
                            .with_approval_decision(format!("rejected: {reason}")),
                    );
                    self.record_audit(
                        turn_id,
                        &audit_tool_call_id,
                        AuditEventKind::ToolExecutionRejected,
                        tool_summary(&request.descriptor, &request.call.arguments)
                            .with_approval_id(&approval_request.id)
                            .with_error_summary(reason),
                    );
                    return needs_approval_outcome(&approval_request);
                };

                match channel.request_approval(approval_request.clone()).await {
                    ApprovalDecision::Approved => {
                        self.record_audit(
                            turn_id,
                            &audit_tool_call_id,
                            AuditEventKind::ApprovalResolved,
                            tool_summary(&request.descriptor, &request.call.arguments)
                                .with_approval_id(&approval_request.id)
                                .with_approval_decision("approved"),
                        );
                        self.execute_approved_tool(
                            entry.tool.as_ref(),
                            &request,
                            turn_id,
                            &audit_tool_call_id,
                        )
                        .await
                    }
                    ApprovalDecision::Rejected { reason } => {
                        self.record_audit(
                            turn_id,
                            &audit_tool_call_id,
                            AuditEventKind::ApprovalResolved,
                            tool_summary(&request.descriptor, &request.call.arguments)
                                .with_approval_id(&approval_request.id)
                                .with_approval_decision(format!("rejected: {reason}")),
                        );
                        self.record_audit(
                            turn_id,
                            &audit_tool_call_id,
                            AuditEventKind::ToolExecutionRejected,
                            tool_summary(&request.descriptor, &request.call.arguments)
                                .with_approval_id(&approval_request.id)
                                .with_error_summary(&reason),
                        );
                        rejected_outcome(&approval_request, &reason)
                    }
                }
            }
        }
    }

    async fn execute_approved_tool(
        &self,
        tool: &dyn crate::tools::Tool,
        request: &ToolCallRequest,
        turn_id: &str,
        audit_tool_call_id: &str,
    ) -> ToolCallOutcome {
        let started_at = unix_timestamp();
        self.record_audit(
            turn_id,
            audit_tool_call_id,
            AuditEventKind::ToolExecutionStarted,
            tool_summary(&request.descriptor, &request.call.arguments).with_started_at(started_at),
        );

        let started = Instant::now();
        let outcome = execute_tool(tool, request).await;
        let finished_at = unix_timestamp();
        let duration_ms = started.elapsed().as_millis() as u64;
        let event_kind = if outcome.status == ToolStatus::Ok {
            AuditEventKind::ToolExecutionFinished
        } else {
            AuditEventKind::ToolExecutionFailed
        };

        let mut summary = tool_summary(&request.descriptor, &request.call.arguments)
            .with_started_at(started_at)
            .with_finished_at(finished_at)
            .with_duration_ms(duration_ms);
        if outcome.status == ToolStatus::Ok {
            summary = summary.with_result_summary(result_summary(&outcome.result));
        } else {
            summary = summary.with_error_summary(result_summary(&outcome.result));
        }

        self.record_audit(turn_id, audit_tool_call_id, event_kind, summary);
        outcome
    }

    fn record_audit(
        &self,
        turn_id: &str,
        tool_call_id: &str,
        kind: AuditEventKind,
        summary: AuditEventSummary,
    ) {
        let event = AuditEvent::new(
            self.next_audit_event_id(),
            &self.session_id,
            turn_id,
            tool_call_id,
            kind,
        )
        .with_summary(summary);

        match self.audit.lock() {
            Ok(mut audit) => {
                if let Err(error) = audit.record(event) {
                    tracing::error!(%error, "tool call audit record failed");
                }
            }
            Err(error) => {
                tracing::error!(%error, "tool call audit mutex poisoned");
            }
        }
    }

    fn next_audit_event_id(&self) -> String {
        let seq = self.audit_event_seq.fetch_add(1, Ordering::Relaxed);
        format!("audit-{}-{seq:08}", self.audit_event_prefix)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentMode;
    use crate::agent::tool_calls::approval::ApprovalChannel;
    use crate::project::{BuildSystem, Language, ProjectProfile};
    use crate::tools::read_file::ReadFileTool;
    use crate::tools::write_file::WriteFileTool;
    use crate::tools::{ToolAvailability, ToolEntry, ToolRegistry};
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn read_file_execution_writes_full_audit_chain() {
        let tmp = TestRoot::new();
        tmp.write("Cargo.toml", "[package]\nname = \"audit-fixture\"\n");
        tmp.write("src/lib.rs", "pub fn answer() -> u8 { 42 }\n");

        let session_root = tmp.path().join(".session");
        let engine = ToolCallEngine::new("session-read", session_root.clone()).unwrap();
        let registry = registry(tmp.path());
        let profile = profile(tmp.path());
        let ctx = context(&registry, &profile, None);

        let outcome = engine
            .execute(
                &ctx,
                "turn-read",
                0,
                &ToolCall {
                    id: "call-read".into(),
                    name: "read_file".into(),
                    arguments: json!({ "path": "src/lib.rs" }),
                },
            )
            .await;

        assert_eq!(outcome.status, ToolStatus::Ok);

        let events = audit_events(&session_root);
        assert_has_subsequence(
            &kinds(&events),
            &[
                AuditEventKind::ToolCallReceived,
                AuditEventKind::PolicyEvaluated,
                AuditEventKind::ToolExecutionStarted,
                AuditEventKind::ToolExecutionFinished,
            ],
        );
        assert!(
            events
                .iter()
                .any(|event| event.kind == AuditEventKind::PolicyEvaluated
                    && event.summary.policy_decision.as_deref() == Some("allow"))
        );
        assert!(events.iter().any(|event| {
            event.kind == AuditEventKind::ToolExecutionFinished
                && event
                    .summary
                    .result_summary
                    .as_deref()
                    .is_some_and(|summary| {
                        summary.contains("file path=") && !summary.contains("answer")
                    })
        }));
    }

    #[tokio::test]
    async fn write_file_rejection_audits_rejection_without_starting_execution() {
        let tmp = TestRoot::new();
        tmp.write("Cargo.toml", "[package]\nname = \"audit-fixture\"\n");

        let session_root = tmp.path().join(".session");
        let engine = ToolCallEngine::new("session-write-reject", session_root.clone()).unwrap();
        let registry = registry(tmp.path());
        let profile = profile(tmp.path());
        let approval = Arc::new(RejectingApproval);
        let ctx = context(&registry, &profile, Some(approval));

        let outcome = engine
            .execute(
                &ctx,
                "turn-write",
                0,
                &ToolCall {
                    id: "call-write".into(),
                    name: "write_file".into(),
                    arguments: json!({
                        "path": "src/lib.rs",
                        "content": "pub fn changed() {}\n"
                    }),
                },
            )
            .await;

        assert_eq!(outcome.status, ToolStatus::Cancelled);
        assert!(!tmp.path().join("src/lib.rs").exists());

        let events = audit_events(&session_root);
        let event_kinds = kinds(&events);
        assert_has_subsequence(
            &event_kinds,
            &[
                AuditEventKind::ToolCallReceived,
                AuditEventKind::PolicyEvaluated,
                AuditEventKind::ApprovalRequested,
                AuditEventKind::ApprovalResolved,
                AuditEventKind::ToolExecutionRejected,
            ],
        );
        assert!(!event_kinds.contains(&AuditEventKind::ToolExecutionStarted));
        assert!(events.iter().any(|event| {
            event.kind == AuditEventKind::ApprovalResolved
                && event
                    .summary
                    .approval_decision
                    .as_deref()
                    .is_some_and(|decision| decision.starts_with("rejected:"))
        }));
    }

    #[tokio::test]
    async fn write_file_approval_audits_approval_and_finished_execution() {
        let tmp = TestRoot::new();
        tmp.write("Cargo.toml", "[package]\nname = \"audit-fixture\"\n");

        let session_root = tmp.path().join(".session");
        let engine = ToolCallEngine::new("session-write-approve", session_root.clone()).unwrap();
        let registry = registry(tmp.path());
        let profile = profile(tmp.path());
        let approval = Arc::new(ApprovingApproval);
        let ctx = context(&registry, &profile, Some(approval));

        let outcome = engine
            .execute(
                &ctx,
                "turn-write",
                0,
                &ToolCall {
                    id: "call-write".into(),
                    name: "write_file".into(),
                    arguments: json!({
                        "path": "src/lib.rs",
                        "content": "pub fn changed() {}\n"
                    }),
                },
            )
            .await;

        assert_eq!(outcome.status, ToolStatus::Ok);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("src/lib.rs")).unwrap(),
            "pub fn changed() {}\n"
        );

        let events = audit_events(&session_root);
        assert_has_subsequence(
            &kinds(&events),
            &[
                AuditEventKind::ToolCallReceived,
                AuditEventKind::PolicyEvaluated,
                AuditEventKind::ApprovalRequested,
                AuditEventKind::ApprovalResolved,
                AuditEventKind::ToolExecutionStarted,
                AuditEventKind::ToolExecutionFinished,
            ],
        );
        assert!(events.iter().any(|event| {
            event.kind == AuditEventKind::ApprovalResolved
                && event.summary.approval_decision.as_deref() == Some("approved")
        }));
        assert!(events.iter().any(|event| {
            event.kind == AuditEventKind::ToolExecutionFinished
                && event
                    .summary
                    .result_summary
                    .as_deref()
                    .is_some_and(|summary| {
                        summary.contains("write path=") && !summary.contains("changed")
                    })
        }));
    }

    #[tokio::test]
    async fn unknown_tool_audits_policy_denial_without_starting_execution() {
        let tmp = TestRoot::new();
        tmp.write("Cargo.toml", "[package]\nname = \"audit-fixture\"\n");

        let session_root = tmp.path().join(".session");
        let engine = ToolCallEngine::new("session-unknown", session_root.clone()).unwrap();
        let registry = registry(tmp.path());
        let profile = profile(tmp.path());
        let ctx = context(&registry, &profile, None);

        let outcome = engine
            .execute(
                &ctx,
                "turn-unknown",
                0,
                &ToolCall {
                    id: "call-unknown".into(),
                    name: "not_a_tool".into(),
                    arguments: json!({}),
                },
            )
            .await;

        assert_eq!(outcome.status, ToolStatus::Cancelled);

        let events = audit_events(&session_root);
        let event_kinds = kinds(&events);
        assert_has_subsequence(
            &event_kinds,
            &[
                AuditEventKind::ToolCallReceived,
                AuditEventKind::PolicyEvaluated,
                AuditEventKind::ToolExecutionDenied,
            ],
        );
        assert!(!event_kinds.contains(&AuditEventKind::ToolExecutionStarted));
        assert!(events.iter().any(|event| {
            event.kind == AuditEventKind::PolicyEvaluated
                && event
                    .summary
                    .policy_decision
                    .as_deref()
                    .is_some_and(|decision| decision.starts_with("deny:"))
        }));
    }

    #[tokio::test]
    async fn audit_log_is_append_only_across_reopening_same_session_root() {
        let tmp = TestRoot::new();
        tmp.write("Cargo.toml", "[package]\nname = \"audit-fixture\"\n");
        tmp.write("src/lib.rs", "pub fn first() {}\n");

        let session_root = tmp.path().join(".session");
        let registry = registry(tmp.path());
        let profile = profile(tmp.path());
        let ctx = context(&registry, &profile, None);

        ToolCallEngine::new("session-append", session_root.clone())
            .unwrap()
            .execute(
                &ctx,
                "turn-one",
                0,
                &ToolCall {
                    id: "call-one".into(),
                    name: "read_file".into(),
                    arguments: json!({ "path": "Cargo.toml" }),
                },
            )
            .await;
        let first_count = audit_events(&session_root).len();
        assert!(first_count > 0);

        ToolCallEngine::new("session-append", session_root.clone())
            .unwrap()
            .execute(
                &ctx,
                "turn-two",
                0,
                &ToolCall {
                    id: "call-two".into(),
                    name: "read_file".into(),
                    arguments: json!({ "path": "src/lib.rs" }),
                },
            )
            .await;

        let all_events = audit_events(&session_root);
        assert!(all_events.len() > first_count);
        assert_has_subsequence(
            &kinds(&all_events[first_count..]),
            &[
                AuditEventKind::ToolCallReceived,
                AuditEventKind::PolicyEvaluated,
                AuditEventKind::ToolExecutionStarted,
                AuditEventKind::ToolExecutionFinished,
            ],
        );
    }

    fn registry(project_root: &Path) -> ToolRegistry {
        let mut registry = ToolRegistry::new(vec![
            ToolEntry::new(
                Box::new(ReadFileTool {
                    project_root: project_root.to_path_buf(),
                }),
                ToolAvailability::Always,
            ),
            ToolEntry::new(
                Box::new(WriteFileTool {
                    project_root: project_root.to_path_buf(),
                    confirm_before_write: true,
                    max_file_size: 1024 * 1024,
                    forbidden_patterns: vec![],
                    lsp: None,
                }),
                ToolAvailability::Always,
            ),
        ]);
        registry.set_enabled("read_file", true);
        registry.set_enabled("write_file", true);
        registry
    }

    fn context<'a>(
        registry: &'a ToolRegistry,
        profile: &'a ProjectProfile,
        approval_channel: Option<Arc<dyn ApprovalChannel>>,
    ) -> ToolCallContext<'a> {
        ToolCallContext {
            registry,
            mode: AgentMode::Normal,
            profile,
            forbidden_write_patterns: &[],
            approval_channel,
        }
    }

    fn profile(root: &Path) -> ProjectProfile {
        ProjectProfile {
            primary: Language::Rust,
            all_languages: vec![Language::Rust],
            build_system: BuildSystem::Cargo,
            test_framework: None,
            root: root.to_path_buf(),
            source_dirs: vec![root.join("src")],
            is_workspace: false,
            workspace_members: vec![],
        }
    }

    fn audit_events(session_root: &Path) -> Vec<AuditEvent> {
        let path = session_root.join("audit.jsonl");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn kinds(events: &[AuditEvent]) -> Vec<AuditEventKind> {
        events.iter().map(|event| event.kind.clone()).collect()
    }

    fn assert_has_subsequence(events: &[AuditEventKind], expected: &[AuditEventKind]) {
        let mut cursor = 0;
        for event in events {
            if expected.get(cursor) == Some(event) {
                cursor += 1;
                if cursor == expected.len() {
                    return;
                }
            }
        }
        panic!("missing event subsequence {expected:?} in {events:?}");
    }

    struct RejectingApproval;

    #[async_trait]
    impl ApprovalChannel for RejectingApproval {
        async fn request_approval(&self, _request: ApprovalRequest) -> ApprovalDecision {
            ApprovalDecision::Rejected {
                reason: "test rejection".into(),
            }
        }
    }

    struct ApprovingApproval;

    #[async_trait]
    impl ApprovalChannel for ApprovingApproval {
        async fn request_approval(&self, _request: ApprovalRequest) -> ApprovalDecision {
            ApprovalDecision::Approved
        }
    }

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bytode-tool-call-audit-{}-{id}",
                std::process::id()
            ));
            if path.exists() {
                std::fs::remove_dir_all(&path).unwrap();
            }
            std::fs::create_dir_all(path.join("src")).unwrap();
            TestRoot { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn audit_tool_call_id(turn_id: &str, call_index: usize, call: &ToolCall) -> String {
    if call.id.is_empty() {
        format!("{turn_id}-{call_index}-{}", call.name)
    } else {
        call.id.clone()
    }
}

fn call_summary(call: &ToolCall, descriptor: Option<&ToolDescriptor>) -> AuditEventSummary {
    let mut summary = AuditEventSummary {
        tool_name: Some(call.name.clone()),
        arguments_summary: Some(arguments_summary(&call.name, &call.arguments)),
        ..AuditEventSummary::default()
    };

    if let Some(descriptor) = descriptor {
        summary.provider_id = Some(descriptor.provider_id.to_string());
        summary.capabilities = descriptor
            .capabilities
            .iter()
            .map(|capability| format!("{capability:?}"))
            .collect();
        summary.risk = Some(format!("{:?}", descriptor.default_risk));
    }

    summary
}

fn tool_summary(descriptor: &ToolDescriptor, arguments: &serde_json::Value) -> AuditEventSummary {
    let call = ToolCall {
        id: String::new(),
        name: descriptor.name.to_string(),
        arguments: arguments.clone(),
    };
    call_summary(&call, Some(descriptor))
}

fn arguments_summary(tool_name: &str, arguments: &serde_json::Value) -> String {
    let formatted = format_args(tool_name, arguments);
    if formatted != "?" || arguments.is_null() {
        return crate::agent::audit::truncate_summary(formatted);
    }

    crate::agent::audit::truncate_summary(serde_json::to_string(arguments).unwrap_or_default())
}

fn policy_decision_summary(decision: &PolicyDecision) -> String {
    match decision {
        PolicyDecision::Allow => "allow".to_string(),
        PolicyDecision::Ask(reason) => format!("ask: {reason}"),
        PolicyDecision::Deny(reason) => format!("deny: {reason}"),
    }
}

fn result_summary(result: &ToolResult) -> String {
    match result {
        ToolResult::FileContent {
            path,
            line_count,
            total_bytes,
            ..
        } => format!("file path={path}, lines={line_count}, bytes={total_bytes}"),
        ToolResult::Diagnostics {
            tool,
            total,
            errors,
            warnings,
            ..
        } => {
            format!("diagnostics tool={tool}, total={total}, errors={errors}, warnings={warnings}")
        }
        ToolResult::Json {
            tool,
            filter,
            count,
            ..
        } => match filter {
            Some(filter) => format!("json tool={tool}, filter={filter}, count={count}"),
            None => format!("json tool={tool}, count={count}"),
        },
        ToolResult::Matches {
            pattern,
            count,
            truncated,
            ..
        } => format!("matches pattern={pattern:?}, count={count}, truncated={truncated}"),
        ToolResult::WriteConfirmation {
            path,
            bytes_written,
            lines,
            ..
        } => format!("write path={path}, bytes={bytes_written}, lines={lines}"),
        ToolResult::Text {
            source,
            content,
            truncated,
        } => format!(
            "text source={source}, truncated={truncated}, preview={}",
            crate::agent::audit::truncate_summary(content)
        ),
    }
}

fn unix_timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
