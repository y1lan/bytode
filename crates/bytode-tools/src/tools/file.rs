use crate::error::{BytodeError, Result};
use crate::{
    ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor,
};
use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ReadFileTool {
    pub project_root: PathBuf,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read_file",
            description: r#"Read a file or list a directory. Returns line-numbered content for files, tree listing for directories.

WHEN TO USE: Before editing any file. When you need to understand code structure.
When you need to discover what files exist in a directory.
WHEN NOT TO USE: For searching across files — use search_code instead.
For compiler errors — use get_diagnostics instead.

EXAMPLES:
  read_file(path="/home/user/project/src/main.rs")           # read a file
  read_file(path="/home/user/project/src/main.rs", offset=10) # read from line 10
  read_file(path="/home/user/project/src/main.rs", limit=50)  # read first 50 lines
  read_file(path="/home/user/project/src")                   # list directory contents

RETURNS: For files: { "type": "file", path, content (with line numbers), line_count, total_bytes }.
For directories: { "type": "text", path, content (tree listing with sizes and types) }."#,
            provider_id: "builtin",
            category: ToolCategory::ReadOnly,
            capabilities: vec![ToolCapability::ReadProjectFile],
            default_risk: RiskLevel::Low,
            approval: ApprovalKind::Never,
        }
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute file or directory path. Files return numbered content, directories return a listing."
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

        if resolved.is_dir() {
            return self.list_dir(&resolved);
        }
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

        let canonical = std::fs::canonicalize(&resolved).map_err(|_| BytodeError::Tool {
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

    fn list_dir(&self, dir: &std::path::Path) -> Result<ToolResult> {
        let mut entries: Vec<String> = Vec::new();
        let mut iter = std::fs::read_dir(dir).map_err(|e| BytodeError::Tool {
            tool: "read_file".into(),
            message: format!("cannot read directory {}: {}", dir.display(), e),
        })?;

        while let Some(entry) = iter.next() {
            let entry = entry.map_err(|e| BytodeError::Tool {
                tool: "read_file".into(),
                message: format!("error reading entry: {}", e),
            })?;
            let ft = entry.file_type().map_err(|e| BytodeError::Tool {
                tool: "read_file".into(),
                message: format!("cannot stat: {}", e),
            })?;
            let name = entry.file_name().to_string_lossy().to_string();
            let suffix = if ft.is_dir() {
                "/".to_string()
            } else if ft.is_symlink() {
                " -> ?".to_string()
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                format!(" ({}B)", human_size(size))
            };
            entries.push(format!("  {}{}", name, suffix));
        }
        entries.sort();

        let content = format!("{}/\n{}", dir.to_string_lossy(), entries.join("\n"));

        Ok(ToolResult::Text {
            source: "read_file".into(),
            content,
            truncated: entries.len() > 200,
        })
    }
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    }
}

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
                let changed: Vec<&str> = diff
                    .lines()
                    .filter(|l| l.starts_with('-') || l.starts_with('+'))
                    .take(40)
                    .collect();
                let mut s = format!("  {} ({} bytes, {} lines)\n", short, bytes_written, lines);
                for l in &changed {
                    s.push_str(&format!("  {}\n", l));
                }
                if diff
                    .lines()
                    .filter(|l| l.starts_with('-') || l.starts_with('+'))
                    .count()
                    > 40
                {
                    s.push_str("  ...\n");
                }
                Some(s)
            }
        } else {
            None
        }
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

        if let Some(ref lsp) = self.lsp {
            lsp.notify_did_change(&resolved, content).await;
        }

        let diff = match &old_content {
            Some(old) => compute_unified_diff(old, content, &path_s),
            None => format!("new file: {} ({} lines)", path_s, content.lines().count()),
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
