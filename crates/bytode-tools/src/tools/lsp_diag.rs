use crate::error::Result;
use crate::lsp::LspClient;
use crate::{
    ApprovalKind, RiskLevel, ToolCapability, ToolCategory, ToolDescriptor,
};
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
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "get_diagnostics",
            description: r#"Get structured diagnostics from rust-analyzer's live cache.

WHEN TO USE (mandatory):
1. After calling cargo_check — if cargo_check shows errors, call get_diagnostics() WITHOUT path to get ALL errors.
2. Before fixing any error — call get_diagnostics() or get_diagnostics(filter="errors") to see what to fix.
3. After write_file — diagnostics refresh automatically, call get_diagnostics() to verify.

HOW TO CALL:
- get_diagnostics()                        → ALL diagnostics for the project (most common)
- get_diagnostics(path="src/main.rs")     → diagnostics for a specific RS file
- get_diagnostics(filter="errors")        → errors only
- get_diagnostics(filter="length")        → count of diagnostics (0 = clean!)

DO NOT:
- grep error logs or cargo_check output — get_diagnostics is the ONLY authorized source
- pass Cargo.toml or directory paths — only .rs files

RETURNS: { "type": "diagnostics", tool: "rust-analyzer", total, errors, warnings, list: [{file, line, column, severity, message, code}] }"#,
            provider_id: "builtin",
            category: ToolCategory::ReadOnly,
            capabilities: vec![ToolCapability::ReadDiagnostics],
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
                    "description": "A .rs source file path to filter diagnostics for. OMIT to get ALL project diagnostics. Do NOT pass directories or Cargo.toml.",
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
