//! In-process recent-turn structure backing the UI transcript only.
//!
//! It is NOT the context source (that is the session log) and it never
//! generates task facts or summaries — the old `## Task / ## Files Modified /
//! ## Errors Resolved` summary compaction has been removed.

use crate::tools::ToolResult;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub user_input: Option<String>,
    pub assistant_text: Option<String>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub user_intent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: ToolResult,
}

impl ToolCallRecord {
    pub fn is_error(&self) -> bool {
        matches!(
            &self.result,
            ToolResult::Text { source, .. } if source == "tool_error"
        )
    }
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
}

pub struct MemoryLayer {
    recent: VecDeque<Turn>,
}

impl MemoryLayer {
    pub fn new() -> Self {
        MemoryLayer {
            recent: VecDeque::new(),
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
}

impl Default for MemoryLayer {
    fn default() -> Self {
        Self::new()
    }
}
