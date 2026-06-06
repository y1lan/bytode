use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Default, Deserialize)]
struct GitStatusInput {
    path: Option<String>,
}

pub struct GitStatusTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitStatusTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_status",
            description: r#"Show working tree status via `git status --porcelain`.

EXAMPLES:
  git_status()                                # full status
  git_status(path="src/")                     # status for a subdirectory

RETURNS: git status --porcelain output, or "clean" if no changes."#,
            provider_id: "builtin",
            provider_meta: None,
            category: ToolCategory::ReadOnly,
            capabilities: vec![ToolCapability::VcsRead],
            default_risk: RiskLevel::Low,
            approval: ApprovalKind::Never,
        }
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Limit status to this subdirectory. Omit for full status.",
                    "nullable": true
                }
            },
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        10_000
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let input: GitStatusInput =
            serde_json::from_value(args).map_err(|e| BytodeError::Tool {
                tool: "git_status".into(),
                message: format!("invalid arguments: {}", e),
            })?;

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.args(["status", "--porcelain"]);

        if let Some(path) = input.path {
            cmd.arg("--").arg(path);
        }

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "git_status".into(),
            message: format!("git status failed: {}", e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let content = if stdout.trim().is_empty() {
            "clean".into()
        } else {
            let mut text = stdout;
            if !stderr.trim().is_empty() {
                text.push_str(&format!("\n--- stderr ---\n{}", stderr));
            }
            text
        };

        Ok(ToolResult::Text {
            source: "git_status".into(),
            content,
            truncated: false,
        })
    }
}
