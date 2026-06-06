use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolAvailability, ToolResult};
use crate::{
    ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor, ToolEntry,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

const WHITELISTED_SUBCOMMANDS: &[&str] = &[
    "check", "build", "test", "clippy", "fmt", "doc", "bench", "run", "clean", "update",
];

pub struct CargoTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for CargoTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "cargo",
            description: r#"Run a cargo subcommand in the project root. Only whitelisted subcommands are allowed.

ALLOWED: check, build, test, clippy, fmt, doc, bench, run, clean, update

EXAMPLES:
  run_cargo(cmd="check")                          # cargo check
  run_cargo(cmd="check", args=["--all-targets"])  # cargo check --all-targets
  run_cargo(cmd="test")                           # cargo test
  run_cargo(cmd="test", args=["-p", "mypackage"])  # cargo test -p mypackage
  run_cargo(cmd="build")                          # cargo build
  run_cargo(cmd="build", args=["--release"])       # cargo build --release

RETURNS: stdout + stderr combined. Exit code is reported if non-zero."#,
            provider_id: "builtin",
            category: ToolCategory::Build,
            capabilities: vec![
                ToolCapability::RunBuild,
                ToolCapability::RunProjectCommand,
            ],
            default_risk: RiskLevel::High,
            approval: ApprovalKind::OnRisk,
        }
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "Cargo subcommand. Must be one of: check, build, test, clippy, fmt, doc, bench, run, clean, update."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional arguments to pass to cargo. Omit for defaults.",
                    "nullable": true
                }
            },
            "required": ["cmd"],
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        120_000
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let cmd_name = args["cmd"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "cargo".into(),
            message: "missing 'cmd' argument".into(),
        })?;

        let whitelist: HashSet<&str> = WHITELISTED_SUBCOMMANDS.iter().copied().collect();
        if !whitelist.contains(cmd_name) {
            return Err(BytodeError::Tool {
                tool: "cargo".into(),
                message: format!(
                    "subcommand '{}' is not whitelisted. Allowed: {}",
                    cmd_name,
                    WHITELISTED_SUBCOMMANDS.join(", ")
                ),
            });
        }

        let mut cmd = Command::new("cargo");
        cmd.current_dir(&self.project_root);
        cmd.arg(cmd_name);

        if let Some(extra_args) = args["args"].as_array() {
            for a in extra_args {
                if let Some(s) = a.as_str() {
                    cmd.arg(s);
                }
            }
        }

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "cargo".into(),
            message: format!("cargo {} failed to start: {}", cmd_name, e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let mut content = stdout.trim().to_string();
        if !stderr.trim().is_empty() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&stderr);
        }

        if content.trim().is_empty() {
            content = format!("cargo {} completed (no output)", cmd_name);
        }

        if !output.status.success() {
            content.push_str(&format!(
                "\n[cargo exited with code {}]",
                output.status.code().unwrap_or(-1)
            ));
        }

        Ok(ToolResult::Text {
            source: format!("cargo_{}", cmd_name),
            content,
            truncated: false,
        })
    }
}

impl CargoTool {
    pub fn entry(project_root: PathBuf) -> ToolEntry {
        ToolEntry::new(
            Box::new(CargoTool { project_root }),
            ToolAvailability::PrimaryLanguage {
                requires: &["rust"],
            },
        )
    }
}
