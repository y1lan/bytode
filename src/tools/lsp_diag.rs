use crate::error::Result;
use crate::lsp::LspClient;
use crate::tools::check::apply_simple_filter;
use crate::tools::{DiagnosticItem, Tool, ToolResult};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct DiagnosticsTool {
    pub lsp: Arc<LspClient>,
}

#[async_trait]
impl Tool for DiagnosticsTool {
    fn name(&self) -> &'static str {
        "get_diagnostics"
    }

    fn description(&self) -> &'static str {
        r#"Get structured diagnostics from rust-analyzer.

WHEN TO USE: After a compile failure. Before reading any file to fix errors.
This is the ONLY source of diagnostics — do not grep error logs or build output.
WHEN NOT TO USE: For finding code patterns → use search_code. For reading code → use read_file.

Diagnostics are cached from rust-analyzer's publishDiagnostics push.
Accepts an optional `filter` argument for precise extraction:
  "errors" → errors only
  "warnings" → warnings only
  "length" → count of diagnostics

RETURNS: { "type": "diagnostics", tool, total, errors, warnings, list: [...] }"#
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Filter diagnostics for this file only. Omit for all files.",
                    "nullable": true
                },
                "filter": {
                    "type": "string",
                    "description": "Simple filter: 'errors', 'warnings', 'length', or .field == value",
                    "nullable": true
                }
            },
            "required": [],
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        5_000
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        if let Err(e) = self.lsp.ensure_started().await {
            return Ok(ToolResult::Text {
                source: "get_diagnostics".into(),
                content: format!(
                    "LSP not available: {}. Do you have rust-analyzer installed?",
                    e
                ),
                truncated: false,
            });
        }

        let path_filter = args["path"].as_str();

        let raw_diags = match path_filter {
            Some(path) => self.lsp.get_cached_diagnostics_for(path).await,
            None => self.lsp.get_cached_diagnostics().await,
        };

        let errors = raw_diags.iter().filter(|d| d.severity == "error").count();
        let warnings = raw_diags.iter().filter(|d| d.severity == "warning").count();

        let list: Vec<DiagnosticItem> = raw_diags
            .into_iter()
            .map(|d| DiagnosticItem {
                file: d.file,
                line: d.line,
                column: d.column,
                severity: d.severity,
                message: d.message,
                code: d.code,
            })
            .collect();

        if let Some(filter_expr) = args["filter"].as_str() {
            let raw_json: Vec<Value> = list
                .iter()
                .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
                .collect();

            let filtered = apply_simple_filter(&raw_json, filter_expr);
            return Ok(ToolResult::Json {
                tool: "rust-analyzer".into(),
                filter: Some(filter_expr.to_string()),
                count: filtered.len(),
                data: filtered,
            });
        }

        Ok(ToolResult::Diagnostics {
            tool: "rust-analyzer".into(),
            total: list.len(),
            errors,
            warnings,
            list,
        })
    }
}
