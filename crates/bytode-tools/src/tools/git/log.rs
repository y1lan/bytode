use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Default, Deserialize)]
struct GitLogInput {
    count: Option<i64>,
    path: Option<String>,
}

pub struct GitLogTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitLogTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_log",
            description: r#"Show recent commit history via `git log --oneline`.

EXAMPLES:
  git_log()                                    # last 10 commits
  git_log(count=5)                             # last 5 commits
  git_log(count=20, path="src/")               # last 20 commits in src/
  git_log(path="Cargo.toml")                   # commits touching Cargo.toml

RETURNS: one line per commit (short hash + message)."#,
            provider_id: "builtin",
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
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (default: 10, max: 100).",
                    "minimum": 1,
                    "maximum": 100,
                    "nullable": true
                },
                "path": {
                    "type": "string",
                    "description": "Show commits touching this file or directory. Omit for all.",
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
        let input: GitLogInput = serde_json::from_value(args).map_err(|e| BytodeError::Tool {
            tool: "git_log".into(),
            message: format!("invalid arguments: {}", e),
        })?;

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.args(["log", "--oneline"]);
        cmd.arg(format!("-n{}", input.count.unwrap_or(10).clamp(1, 100)));

        if let Some(path) = input.path {
            cmd.arg("--").arg(path);
        }

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "git_log".into(),
            message: format!("git log failed: {}", e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let content = if stdout.trim().is_empty() {
            "no commits found".into()
        } else {
            stdout
        };

        Ok(ToolResult::Text {
            source: "git_log".into(),
            content,
            truncated: false,
        })
    }
}
