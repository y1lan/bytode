use super::input::ReadFileInput;
use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use crate::{ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

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
            provider_meta: None,
            category: ToolCategory::ReadOnly,
            capabilities: vec![ToolCapability::ReadProjectFile],
            default_risk: RiskLevel::Low,
            approval: ApprovalKind::Never,
        }
    }

    fn parameters_schema(&self) -> Value {
        ReadFileInput::schema()
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let input = ReadFileInput::from_value(args)?;
        let resolved = self.resolve_path(&input.path)?;

        if resolved.is_dir() {
            return self.list_dir(&resolved);
        }

        let content = std::fs::read_to_string(&resolved).map_err(|error| BytodeError::Tool {
            tool: "read_file".into(),
            message: format!("cannot read {}: {}", resolved.display(), error),
        })?;

        let all_lines: Vec<&str> = content.lines().collect();
        let total_lines = all_lines.len();
        let start = input.offset.saturating_sub(1).min(total_lines);
        let end = match input.limit {
            Some(limit) => (start + limit).min(total_lines),
            None => total_lines,
        };

        let numbered = all_lines[start..end]
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{:>4} | {}", start + index + 1, line))
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
        let mut entries = Vec::new();
        let iter = std::fs::read_dir(dir).map_err(|error| BytodeError::Tool {
            tool: "read_file".into(),
            message: format!("cannot read directory {}: {}", dir.display(), error),
        })?;

        for entry in iter {
            let entry = entry.map_err(|error| BytodeError::Tool {
                tool: "read_file".into(),
                message: format!("error reading entry: {}", error),
            })?;
            let file_type = entry.file_type().map_err(|error| BytodeError::Tool {
                tool: "read_file".into(),
                message: format!("cannot stat: {}", error),
            })?;
            let name = entry.file_name().to_string_lossy().to_string();
            let suffix = if file_type.is_dir() {
                "/".to_string()
            } else if file_type.is_symlink() {
                " -> ?".to_string()
            } else {
                let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                format!(" ({}B)", human_size(size))
            };
            entries.push(format!("  {}{}", name, suffix));
        }
        entries.sort();

        Ok(ToolResult::Text {
            source: "read_file".into(),
            content: format!("{}/\n{}", dir.to_string_lossy(), entries.join("\n")),
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
