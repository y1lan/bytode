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

// ── git_commit ─────────────────────────────────────────────────────────

pub struct GitCommitTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &'static str {
        "git_commit"
    }

    fn description(&self) -> &'static str {
        r#"Stage all changes and create a commit. Runs `git add -A && git commit -m <message>`.

SAFETY: Only operates within project root. Commit message is required.
Returns the commit hash on success.

EXAMPLES:
  git_commit(message="fix: resolve clippy warnings")    # stage all + commit
  git_commit(message="feat: add login", files=["src/auth.rs", "tests/auth.rs"])  # commit specific files

RETURNS: commit summary (hash + message) or error if nothing to commit."#
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

    fn requires_approval(&self) -> bool {
        true
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
        let message = args["message"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "git_commit".into(),
            message: "missing 'message' argument".into(),
        })?;

        if message.trim().is_empty() {
            return Err(BytodeError::Tool {
                tool: "git_commit".into(),
                message: "commit message cannot be empty".into(),
            });
        }

        // Stage files
        let mut add = Command::new("git");
        add.current_dir(&self.project_root);
        add.arg("add");
        if let Some(files) = args["files"].as_array() {
            for f in files {
                if let Some(s) = f.as_str() {
                    add.arg(s);
                }
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

        // Commit
        let mut commit = Command::new("git");
        commit.current_dir(&self.project_root);
        commit.args(["commit", "-m", message]);

        let output = commit.output().map_err(|e| BytodeError::Tool {
            tool: "git_commit".into(),
            message: format!("git commit failed: {}", e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            let detail = if stderr.trim().is_empty() { &stdout } else { &stderr };
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
            content: if content.is_empty() { "committed successfully".into() } else { content },
            truncated: false,
        })
    }
}

// ── git_push ───────────────────────────────────────────────────────────

pub struct GitPushTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for GitPushTool {
    fn name(&self) -> &'static str {
        "git_push"
    }

    fn description(&self) -> &'static str {
        r#"Push commits to the remote repository. Runs `git push [remote] [branch]`.

SAFETY: Force push requires explicit `force=true`. Defaults to `origin` and current branch.

EXAMPLES:
  git_push()                                    # push current branch to origin
  git_push(remote="origin", branch="main")      # push to origin/main
  git_push(branch="feature-x")                  # push to origin/feature-x
  git_push(force=true)                          # force push (use with caution!)

RETURNS: push output from git."#
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

    fn requires_approval(&self) -> bool {
        true
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
        let remote = args["remote"].as_str().unwrap_or("origin");
        let branch = args["branch"].as_str();

        let mut cmd = Command::new("git");
        cmd.current_dir(&self.project_root);
        cmd.arg("push");

        if args.get("force").and_then(|v| v.as_bool()).unwrap_or(false) {
            cmd.arg("--force");
        }

        if args.get("set_upstream").and_then(|v| v.as_bool()).unwrap_or(false) {
            cmd.arg("-u");
        }

        cmd.arg(remote);
        if let Some(b) = branch {
            cmd.arg(b);
        }

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "git_push".into(),
            message: format!("git push failed: {}", e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            let detail = if stderr.trim().is_empty() { &stdout } else { &stderr };
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
