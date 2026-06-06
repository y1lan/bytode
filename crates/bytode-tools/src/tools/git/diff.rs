use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Default, Deserialize)]
struct GitDiffInput {
    path: Option<String>,
    staged: Option<bool>,
}

pub struct GitDiffTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitDiffTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_diff",
            description: r#"Show unstaged diff via `git diff`. Use `staged=true` for staged diff (`git diff --cached`).

EXAMPLES:
  git_diff()                                   # full unstaged diff
  git_diff(path="src/main.rs")                 # diff for one file
  git_diff(staged=true)                        # staged diff only
  git_diff(staged=true, path="Cargo.toml")     # staged diff for one file

RETURNS: unified diff output."#,
            provider_id: "builtin",
            category: ToolCategory::ReadOnly,
            capabilities: vec![ToolCapability::VcsRead],
            default_risk: RiskLevel::Medium,
            approval: ApprovalKind::OnRisk,
        }
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Limit diff to this file or directory. Omit for full diff.",
                    "nullable": true
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show staged changes (git diff --cached). Default: false.",
                    "nullable": true
                }
            },
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        15_000
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let input: GitDiffInput = serde_json::from_value(args).map_err(|e| BytodeError::Tool {
            tool: "git_diff".into(),
            message: format!("invalid arguments: {}", e),
        })?;

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.arg("diff");

        if input.staged.unwrap_or(false) {
            cmd.arg("--cached");
        }

        if let Some(path) = input.path {
            cmd.arg("--").arg(path);
        }

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "git_diff".into(),
            message: format!("git diff failed: {}", e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let content = if stdout.trim().is_empty() {
            "no differences".into()
        } else {
            stdout
        };

        Ok(ToolResult::Text {
            source: "git_diff".into(),
            content,
            truncated: false,
        })
    }
}
