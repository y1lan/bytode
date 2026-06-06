pub mod cargo;
pub mod git;
pub mod lsp;
pub mod read_file;
pub mod search_code;
pub mod web;
pub mod write_file;

use crate::ToolDescriptor;
use crate::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResult {
    #[serde(rename = "file")]
    FileContent {
        path: String,
        content: String,
        line_count: usize,
        total_bytes: usize,
    },

    #[serde(rename = "diagnostics")]
    Diagnostics {
        tool: String,
        total: usize,
        errors: usize,
        warnings: usize,
        list: Vec<DiagnosticItem>,
    },

    #[serde(rename = "json")]
    Json {
        tool: String,
        filter: Option<String>,
        count: usize,
        data: Vec<Value>,
    },

    #[serde(rename = "matches")]
    Matches {
        pattern: String,
        count: usize,
        items: Vec<MatchItem>,
        truncated: bool,
    },

    #[serde(rename = "write_confirmation")]
    WriteConfirmation {
        path: String,
        bytes_written: usize,
        lines: usize,
        diff: String,
    },

    #[serde(rename = "text")]
    Text {
        source: String,
        content: String,
        truncated: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiagnosticItem {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: String,
    pub message: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatchItem {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum ToolAvailability {
    Always,
    PrimaryLanguage { requires: &'static [&'static str] },
    DetectedLanguage { languages: &'static [&'static str] },
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: Value) -> Result<ToolResult>;

    fn timeout_ms(&self) -> u64 {
        30_000
    }
    fn cooldown_ms(&self) -> u64 {
        0
    }
    fn max_consecutive_calls(&self) -> Option<u32> {
        None
    }

    fn format_result_for_display(&self, _result: &ToolResult) -> Option<String> {
        None
    }
}
