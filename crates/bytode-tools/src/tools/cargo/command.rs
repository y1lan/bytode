use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolAvailability, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor, ToolEntry};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

const WHITELISTED_SUBCOMMANDS: &[&str] = &[
    "check", "build", "test", "clippy", "fmt", "doc", "bench", "run", "clean", "update",
];

#[derive(Debug, Deserialize)]
struct CargoInput {
    cmd: String,
    #[serde(default)]
    args: Vec<String>,
}

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
  cargo(cmd="check")                           # cargo check
  cargo(cmd="check", args=["--all-targets"])  # cargo check --all-targets
  cargo(cmd="test")                            # cargo test
  cargo(cmd="test", args=["-p", "mypackage"]) # cargo test -p mypackage
  cargo(cmd="build")                           # cargo build
  cargo(cmd="build", args=["--release"])      # cargo build --release

RETURNS: stdout + stderr combined. Exit code is reported if non-zero."#,
            provider_id: "builtin",
            category: ToolCategory::Build,
            capabilities: vec![ToolCapability::RunBuild, ToolCapability::RunProjectCommand],
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
        let input: CargoInput = serde_json::from_value(args).map_err(|e| BytodeError::Tool {
            tool: "cargo".into(),
            message: format!("invalid arguments: {}", e),
        })?;

        let whitelist: HashSet<&str> = WHITELISTED_SUBCOMMANDS.iter().copied().collect();
        if !whitelist.contains(input.cmd.as_str()) {
            return Err(BytodeError::Tool {
                tool: "cargo".into(),
                message: format!(
                    "subcommand '{}' is not whitelisted. Allowed: {}",
                    input.cmd,
                    WHITELISTED_SUBCOMMANDS.join(", ")
                ),
            });
        }

        let mut cmd = Command::new("cargo");
        cmd.current_dir(&self.project_root);
        cmd.arg(&input.cmd);
        cmd.args(&input.args);

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "cargo".into(),
            message: format!("cargo {} failed to start: {}", input.cmd, e),
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
            content = format!("cargo {} completed (no output)", input.cmd);
        }

        if !output.status.success() {
            content.push_str(&format!(
                "\n[cargo exited with code {}]",
                output.status.code().unwrap_or(-1)
            ));
        }

        Ok(ToolResult::Text {
            source: format!("cargo_{}", input.cmd),
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
