use super::{tool_calls::format_args, Agent, memory};
use crate::error::Result;
use crate::tools::ToolResult;
use crate::transcript::{
    AssistantMessage, AssistantPart, HistoryEntry, TextPart, ToolPart, ToolPresentation, ToolState,
    UserMessage,
};

impl Agent {
    pub fn chat_history_entries(&self) -> Vec<HistoryEntry> {
        let mut entries = Vec::new();

        for turn in self.memory.recent_turns() {
            if let Some(input) = &turn.user_input {
                entries.push(HistoryEntry::User(UserMessage {
                    body: input.clone(),
                    meta: None,
                }));
            }

            let mut parts = turn
                .tool_calls
                .iter()
                .map(history_tool_part)
                .map(AssistantPart::Tool)
                .collect::<Vec<_>>();

            if let Some(text) = &turn.assistant_text
                && !text.trim().is_empty()
            {
                parts.push(AssistantPart::Text(TextPart {
                    content: text.clone(),
                }));
            }

            if !parts.is_empty() {
                entries.push(HistoryEntry::Assistant(AssistantMessage { parts }));
            }
        }

        entries
    }

    /// Replay the session log and convert it into `HistoryEntry` items for UI
    /// display. Called on startup to restore the conversation view. Audit
    /// entries (MicroCompact, InteractiveCompact) are skipped.
    pub fn replay_chat_history(&self) -> Result<Vec<HistoryEntry>> {
        let entries = self.runtime.replay_entries()?;
        let mut history = Vec::new();
        let mut pending_tool_results: Vec<String> = Vec::new();

        for entry in &entries {
            match &entry.kind {
                crate::session::SessionEntryKind::UserMessage(u) => {
                    flush_tool_results(&mut history, &mut pending_tool_results);
                    history.push(HistoryEntry::User(UserMessage {
                        body: u.content.clone(),
                        meta: None,
                    }));
                }
                crate::session::SessionEntryKind::AssistantMessage(a) => {
                    flush_tool_results(&mut history, &mut pending_tool_results);
                    history.push(HistoryEntry::Assistant(AssistantMessage::from_text(
                        a.content.clone(),
                    )));
                }
                crate::session::SessionEntryKind::ToolCall(tc) => {
                    let summary = format!("{} (args archived)", tc.tool_name);
                    let body = tc
                        .inline_args
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "(archived)".into());
                    pending_tool_results.push(format!("{summary}\n{body}"));
                }
                crate::session::SessionEntryKind::ToolResult(tr) => {
                    let preview = tr
                        .preview
                        .clone()
                        .or_else(|| tr.inline_content.clone())
                        .unwrap_or_else(|| "(result archived)".into());
                    if let Some(last) = pending_tool_results.last_mut() {
                        last.push_str(&format!("\n→ {preview}"));
                    } else {
                        pending_tool_results.push(format!("tool result: {preview}"));
                    }
                }
                crate::session::SessionEntryKind::MicroCompact(_)
                | crate::session::SessionEntryKind::InteractiveCompact(_) => {}
                crate::session::SessionEntryKind::SystemNote(n) => {
                    flush_tool_results(&mut history, &mut pending_tool_results);
                    match n.kind {
                        crate::session::SystemNoteKind::SlashCommand => {
                            history.push(HistoryEntry::User(UserMessage {
                                body: n.content.clone(),
                                meta: None,
                            }));
                        }
                        crate::session::SystemNoteKind::Error
                        | crate::session::SystemNoteKind::Cancelled => {
                            history.push(HistoryEntry::Error(n.content.clone()));
                        }
                    }
                }
            }
        }
        flush_tool_results(&mut history, &mut pending_tool_results);

        Ok(history)
    }
}

fn flush_tool_results(history: &mut Vec<HistoryEntry>, pending: &mut Vec<String>) {
    if pending.is_empty() {
        return;
    }
    let parts: Vec<AssistantPart> = std::mem::take(pending)
        .into_iter()
        .map(|body| {
            AssistantPart::Tool(ToolPart {
                name: String::new(),
                summary: String::new(),
                body: Some(body),
                state: ToolState::Completed,
                presentation: ToolPresentation::Block,
                collapsed: true,
            })
        })
        .collect();
    history.push(HistoryEntry::Assistant(AssistantMessage { parts }));
}

fn history_tool_part(record: &memory::ToolCallRecord) -> ToolPart {
    let summary = format!("{} {}", record.name, format_args(&record.name, &record.arguments))
        .trim()
        .to_string();
    let state = if record.is_error() {
        ToolState::Failed
    } else {
        ToolState::Completed
    };
    let body = history_tool_body(&record.result);
    let presentation = history_tool_presentation(&record.name, body.as_deref());
    let collapsed = body
        .as_deref()
        .map(|content| content.lines().count() > 6)
        .unwrap_or(false);

    ToolPart {
        name: record.name.clone(),
        summary,
        body,
        state,
        presentation,
        collapsed,
    }
}

fn history_tool_body(result: &ToolResult) -> Option<String> {
    match result {
        ToolResult::FileContent {
            path,
            content,
            line_count,
            ..
        } => Some(format!("{}\n{} lines\n{}", path, line_count, content)),
        ToolResult::Diagnostics {
            total,
            errors,
            warnings,
            list,
            ..
        } => {
            let mut lines = vec![format!(
                "diagnostics: total={} errors={} warnings={}",
                total, errors, warnings
            )];
            lines.extend(list.iter().take(8).map(|item| {
                format!(
                    "{}:{} {} {}",
                    item.file, item.line, item.severity, item.message
                )
            }));
            Some(lines.join("\n"))
        }
        ToolResult::Json {
            tool, count, data, ..
        } => {
            let preview = data
                .iter()
                .take(6)
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            Some(format!("{tool}: {count} item(s)\n{preview}"))
        }
        ToolResult::Matches {
            pattern,
            count,
            items,
            truncated,
        } => {
            let mut lines = vec![format!("pattern={pattern} count={count}")];
            lines.extend(
                items
                    .iter()
                    .take(8)
                    .map(|item| format!("{}:{} {}", item.file, item.line, item.text)),
            );
            if *truncated {
                lines.push("...".into());
            }
            Some(lines.join("\n"))
        }
        ToolResult::WriteConfirmation {
            path,
            bytes_written,
            lines,
            diff,
        } => Some(format!(
            "{}\n{} bytes, {} lines\n{}",
            path, bytes_written, lines, diff
        )),
        ToolResult::Text { content, .. } => Some(content.clone()),
    }
}

fn history_tool_presentation(name: &str, body: Option<&str>) -> ToolPresentation {
    if matches!(
        name,
        "write_file" | "cargo" | "cargo_check" | "get_diagnostics"
    ) {
        return ToolPresentation::Block;
    }

    let Some(body) = body else {
        return ToolPresentation::Inline;
    };

    if body.contains("```")
        || body.lines().count() > 4
        || body
            .lines()
            .any(|line| line.starts_with('+') || line.starts_with('-') || line.contains(" | "))
        || body.len() > 160
    {
        ToolPresentation::Block
    } else {
        ToolPresentation::Inline
    }
}
