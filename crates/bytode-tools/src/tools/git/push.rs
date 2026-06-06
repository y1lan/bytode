use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Default, Deserialize)]
struct GitPushInput {
    remote: Option<String>,
    branch: Option<String>,
    force: Option<bool>,
    set_upstream: Option<bool>,
}

pub struct GitPushTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitPushTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "git_push",
            description: r#"Push commits to the remote repository. Runs `git push [remote] [branch]`.

SAFETY: Force push requires explicit `force=true`. Defaults to `origin` and current branch.

EXAMPLES:
  git_push()                               # push current branch to origin
  git_push(remote="origin", branch="main") # push to origin/main
  git_push(branch="feature-x")             # push to origin/feature-x
  git_push(force=true)                     # force push (use with caution!)

RETURNS: push output from git."#,
            provider_id: "builtin",
            category: ToolCategory::Modification,
            capabilities: vec![ToolCapability::VcsWrite, ToolCapability::NetworkAccess],
            default_risk: RiskLevel::Critical,
            approval: ApprovalKind::Always,
        }
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "remote": {
                    "type": "string",
                    "description": "Remote name. Default: 'origin'.",
                    "nullable": true
                },
                "branch": {
                    "type": "string",
                    "description": "Branch name. Default: current branch.",
                    "nullable": true
                },
                "force": {
                    "type": "boolean",
                    "description": "Force push (--force). Default: false. Use with caution.",
                    "nullable": true
                },
                "set_upstream": {
                    "type": "boolean",
                    "description": "Set upstream tracking (-u). Default: false.",
                    "nullable": true
                }
            },
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        60_000
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
        let input: GitPushInput = serde_json::from_value(args).map_err(|e| BytodeError::Tool {
            tool: "git_push".into(),
            message: format!("invalid arguments: {}", e),
        })?;

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.arg("push");

        if input.force.unwrap_or(false) {
            cmd.arg("--force");
        }

        if input.set_upstream.unwrap_or(false) {
            cmd.arg("-u");
        }

        cmd.arg(input.remote.as_deref().unwrap_or("origin"));
        if let Some(branch) = input.branch {
            cmd.arg(branch);
        }

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "git_push".into(),
            message: format!("git push failed: {}", e),
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
                tool: "git_push".into(),
                message: format!("push failed: {}", detail.trim()),
            });
        }

        let content = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else if !stdout.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            "push completed".into()
        };

        Ok(ToolResult::Text {
            source: "git_push".into(),
            content,
            truncated: false,
        })
    }
}
