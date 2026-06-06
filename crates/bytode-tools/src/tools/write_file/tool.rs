use super::input::WriteFileInput;
use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub struct WriteFileTool {
    pub project_root: PathBuf,
    pub confirm_before_write: bool,
    pub max_file_size: u64,
    pub forbidden_patterns: Vec<String>,
    pub lsp: Option<Arc<crate::lsp::LspClient>>,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write_file",
            description: r#"Write content to a file. Creates or overwrites. Returns a diff of changes.

WHEN TO USE: After diagnosing and deciding on a fix. After reading the current content.
WHEN NOT TO USE: For reading — use read_file. For searching — use search_code.

SAFETY: Writes are atomic (tmp file + rename). Path must be within project root.
RETURNS: { "type": "write_confirmation", path, bytes_written, lines, diff }"#,
            provider_id: "builtin",
            category: ToolCategory::Modification,
            capabilities: vec![ToolCapability::WriteProjectFile],
            default_risk: RiskLevel::High,
            approval: ApprovalKind::Always,
        }
    }

    fn parameters_schema(&self) -> Value {
        WriteFileInput::schema()
    }

    fn timeout_ms(&self) -> u64 {
        10_000
    }

    fn format_result_for_display(&self, result: &ToolResult) -> Option<String> {
        if let ToolResult::WriteConfirmation {
            path,
            diff,
            bytes_written,
            lines,
        } = result
        {
            let short = path.replace(&std::env::var("HOME").unwrap_or_default(), "~");
            if diff.starts_with("new file:") {
                Some(format!("  {} ({}, {} lines)", diff, bytes_written, lines))
            } else {
                let changed = diff
                    .lines()
                    .filter(|line| line.starts_with('-') || line.starts_with('+'))
                    .take(40)
                    .collect::<Vec<_>>();
                let mut summary = format!("  {} ({} bytes, {} lines)\n", short, bytes_written, lines);
                for line in &changed {
                    summary.push_str(&format!("  {}\n", line));
                }
                if diff
                    .lines()
                    .filter(|line| line.starts_with('-') || line.starts_with('+'))
                    .count()
                    > 40
                {
                    summary.push_str("  ...\n");
                }
                Some(summary)
            }
        } else {
            None
        }
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let input = WriteFileInput::from_value(args)?;
        let resolved = if PathBuf::from(&input.path).is_absolute() {
            PathBuf::from(&input.path)
        } else {
            self.project_root.join(&input.path)
        };

        let canonical = if resolved.exists() {
            std::fs::canonicalize(&resolved)?
        } else {
            let parent = resolved.parent().ok_or_else(|| BytodeError::Tool {
                tool: "write_file".into(),
                message: "no parent directory".into(),
            })?;
            std::fs::canonicalize(parent)?.join(resolved.file_name().unwrap())
        };

        if !canonical.starts_with(&self.project_root) {
            return Err(BytodeError::Tool {
                tool: "write_file".into(),
                message: "path outside project root".into(),
            });
        }

        if resolved.exists() {
            let metadata = std::fs::metadata(&resolved)?;
            if metadata.len() > self.max_file_size {
                return Err(BytodeError::Tool {
                    tool: "write_file".into(),
                    message: format!(
                        "file exceeds max size ({} > {})",
                        metadata.len(),
                        self.max_file_size
                    ),
                });
            }
        }

        let resolved_str = resolved.to_string_lossy().to_string();
        for pattern in &self.forbidden_patterns {
            if resolved_str.contains(pattern.trim_end_matches('*')) {
                return Err(BytodeError::Tool {
                    tool: "write_file".into(),
                    message: format!("path matches forbidden pattern: {}", pattern),
                });
            }
        }

        let old_content = if resolved.exists() {
            Some(std::fs::read_to_string(&resolved)?)
        } else {
            None
        };

        let tmp = resolved.with_extension("bytode_tmp");
        std::fs::write(&tmp, &input.content)?;
        std::fs::rename(&tmp, &resolved)?;

        if let Some(ref lsp) = self.lsp {
            lsp.notify_did_change(&resolved, &input.content).await;
        }

        let diff = match &old_content {
            Some(old) => compute_unified_diff(old, &input.content, &resolved_str),
            None => format!("new file: {} ({} lines)", resolved_str, input.content.lines().count()),
        };

        Ok(ToolResult::WriteConfirmation {
            path: resolved_str,
            bytes_written: input.content.len(),
            lines: input.content.lines().count(),
            diff,
        })
    }
}

fn compute_unified_diff(old: &str, new: &str, filename: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut diff = vec![format!("--- {}", filename), format!("+++ {}", filename)];
    let mut i = 0;
    let mut j = 0;

    while i < old_lines.len() || j < new_lines.len() {
        let old_line = old_lines.get(i).copied().unwrap_or("");
        let new_line = new_lines.get(j).copied().unwrap_or("");

        if i < old_lines.len() && j < new_lines.len() && old_line == new_line {
            i += 1;
            j += 1;
            continue;
        }

        if i < old_lines.len() && (j >= new_lines.len() || old_line != new_line) {
            diff.push(format!("-{}", old_line));
            i += 1;
        }
        if j < new_lines.len() && (i >= old_lines.len() || old_lines.get(i) != Some(&new_line)) {
            diff.push(format!("+{}", new_line));
            j += 1;
        }
    }

    diff.join("\n")
}
