use crate::tools::ToolResult;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct Turn {
    pub user_input: Option<String>,
    pub assistant_text: Option<String>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub user_intent: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: ToolResult,
}

impl Turn {
    pub fn new(user_input: &str) -> Self {
        Turn {
            user_input: Some(user_input.to_string()),
            assistant_text: None,
            tool_calls: Vec::new(),
            user_intent: Some(user_input.to_string()),
        }
    }

    pub fn add_result(
        &mut self,
        name: String,
        args: serde_json::Value,
        result: ToolResult,
    ) {
        self.tool_calls.push(ToolCallRecord {
            name,
            arguments: args,
            result,
        });
    }
}

pub struct MemoryLayer {
    recent: VecDeque<Turn>,
    summary: Option<String>,
    max_recent: usize,
    compress_threshold: f64,
}

impl MemoryLayer {
    pub fn new(max_recent: usize) -> Self {
        MemoryLayer {
            recent: VecDeque::new(),
            summary: None,
            max_recent,
            compress_threshold: 0.8,
        }
    }

    pub fn add_turn(&mut self, turn: Turn) {
        self.recent.push_back(turn);
    }

    pub fn last_turn_mut(&mut self) -> Option<&mut Turn> {
        self.recent.back_mut()
    }

    pub fn recent_turns(&self) -> &VecDeque<Turn> {
        &self.recent
    }

    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    pub fn should_compress(&self, ctx_used: u64, ctx_total: u64) -> bool {
        let ratio = ctx_used as f64 / ctx_total as f64;
        ratio > self.compress_threshold && self.recent.len() > self.max_recent
    }

    pub fn compress(&mut self) -> String {
        let to_compress: Vec<Turn> = self
            .recent
            .drain(..self.recent.len().saturating_sub(self.max_recent))
            .collect();

        if to_compress.is_empty() {
            return self.summary.clone().unwrap_or_default();
        }

        let new_summary = summarize_turns(&to_compress);

        self.summary = Some(match &self.summary {
            Some(old) => merge_summaries(old, &new_summary),
            None => new_summary,
        });

        self.summary.clone().unwrap()
    }
}

fn summarize_turns(turns: &[Turn]) -> String {
    let mut files_modified = HashSet::new();
    let mut errors_fixed = Vec::new();
    let mut task_description = String::new();

    for turn in turns {
        if let Some(ref intent) = turn.user_intent {
            task_description = intent.clone();
        }

        for record in &turn.tool_calls {
            match &record.result {
                ToolResult::WriteConfirmation { path, .. } => {
                    files_modified.insert(path.clone());
                }
                ToolResult::Diagnostics { list, .. } => {
                    for d in list.iter().filter(|d| d.severity == "error") {
                        errors_fixed.push(format!(
                            "{} [{}:{}]",
                            d.message, d.file, d.line
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    let mut summary = format!("## Task\n{}\n", task_description);

    if !files_modified.is_empty() {
        summary.push_str("## Files Modified\n");
        for f in &files_modified {
            summary.push_str(&format!("- {}\n", f));
        }
    }

    if !errors_fixed.is_empty() {
        summary.push_str("## Errors Resolved\n");
        for e in errors_fixed.iter().take(10) {
            summary.push_str(&format!("- {}\n", e));
        }
    }

    summary
}

fn merge_summaries(old: &str, new: &str) -> String {
    format!("{}\n{}", old, new)
}
