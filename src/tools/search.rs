use crate::error::{BytodeError, Result};
use crate::tools::{MatchItem, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

pub struct SearchTool {
    pub project_root: PathBuf,
    pub ignore_dirs: Vec<String>,
    pub max_results: usize,
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &'static str {
        "search_code"
    }

    fn description(&self) -> &'static str {
        r#"Search codebase with ripgrep. Returns structured matches with file:line:column.

WHEN TO USE: Find where a symbol is defined. Find all call sites. Discover patterns across files.
WHEN NOT TO USE: For build errors → use get_diagnostics. For reading code → use read_file.
For full text of a file → use read_file.

Matches are truncated at 200 results. Use a more specific pattern if truncated.
RETURNS: { "type": "matches", pattern, count, items: [{file, line, column, text}], truncated }"#
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Ripgrep-compatible regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in. Omit to search entire project.",
                    "nullable": true
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        15_000
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let pattern = args["pattern"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "search_code".into(),
            message: "missing 'pattern'".into(),
        })?;

        let search_path = args["path"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.project_root.clone());

        let resolved = if search_path.is_absolute() {
            search_path
        } else {
            self.project_root.join(&search_path)
        };

        let mut cmd = Command::new("rg");
        cmd.args(["--json", "--line-number", "--no-heading"])
            .arg(pattern)
            .arg(&resolved);

        for dir in &self.ignore_dirs {
            cmd.args(["--glob", &format!("!{}", dir)]);
        }

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "search_code".into(),
            message: format!("rg execution failed: {} (is ripgrep installed?)", e),
        })?;

        let raw_items: Vec<Value> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        let mut items: Vec<MatchItem> = raw_items
            .iter()
            .filter(|v| v["type"].as_str() == Some("match"))
            .filter_map(|v| {
                let data = &v["data"];
                Some(MatchItem {
                    file: data["path"]["text"].as_str()?.to_string(),
                    line: data["line_number"].as_u64()? as u32,
                    column: data["absolute_offset"].as_u64().map(|o| o as u32).unwrap_or(0),
                    text: data["lines"]["text"].as_str()?.trim_end().to_string(),
                })
            })
            .collect();

        let total_count = items.len();
        let truncated = total_count > self.max_results;
        items.truncate(self.max_results);

        Ok(ToolResult::Matches {
            pattern: pattern.to_string(),
            count: total_count,
            items,
            truncated,
        })
    }
}
