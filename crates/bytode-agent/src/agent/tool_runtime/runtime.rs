use super::error::ToolRuntimeError;
use super::{ToolExecutionResult, ToolInvocation};
use crate::tools::Tool;
use std::time::Instant;

#[derive(Debug, Default, Clone, Copy)]
pub struct ToolRuntime;

impl ToolRuntime {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(
        &self,
        tool: &dyn Tool,
        invocation: ToolInvocation,
    ) -> ToolExecutionResult {
        let started = Instant::now();
        let execution_id = invocation.execution_id.clone();
        let timeout = invocation.timeout.duration();

        tracing::debug!(
            execution_id = %invocation.execution_id,
            session_id = %invocation.session_id,
            turn_id = %invocation.turn_id,
            tool_call_id = %invocation.tool_call_id,
            tool_name = %invocation.tool_name,
            provider_id = %invocation.provider_id,
            descriptor_name = %invocation.descriptor.name,
            working_dir = %invocation.working_dir.display(),
            timeout_ms = timeout.as_millis(),
            "tool runtime execution started"
        );

        match tokio::time::timeout(timeout, tool.execute(invocation.arguments)).await {
            Ok(Ok(output)) => ToolExecutionResult::success(execution_id, output, started.elapsed()),
            Ok(Err(error)) => ToolExecutionResult::failed(
                execution_id,
                ToolRuntimeError::tool_failed(error).message(),
                started.elapsed(),
            ),
            Err(_) => ToolExecutionResult::timed_out(
                execution_id,
                ToolRuntimeError::timed_out(invocation.tool_name).message(),
                started.elapsed(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{BytodeError, Result};
    use crate::tools::read_file::ReadFileTool;
    use crate::tools::write_file::WriteFileTool;
    use crate::tools::{
        ApprovalKind, RiskLevel, Tool, ToolCapability, ToolCategory, ToolDescriptor, ToolResult,
    };
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[derive(Clone, Copy)]
    enum MockBehavior {
        Succeed,
        Fail,
        Sleep(Duration),
    }

    struct MockTool {
        behavior: MockBehavior,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn descriptor(&self) -> ToolDescriptor {
            descriptor()
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn execute(&self, _args: Value) -> Result<ToolResult> {
            match self.behavior {
                MockBehavior::Succeed => Ok(ToolResult::Text {
                    source: "mock".into(),
                    content: "done".into(),
                    truncated: false,
                }),
                MockBehavior::Fail => Err(BytodeError::Tool {
                    tool: "mock_tool".into(),
                    message: "bad input".into(),
                }),
                MockBehavior::Sleep(duration) => {
                    tokio::time::sleep(duration).await;
                    Ok(ToolResult::Text {
                        source: "mock".into(),
                        content: "late".into(),
                        truncated: false,
                    })
                }
            }
        }
    }

    fn descriptor() -> ToolDescriptor {
        ToolDescriptor {
            name: "mock_tool",
            description: "mock tool",
            provider_id: "test",
            provider_meta: None,
            category: ToolCategory::ReadOnly,
            capabilities: vec![ToolCapability::ReadProjectFile],
            default_risk: RiskLevel::Low,
            approval: ApprovalKind::Never,
        }
    }

    fn mock_invocation(timeout: Duration) -> ToolInvocation {
        invocation(
            "exec-1",
            "mock_tool",
            "test",
            descriptor(),
            json!({"value": 1}),
            PathBuf::from("/tmp"),
            timeout,
        )
    }

    fn invocation(
        execution_id: &str,
        tool_name: &str,
        provider_id: &str,
        descriptor: ToolDescriptor,
        arguments: Value,
        working_dir: PathBuf,
        timeout: Duration,
    ) -> ToolInvocation {
        ToolInvocation {
            execution_id: execution_id.into(),
            session_id: "session-1".into(),
            turn_id: "turn-1".into(),
            tool_call_id: "call-1".into(),
            tool_name: tool_name.into(),
            provider_id: provider_id.into(),
            arguments,
            descriptor,
            working_dir,
            timeout: timeout.into(),
        }
    }

    #[tokio::test]
    async fn execute_returns_success_result() {
        let runtime = ToolRuntime::new();
        let tool = MockTool {
            behavior: MockBehavior::Succeed,
        };

        let result = runtime
            .execute(&tool, mock_invocation(Duration::from_secs(1)))
            .await;

        assert_eq!(result.execution_id, "exec-1");
        assert_eq!(result.status, super::super::ToolExecutionStatus::Success);
        assert_eq!(result.error, None);
        assert!(result.duration <= Duration::from_secs(1));
        assert!(result.artifacts.is_empty());
        assert!(result.effects.is_empty());

        let Some(ToolResult::Text {
            source,
            content,
            truncated,
        }) = result.output
        else {
            panic!("expected text output");
        };
        assert_eq!(source, "mock");
        assert_eq!(content, "done");
        assert!(!truncated);
    }

    #[tokio::test]
    async fn execute_normalizes_tool_failures() {
        let runtime = ToolRuntime::new();
        let tool = MockTool {
            behavior: MockBehavior::Fail,
        };

        let result = runtime
            .execute(&tool, mock_invocation(Duration::from_secs(1)))
            .await;

        assert_eq!(result.execution_id, "exec-1");
        assert_eq!(result.status, super::super::ToolExecutionStatus::Failed);
        assert!(result.output.is_none());
        assert_eq!(result.error.as_deref(), Some("tool 'mock_tool': bad input"));
        assert!(result.duration <= Duration::from_secs(1));
        assert!(result.artifacts.is_empty());
        assert!(result.effects.is_empty());
    }

    #[tokio::test]
    async fn execute_returns_timed_out_result() {
        let runtime = ToolRuntime::new();
        let tool = MockTool {
            behavior: MockBehavior::Sleep(Duration::from_secs(10)),
        };

        let result = runtime
            .execute(&tool, mock_invocation(Duration::from_millis(10)))
            .await;

        assert_eq!(result.execution_id, "exec-1");
        assert_eq!(result.status, super::super::ToolExecutionStatus::TimedOut);
        assert!(result.output.is_none());
        assert_eq!(result.error.as_deref(), Some("tool 'mock_tool' timed out"));
        assert!(result.duration < Duration::from_secs(1));
        assert!(result.artifacts.is_empty());
        assert!(result.effects.is_empty());
    }

    #[tokio::test]
    async fn execute_runs_read_file_tool() {
        let tmp = TestRoot::new();
        tmp.write("src/lib.rs", "pub fn answer() -> u8 { 42 }\n");

        let runtime = ToolRuntime::new();
        let tool = ReadFileTool {
            project_root: tmp.path(),
        };
        let descriptor = tool.descriptor();

        let result = runtime
            .execute(
                &tool,
                invocation(
                    "exec-read",
                    descriptor.name,
                    descriptor.provider_id,
                    descriptor,
                    json!({ "path": "src/lib.rs" }),
                    tmp.path(),
                    Duration::from_secs(1),
                ),
            )
            .await;

        assert_eq!(result.execution_id, "exec-read");
        assert_eq!(result.status, super::super::ToolExecutionStatus::Success);
        let Some(ToolResult::FileContent {
            path,
            content,
            line_count,
            ..
        }) = result.output
        else {
            panic!("expected file content output");
        };
        assert!(path.ends_with("src/lib.rs"));
        assert!(content.contains("answer"));
        assert_eq!(line_count, 1);
    }

    #[tokio::test]
    async fn execute_runs_write_file_tool() {
        let tmp = TestRoot::new();

        let runtime = ToolRuntime::new();
        let tool = WriteFileTool {
            project_root: tmp.path(),
            confirm_before_write: true,
            max_file_size: 1024 * 1024,
            forbidden_patterns: vec![],
            lsp: None,
        };
        let descriptor = tool.descriptor();

        let result = runtime
            .execute(
                &tool,
                invocation(
                    "exec-write",
                    descriptor.name,
                    descriptor.provider_id,
                    descriptor,
                    json!({
                        "path": "src/lib.rs",
                        "content": "pub fn changed() {}\n"
                    }),
                    tmp.path(),
                    Duration::from_secs(1),
                ),
            )
            .await;

        assert_eq!(result.execution_id, "exec-write");
        assert_eq!(result.status, super::super::ToolExecutionStatus::Success);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("src/lib.rs")).unwrap(),
            "pub fn changed() {}\n"
        );
        let Some(ToolResult::WriteConfirmation {
            path,
            bytes_written,
            lines,
            ..
        }) = result.output
        else {
            panic!("expected write confirmation output");
        };
        assert!(path.ends_with("src/lib.rs"));
        assert_eq!(bytes_written, "pub fn changed() {}\n".len());
        assert_eq!(lines, 1);
    }

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("bytode-tool-runtime-{}-{id}", std::process::id()));
            if path.exists() {
                std::fs::remove_dir_all(&path).unwrap();
            }
            std::fs::create_dir_all(path.join("src")).unwrap();
            Self { path }
        }

        fn path(&self) -> PathBuf {
            self.path.clone()
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
