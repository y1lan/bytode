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

    /// Returns the core system prompt (for use with canonical context).
    pub fn core_prompt(&self) -> &str {
        &self.core_prompt
    }

    /// Returns the SOP prompt (for use with canonical context).
    pub fn sop_prompt(&self) -> &str {
        &self.sop_prompt
    }

    /// Whether the last tool result was a failure.
    pub fn last_was_failure(&self) -> bool {
        self.last_was_failure
    }

    /// Clear the last-failure flag after injecting SOP.
    pub fn clear_last_failure(&mut self) {
        self.last_was_failure = false;
    }

    /// Estimate tokens from the given messages (1 tok ≈ 4 chars).
    pub fn estimate_tokens(&mut self, messages: &[ChatCompletionRequestMessage]) -> u64 {
        let used: u64 = messages
            .iter()
            .map(|m| {
                let json_str = serde_json::to_string(m).unwrap_or_default();
                json_str.len() as u64 / 4
            })
            .sum();
        self.ctx_used = used;
        used
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

#[allow(dead_code)]
fn append_rendered_entries(
    messages: &mut Vec<ChatCompletionRequestMessage>,
    rendered: &[RenderedEntry],
) {
    let mut idx = 0usize;
    while idx < rendered.len() {
        match &rendered[idx] {
            RenderedEntry::User(content) => {
                messages.push(llm::build_user_message(content));
                idx += 1;
            }
            RenderedEntry::Assistant(content) => {
                messages.push(llm::build_assistant_text_message(content));
                idx += 1;
            }
            RenderedEntry::ToolCall { group_id, .. } => {
                let mut tool_calls = Vec::new();
                let mut tool_results = Vec::new();
                let current_group_id = group_id.clone();

                while idx + 1 < rendered.len() {
                    let RenderedEntry::ToolCall {
                        group_id,
                        id,
                        name,
                        args,
                    } = &rendered[idx]
                    else {
                        break;
                    };
                    let RenderedEntry::ToolResult {
                        group_id: result_group_id,
                        call_id,
                        content,
                    } = &rendered[idx + 1]
                    else {
                        break;
                    };
                    if *group_id != current_group_id || *result_group_id != current_group_id {
                        break;
                    }

                    tool_calls.push(llm::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: args.clone(),
                    });
                    tool_results.push((call_id.clone(), content.clone()));
                    idx += 2;
                }

                if tool_calls.is_empty() {
                    idx += 1;
                    continue;
                }

                messages.push(llm::build_assistant_tool_calls_message(&tool_calls));
                for (call_id, content) in tool_results {
                    messages.push(llm::build_tool_result_message(&content, &call_id));
                }
            }
            RenderedEntry::ToolResult {
                call_id, content, ..
            } => {
                messages.push(llm::build_tool_result_message(content, call_id));
                idx += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::append_rendered_entries;
    use crate::session::RenderedEntry;

    #[test]
    fn consecutive_tool_pairs_are_grouped_into_one_assistant_message() {
        let rendered = vec![
            RenderedEntry::User("u".into()),
            RenderedEntry::ToolCall {
                group_id: Some("g1".into()),
                id: "call_1".into(),
                name: "read_file".into(),
                args: serde_json::json!({"path": "a.rs"}),
            },
            RenderedEntry::ToolResult {
                group_id: Some("g1".into()),
                call_id: "call_1".into(),
                content: "a".into(),
            },
            RenderedEntry::ToolCall {
                group_id: Some("g1".into()),
                id: "call_2".into(),
                name: "git_status".into(),
                args: serde_json::json!({}),
            },
            RenderedEntry::ToolResult {
                group_id: Some("g1".into()),
                call_id: "call_2".into(),
                content: "b".into(),
            },
        ];

        let mut messages = Vec::new();
        append_rendered_entries(&mut messages, &rendered);

        assert_eq!(messages.len(), 4);
        let assistant = serde_json::to_value(&messages[1]).unwrap();
        let tool_calls = assistant
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .expect("assistant tool_calls");
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(
            tool_calls[0].get("id").and_then(|v| v.as_str()),
            Some("call_1")
        );
        assert_eq!(
            tool_calls[1].get("id").and_then(|v| v.as_str()),
            Some("call_2")
        );
        let tool_1 = serde_json::to_value(&messages[2]).unwrap();
        let tool_2 = serde_json::to_value(&messages[3]).unwrap();
        assert_eq!(
            tool_1.get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );
        assert_eq!(
            tool_2.get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_2")
        );
    }

    #[test]
    fn different_tool_groups_stay_in_separate_assistant_messages() {
        let rendered = vec![
            RenderedEntry::ToolCall {
                group_id: Some("g1".into()),
                id: "call_1".into(),
                name: "read_file".into(),
                args: serde_json::json!({"path": "a.rs"}),
            },
            RenderedEntry::ToolResult {
                group_id: Some("g1".into()),
                call_id: "call_1".into(),
                content: "a".into(),
            },
            RenderedEntry::ToolCall {
                group_id: Some("g2".into()),
                id: "call_2".into(),
                name: "git_status".into(),
                args: serde_json::json!({}),
            },
            RenderedEntry::ToolResult {
                group_id: Some("g2".into()),
                call_id: "call_2".into(),
                content: "b".into(),
            },
        ];

        let mut messages = Vec::new();
        append_rendered_entries(&mut messages, &rendered);

        assert_eq!(messages.len(), 4);
        let assistant_1 = serde_json::to_value(&messages[0]).unwrap();
        let assistant_2 = serde_json::to_value(&messages[2]).unwrap();
        assert_eq!(
            assistant_1
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
        assert_eq!(
            assistant_2
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
    }

    #[test]
    fn single_tool_responses_with_different_groups_do_not_merge() {
        let rendered = vec![
            RenderedEntry::ToolCall {
                group_id: Some("g1".into()),
                id: "call_1".into(),
                name: "read_file".into(),
                args: serde_json::json!({"path": "a.rs"}),
            },
            RenderedEntry::ToolResult {
                group_id: Some("g1".into()),
                call_id: "call_1".into(),
                content: "a".into(),
            },
            RenderedEntry::ToolCall {
                group_id: Some("g2".into()),
                id: "call_2".into(),
                name: "read_file".into(),
                args: serde_json::json!({"path": "b.rs"}),
            },
            RenderedEntry::ToolResult {
                group_id: Some("g2".into()),
                call_id: "call_2".into(),
                content: "b".into(),
            },
        ];

        let mut messages = Vec::new();
        append_rendered_entries(&mut messages, &rendered);

        assert_eq!(messages.len(), 4);
        for idx in [0usize, 2usize] {
            let assistant = serde_json::to_value(&messages[idx]).unwrap();
            assert_eq!(
                assistant
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .map(|v| v.len()),
                Some(1)
            );
        }
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
