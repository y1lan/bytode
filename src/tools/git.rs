use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

pub struct GitStatusTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn description(&self) -> &'static str {
        r#"Show working tree status via `git status --porcelain`.

EXAMPLES:
  git_status()                                # full status
  git_status(path="src/")                     # status for a subdirectory

RETURNS: git status --porcelain output, or "clean" if no changes."#
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

    fn timeout_ms(&self) -> u64 { 10_000 }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.args(["status", "--porcelain"]);

        if let Some(path) = args["path"].as_str() {
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
            let mut s = stdout;
            if !stderr.trim().is_empty() {
                s.push_str(&format!("\n--- stderr ---\n{}", stderr));
            }
            s
        };

        Ok(ToolResult::Text {
            source: "git_status".into(),
            content,
            truncated: false,
        })
    }
}

pub struct GitDiffTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn description(&self) -> &'static str {
        r#"Show unstaged diff via `git diff`. Use `staged=true` for staged diff (`git diff --cached`).

EXAMPLES:
  git_diff()                                   # full unstaged diff
  git_diff(path="src/main.rs")                 # diff for one file
  git_diff(staged=true)                         # staged diff only
  git_diff(staged=true, path="Cargo.toml")      # staged diff for one file

RETURNS: unified diff output."#
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

    fn timeout_ms(&self) -> u64 { 15_000 }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.arg("diff");

        if args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false) {
            cmd.arg("--cached");
        }

        if let Some(path) = args["path"].as_str() {
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

pub struct GitLogTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitLogTool {
    fn name(&self) -> &'static str {
        "git_log"
    }

    fn description(&self) -> &'static str {
        r#"Show recent commit history via `git log --oneline`.

EXAMPLES:
  git_log()                                    # last 10 commits
  git_log(count=5)                             # last 5 commits
  git_log(count=20, path="src/")               # last 20 commits in src/
  git_log(path="Cargo.toml")                   # commits touching Cargo.toml

RETURNS: one line per commit (short hash + message)."#
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

    fn timeout_ms(&self) -> u64 { 10_000 }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.args(["log", "--oneline"]);

        let count = args.get("count").and_then(|v| v.as_i64()).unwrap_or(10);
        cmd.arg(format!("-n{}", count.max(1).min(100)));

        if let Some(path) = args["path"].as_str() {
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
