use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct GitCommitInput {
    message: String,
    files: Option<Vec<String>>,
}

pub struct GitCommitTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitCommitTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_commit",
            description: r#"Stage all changes and create a commit. Runs `git add -A && git commit -m <message>`.

SAFETY: Only operates within project root. Commit message is required.
Returns the commit hash on success.

EXAMPLES:
  git_commit(message="fix: resolve clippy warnings")   # stage all + commit
  git_commit(message="feat: add login", files=["src/auth.rs", "tests/auth.rs"])  # commit specific files

RETURNS: commit summary (hash + message) or error if nothing to commit."#,
            provider_id: "builtin",
            provider_meta: None,
            category: ToolCategory::Modification,
            capabilities: vec![ToolCapability::VcsWrite],
            default_risk: RiskLevel::High,
            approval: ApprovalKind::Always,
        }
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Commit message. Follow conventional commits (feat:/fix:/docs:/refactor:)."
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Specific files to commit. Omit to stage all changes (git add -A).",
                    "nullable": true
                }
            },
            "required": ["message"],
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        30_000
    }

    fn format_result_for_display(&self, result: &ToolResult) -> Option<String> {
        if let ToolResult::Text { content, .. } = result {
            let short = content.lines().next().unwrap_or(content);
            Some(format!("  {}", short))
        } else {
            None
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let input: GitCommitInput =
            serde_json::from_value(args).map_err(|e| BytodeError::Tool {
                tool: "git_commit".into(),
                message: format!("invalid arguments: {}", e),
            })?;

        if input.message.trim().is_empty() {
            return Err(BytodeError::Tool {
                tool: "git_commit".into(),
                message: "commit message cannot be empty".into(),
            });
        }

        let mut add = Command::new("git");
        add.current_dir(&self.project_root);
        add.arg("add");
        if let Some(files) = input.files {
            for file in files {
                add.arg(file);
            }
        } else {
            add.arg("-A");
        }

        let add_out = add.output().map_err(|e| BytodeError::Tool {
            tool: "git_commit".into(),
            message: format!("git add failed: {}", e),
        })?;

        if !add_out.status.success() {
            let stderr = String::from_utf8_lossy(&add_out.stderr);
            return Err(BytodeError::Tool {
                tool: "git_commit".into(),
                message: format!("git add failed: {}", stderr.trim()),
            });
        }

        let mut commit = Command::new("git");
        commit.current_dir(&self.project_root);
        commit.args(["commit", "-m", &input.message]);

        let output = commit.output().map_err(|e| BytodeError::Tool {
            tool: "git_commit".into(),
            message: format!("git commit failed: {}", e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            let detail = if stderr.trim().is_empty() {
                &stdout
            } else {
                &stderr
            };
            return Err(BytodeError::Tool {
                tool: "git_commit".into(),
                message: format!("commit failed: {}", detail.trim()),
            });
        }

        let content = if stdout.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };

        Ok(ToolResult::Text {
            source: "git_commit".into(),
            content: if content.is_empty() {
                "committed successfully".into()
            } else {
                content
            },
            truncated: false,
        })
    }
}
