pub mod cargo;
pub mod check;
pub mod file;
pub mod git;
pub mod lsp_diag;
pub mod search;
pub mod web;

use crate::error::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

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
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: Value) -> Result<ToolResult>;

    fn requires_approval(&self) -> bool {
        false
    }
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

pub struct ToolEntry {
    pub tool: Box<dyn Tool>,
    pub category: ToolCategory,
    pub availability: ToolAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    ReadOnly,
    Modification,
    Build,
}

pub struct ToolRegistry {
    all: Vec<ToolEntry>,
    active: Vec<usize>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<ToolEntry>) -> Self {
        ToolRegistry {
            all: tools,
            active: Vec::new(),
        }
    }

    pub fn activate_for(
        &mut self,
        primary_language: &str,
        detected_languages: &HashSet<String>,
        enabled: &HashSet<String>,
        disabled: &HashSet<String>,
        is_exact: bool,
    ) {
        self.active.clear();

        for (i, entry) in self.all.iter().enumerate() {
            let name = entry.tool.name().to_string();

            if disabled.contains(&name) {
                continue;
            }

            if is_exact {
                if enabled.contains(&name) {
                    self.active.push(i);
                }
                continue;
            }

            let allowed = match &entry.availability {
                ToolAvailability::Always => true,
                ToolAvailability::PrimaryLanguage { requires } => {
                    requires.contains(&primary_language)
                }
                ToolAvailability::DetectedLanguage { languages } => languages
                    .iter()
                    .any(|l| detected_languages.contains(*l)),
            };

            let user_added = enabled.contains(&name);
            if allowed || user_added {
                self.active.push(i);
            }
        }
    }

    pub fn find(&self, name: &str) -> Option<&dyn Tool> {
        self.active
            .iter()
            .find(|&&i| self.all[i].tool.name() == name)
            .map(|&i| &*self.all[i].tool)
    }

    pub fn active_names(&self) -> Vec<String> {
        self.active
            .iter()
            .map(|&i| self.all[i].tool.name().to_string())
            .collect()
    }

    pub fn to_openai_format(&self) -> Vec<Value> {
        self.active
            .iter()
            .map(|&i| {
                let t = &self.all[i];
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.tool.name(),
                        "description": t.tool.description(),
                        "parameters": t.tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }
}
