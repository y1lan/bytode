use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

pub struct ReadFileTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        r#"Read a file from disk. Returns content with line numbers prefixed.

WHEN TO USE: Before editing any file. When you need to understand code structure.
WHEN NOT TO USE: For searching across files — use search_code instead.
For compiler errors — use get_diagnostics instead.

RETURNS: { "type": "file", path, content (with line numbers), line_count, total_bytes }"#
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute file path to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "1-based line number to start from. Omit to read from beginning.",
                    "minimum": 1,
                    "nullable": true
                },
                "limit": {
                    "type": "integer",
                    "description": "Max lines to return. Omit to read all. Max 500.",
                    "minimum": 1,
                    "maximum": 500,
                    "nullable": true
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let path_str = args["path"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "read_file".into(),
            message: "missing 'path' argument".into(),
        })?;

        let resolved = self.resolve_path(path_str)?;
        let content = std::fs::read_to_string(&resolved).map_err(|e| BytodeError::Tool {
            tool: "read_file".into(),
            message: format!("cannot read {}: {}", resolved.display(), e),
        })?;

        let offset = args["offset"].as_u64().unwrap_or(1) as usize;
        let limit = args["limit"].as_u64().map(|l| l as usize);

        let all_lines: Vec<&str> = content.lines().collect();
        let total_lines = all_lines.len();
        let start = offset.saturating_sub(1).min(total_lines);
        let end = match limit {
            Some(l) => (start + l).min(total_lines),
            None => total_lines,
        };

        let numbered: String = all_lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4} | {}", start + i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolResult::FileContent {
            path: resolved.to_string_lossy().to_string(),
            content: numbered,
            line_count: end - start,
            total_bytes: content.len(),
        })
    }
}

impl ReadFileTool {
    fn resolve_path(&self, path_str: &str) -> Result<PathBuf> {
        let path = PathBuf::from(path_str);
        let resolved = if path.is_absolute() {
            path
        } else {
            self.project_root.join(path)
        };

        let canonical =
            std::fs::canonicalize(&resolved).map_err(|_| BytodeError::Tool {
                tool: "read_file".into(),
                message: format!("path does not exist: {}", resolved.display()),
            })?;

        if !canonical.starts_with(&self.project_root) {
            return Err(BytodeError::Tool {
                tool: "read_file".into(),
                message: "path outside project root".into(),
            });
        }

        Ok(canonical)
    }
}

pub struct WriteFileTool {
    pub project_root: PathBuf,
    pub confirm_before_write: bool,
    pub max_file_size: u64,
    pub forbidden_patterns: Vec<String>,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        r#"Write content to a file. Creates or overwrites. Returns a diff of changes.

WHEN TO USE: After diagnosing and deciding on a fix. After reading the current content.
WHEN NOT TO USE: For reading — use read_file. For searching — use search_code.

SAFETY: Writes are atomic (tmp file + rename). Path must be within project root.
RETURNS: { "type": "write_confirmation", path, bytes_written, lines, diff }"#
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute file path to write"
                },
                "content": {
                    "type": "string",
                    "description": "Complete file content to write"
                },
                "reason": {
                    "type": "string",
                    "description": "Brief explanation of why this change is needed"
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    fn requires_approval(&self) -> bool {
        self.confirm_before_write
    }
    fn timeout_ms(&self) -> u64 {
        10_000
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let path_str = args["path"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "write_file".into(),
            message: "missing 'path'".into(),
        })?;

        let content = args["content"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "write_file".into(),
            message: "missing 'content'".into(),
        })?;

        let path = PathBuf::from(path_str);
        let resolved = if path.is_absolute() {
            path.clone()
        } else {
            self.project_root.join(&path)
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
            let meta = std::fs::metadata(&resolved)?;
            if meta.len() > self.max_file_size {
                return Err(BytodeError::Tool {
                    tool: "write_file".into(),
                    message: format!(
                        "file exceeds max size ({} > {})",
                        meta.len(),
                        self.max_file_size
                    ),
                });
            }
        }

        let path_s = resolved.to_string_lossy().to_string();
        for pattern in &self.forbidden_patterns {
            if path_s.contains(pattern.trim_end_matches('*')) {
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
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, &resolved)?;

        let diff = match &old_content {
            Some(old) => compute_unified_diff(old, content, &path_s),
            None => format!(
                "new file: {} ({} lines)",
                path_s,
                content.lines().count()
            ),
        };

        Ok(ToolResult::WriteConfirmation {
            path: path_s,
            bytes_written: content.len(),
            lines: content.lines().count(),
            diff,
        })
    }
}

fn compute_unified_diff(old: &str, new: &str, filename: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut diff = Vec::new();
    diff.push(format!("--- {}", filename));
    diff.push(format!("+++ {}", filename));

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

        if i < old_lines.len()
            && (j >= new_lines.len() || old_line != new_line)
        {
            diff.push(format!("-{}", old_line));
            i += 1;
        }
        if j < new_lines.len()
            && (i >= old_lines.len() || old_lines.get(i) != Some(&new_line))
        {
            diff.push(format!("+{}", new_line));
            j += 1;
        }
    }

    diff.join("\n")
}
