use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::process::Command;

pub struct CheckTool {
    pub extra_args: Vec<String>,
}

#[async_trait]
impl Tool for CheckTool {
    fn name(&self) -> &'static str {
        "cargo_check"
    }

    fn description(&self) -> &'static str {
        r#"Run `cargo check --message-format json` and return structured build diagnostics.

All output is line-delimited JSON. Use the optional `filter` argument with simple jq-like
syntax to extract specific information.

Supported filter operations:
  - `errors` → errors only (macro-like shorthand)
  - `.severity == "error"` → filter by field match
  - `length` → count of diagnostics
  - Fields can be extracted: file, line, column, severity, message, code

RETURNS: { "type": "json", tool: "cargo", filter: optional, count, data: [...] }"#
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Simple filter: 'errors', 'length', or '.field == \"value\"'",
                    "nullable": true
                }
            },
            "required": [],
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        120_000
    }
    fn max_consecutive_calls(&self) -> Option<u32> {
        Some(5)
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let mut cmd = Command::new("cargo");
        cmd.args(["check", "--message-format", "json"]);
        cmd.args(&self.extra_args);

        let output = cmd.output().map_err(|e| BytodeError::Tool {
            tool: "cargo_check".into(),
            message: format!("cargo check failed to start: {}", e),
        })?;

        let raw_diagnostics: Vec<Value> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|v: &Value| {
                v.get("reason").and_then(|r| r.as_str()) == Some("compiler-message")
            })
            .map(|v| {
                let msg = &v["message"];
                let spans = &msg["spans"];
                let primary_span = spans.as_array().and_then(|s| {
                    s.iter().find(|sp| {
                        sp.get("is_primary")
                            .and_then(|p| p.as_bool())
                            == Some(true)
                    })
                });

                let code = msg["code"]
                    .as_object()
                    .and_then(|c| c.get("code"))
                    .and_then(|c| c.as_str())
                    .map(String::from);

                serde_json::json!({
                    "severity": msg["level"].as_str().unwrap_or("unknown"),
                    "message": msg["message"].as_str().unwrap_or(""),
                    "code": code,
                    "file": primary_span
                        .and_then(|s| s["file_name"].as_str())
                        .unwrap_or("<unknown>"),
                    "line": primary_span
                        .and_then(|s| s["line_start"].as_u64())
                        .unwrap_or(0) as u32,
                    "column": primary_span
                        .and_then(|s| s["line_end"].as_u64())
                        .unwrap_or(0) as u32,
                })
            })
            .collect();

        let filter = args["filter"].as_str().map(String::from);

        if let Some(ref filter_expr) = filter {
            let filtered = apply_simple_filter(&raw_diagnostics, filter_expr);
            return Ok(ToolResult::Json {
                tool: "cargo".into(),
                filter: Some(filter_expr.clone()),
                count: filtered.len(),
                data: filtered,
            });
        }

        Ok(ToolResult::Json {
            tool: "cargo".into(),
            filter: None,
            count: raw_diagnostics.len(),
            data: raw_diagnostics,
        })
    }
}

/// Apply a simple filter expression to JSON diagnostic data
///
/// Supported:
///   "errors"   → only items with severity == "error"
///   "warnings" → only items with severity == "warning"
///   "length"   → returns [{count: N}] format
///   ".field == \"value\"" → filter items where field matches
pub(crate) fn apply_simple_filter(data: &[Value], filter: &str) -> Vec<Value> {
    match filter.trim() {
        "errors" => data
            .iter()
            .filter(|v| v["severity"].as_str() == Some("error"))
            .cloned()
            .collect(),

        "warnings" => data
            .iter()
            .filter(|v| v["severity"].as_str() == Some("warning"))
            .cloned()
            .collect(),

        "length" => vec![serde_json::json!({ "count": data.len() })],

        f if f.starts_with('.') && f.contains("==") => {
            // Simple field == value filter: .severity == "error"
            let parts: Vec<&str> = f.splitn(2, "==").collect();
            if parts.len() == 2 {
                let field = parts[0]
                    .trim()
                    .strip_prefix('.')
                    .unwrap_or(parts[0].trim());
                let value = parts[1].trim().trim_matches('"');
                data.iter()
                    .filter(|v| v[field].as_str() == Some(value))
                    .cloned()
                    .collect()
            } else {
                data.to_vec()
            }
        }

        _ => data.to_vec(),
    }
}
