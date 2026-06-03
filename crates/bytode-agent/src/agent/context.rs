use crate::llm;
use crate::project::ProjectProfile;
use crate::session::RenderedEntry;
use crate::tools::{ToolRegistry, ToolResult};
use async_openai::types::ChatCompletionRequestMessage;

pub struct ContextBuilder {
    core_prompt: String,
    sop_prompt: String,
    last_was_failure: bool,
    ctx_total: u64,
    ctx_used: u64,
}

impl ContextBuilder {
    pub fn new(profile: &ProjectProfile, registry: &ToolRegistry) -> Self {
        let core_prompt = build_core_prompt(profile, registry);
        let sop_prompt = build_sop_prompt();

        ContextBuilder {
            core_prompt,
            sop_prompt,
            last_was_failure: false,
            ctx_total: 1_000_000,
            ctx_used: 0,
        }
    }

    /// Build the request messages from session-log-derived context items. The
    /// rendered slice is a full replay (already overlay-applied); nothing is
    /// dropped here.
    pub fn build(&mut self, rendered: &[RenderedEntry]) -> Vec<ChatCompletionRequestMessage> {
        // Layer 1: Core (prefix-cache anchor)
        let mut messages = vec![llm::build_system_message(&self.core_prompt)];

        for item in rendered {
            match item {
                RenderedEntry::User(content) => {
                    messages.push(llm::build_user_message(content));
                }
                RenderedEntry::Assistant(content) => {
                    messages.push(llm::build_assistant_text_message(content));
                }
                RenderedEntry::ToolCall { id, name, args } => {
                    // Must emit assistant tool_calls before the tool result
                    let assistant_tc = llm::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: args.clone(),
                    };
                    messages.push(llm::build_assistant_tool_call_message(&assistant_tc));
                }
                RenderedEntry::ToolResult { call_id, content } => {
                    messages.push(llm::build_tool_result_message(content, call_id));
                }
            }
        }

        // Layer 2: SOP (conditional)
        if self.last_was_failure {
            messages.push(llm::build_system_message(&self.sop_prompt));
            self.last_was_failure = false;
        }

        // Estimate tokens (1 tok ≈ 4 chars)
        self.ctx_used = messages
            .iter()
            .map(|m| {
                let json_str = serde_json::to_string(m).unwrap_or_default();
                json_str.len() as u64 / 4
            })
            .sum();

        messages
    }

    pub fn update_last_result(&mut self, result: &ToolResult) {
        self.last_was_failure = matches!(
            result,
            ToolResult::Diagnostics { errors, .. } if *errors > 0
        );
    }

    pub fn ctx_used(&self) -> u64 {
        self.ctx_used
    }
    pub fn ctx_total(&self) -> u64 {
        self.ctx_total
    }

    pub fn rebuild_core_prompt(&mut self, profile: &ProjectProfile, registry: &ToolRegistry) {
        self.core_prompt = build_core_prompt(profile, registry);
    }
}

fn build_core_prompt(profile: &ProjectProfile, registry: &ToolRegistry) -> String {
    let project_snapshot = profile.snapshot();
    let tool_list: Vec<String> = registry
        .active_names()
        .iter()
        .map(|n| format!("- {}", n))
        .collect();

    format!(
        r#"You are bytode, a terminal coding agent specialized in Rust development.

## Project Context
{}

## Behaviour Rules
1. Read before write — always use read_file before editing
2. Cite file:line — when referencing code, always give file path and line number
3. Diagnose first — after any build failure, use get_diagnostics or cargo_check first
4. Never grep for errors — get_diagnostics is the ONLY source of build diagnostics
5. Fix one at a time — fix one error, then cargo_check to verify, then next
6. Verify before claiming done — cargo_check with filter "length" to confirm zero diagnostics

## Available Tools
{}

You operate in a ReAct loop: think → call tool → observe result → think → ...
When your work is complete, explain what was done and what files were changed."#,
        project_snapshot,
        tool_list.join("\n"),
    )
}

fn build_sop_prompt() -> String {
    r#"## ⛔ STANDARD OPERATING PROCEDURE — MANDATORY AFTER FAILURE

A build or test failure was just detected. You MUST follow this sequence. Skipping
any step is a violation.

### Step 1: get_diagnostics()
Call get_diagnostics immediately. Do NOT:
- grep error logs
- read_file to manually scan for errors
- Guess the cause without diagnostics

### Step 2: Filter and Prioritise
Use get_diagnostics with filter="errors" to isolate errors.
Address ERRORS first, then warnings.
Fix ONE error at a time.

### Step 3: Verify After Each Fix
After each fix, call cargo_check with filter="length" to verify.
Zero diagnostics = fixed. Non-zero = continue.

### Step 4: Adversarial Verification
When you believe all errors are fixed:
1. cargo_check with filter="length" — must return 0
2. get_diagnostics with filter="errors" — must return []
3. Do NOT claim "it should be fixed" or "probably works".
   Evidence or it didn't happen."#
        .to_string()
}

pub(crate) fn format_tool_result(result: &ToolResult) -> String {
    match result {
        ToolResult::FileContent {
            path,
            content,
            line_count,
            ..
        } => {
            format!("File: {} ({} lines shown)\n{}", path, line_count, content)
        }
        ToolResult::Diagnostics {
            total,
            errors,
            warnings,
            list,
            ..
        } => {
            let mut s = format!(
                "{} diagnostics: {} errors, {} warnings\n",
                total, errors, warnings
            );
            for d in list.iter().take(20) {
                s.push_str(&format!(
                    "  {} [{}:{}:{}] {}\n",
                    d.severity.to_uppercase(),
                    d.file,
                    d.line,
                    d.column,
                    d.message
                ));
            }
            if list.len() > 20 {
                s.push_str(&format!("  ... and {} more\n", list.len() - 20));
            }
            s
        }
        ToolResult::Json {
            count,
            data,
            filter,
            ..
        } => {
            let mut s = format!("{} results", count);
            if let Some(f) = filter {
                s.push_str(&format!(" (filter: {})", f));
            }
            s.push('\n');
            let json_str = serde_json::to_string_pretty(data).unwrap_or_default();
            if json_str.len() > 2000 {
                s.push_str(&json_str[..2000]);
                s.push_str("\n... (truncated)");
            } else {
                s.push_str(&json_str);
            }
            s
        }
        ToolResult::Matches {
            pattern,
            count,
            items,
            truncated,
        } => {
            let mut s = format!(
                "{}: {} matches{}\n",
                pattern,
                count,
                if *truncated { " (truncated)" } else { "" }
            );
            for m in items.iter().take(30) {
                s.push_str(&format!(
                    "  {}:{}:{} {}\n",
                    m.file, m.line, m.column, m.text
                ));
            }
            if items.len() > 30 {
                s.push_str(&format!("  ... and {} more\n", items.len() - 30));
            }
            s
        }
        ToolResult::WriteConfirmation {
            path,
            bytes_written,
            lines,
            diff,
        } => {
            format!(
                "Wrote {} ({} bytes, {} lines)\nDiff:\n{}",
                path, bytes_written, lines, diff
            )
        }
        ToolResult::Text { content, .. } => content.clone(),
    }
}
