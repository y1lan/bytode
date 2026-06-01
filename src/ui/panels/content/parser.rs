use super::model::{
    AssistantMessage, AssistantPart, ReasoningPart, TextPart, ToolPart, ToolPresentation,
    ToolState,
};
use crate::ui::events::ExecState;

pub(crate) fn parse_assistant_message(
    content: &str,
    current_tool_state: ToolState,
) -> AssistantMessage {
    let mut parts = Vec::new();
    let mut text_buf = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        if let Some(tool_summary) = parse_tool_header(line) {
            flush_text_buffer(&mut text_buf, &mut parts);

            let mut body = Vec::new();
            while let Some(next) = lines.peek() {
                if parse_tool_header(next).is_some() {
                    break;
                }
                if body.is_empty() && next.trim().is_empty() {
                    lines.next();
                    break;
                }

                let next_line = lines.next().unwrap_or_default();
                if next_line.trim().is_empty() {
                    break;
                }
                body.push(next_line.to_string());
            }

            let joined_body = join_nonempty_lines(&body);
            let state = classify_tool_state(&tool_summary, joined_body.as_deref(), current_tool_state);
            let presentation = choose_tool_presentation(&tool_summary, joined_body.as_deref(), state);
            let collapsed = joined_body
                .as_deref()
                .map(|value| value.lines().count() > 6)
                .unwrap_or(false);

            parts.push(AssistantPart::Tool(ToolPart {
                name: tool_name_from_summary(&tool_summary),
                summary: tool_summary,
                body: joined_body,
                state,
                presentation,
                collapsed,
            }));
            continue;
        }

        if let Some(reasoning) = parse_reasoning_header(line) {
            flush_text_buffer(&mut text_buf, &mut parts);

            let mut body = Vec::new();
            while let Some(next) = lines.peek() {
                if parse_tool_header(next).is_some() || parse_reasoning_header(next).is_some() {
                    break;
                }
                let next_line = lines.next().unwrap_or_default();
                if next_line.trim().is_empty() {
                    break;
                }
                body.push(next_line.to_string());
            }

            parts.push(AssistantPart::Reasoning(ReasoningPart {
                summary: reasoning,
                content: join_nonempty_lines(&body).unwrap_or_default(),
                collapsed: true,
            }));
            continue;
        }

        text_buf.push(line.to_string());
    }

    flush_text_buffer(&mut text_buf, &mut parts);

    AssistantMessage { parts }
}

pub(crate) fn stream_tool_state(exec_state: Option<&ExecState>) -> ToolState {
    match exec_state {
        Some(ExecState::AwaitingApproval { .. }) => ToolState::WaitingApproval,
        Some(ExecState::ToolRunning { .. }) | Some(ExecState::Streaming { .. }) => {
            ToolState::Running
        }
        Some(ExecState::Blocked { reason, .. })
            if reason.to_ascii_lowercase().contains("denied") =>
        {
            ToolState::Denied
        }
        _ => ToolState::Completed,
    }
}

fn flush_text_buffer(buffer: &mut Vec<String>, parts: &mut Vec<AssistantPart>) {
    let content = join_nonempty_lines(buffer);
    buffer.clear();
    if let Some(content) = content {
        parts.push(AssistantPart::Text(TextPart { content }));
    }
}

fn join_nonempty_lines(lines: &[String]) -> Option<String> {
    let joined = lines.join("\n");
    if joined.trim().is_empty() {
        None
    } else {
        Some(joined.trim().to_string())
    }
}

fn parse_tool_header(line: &str) -> Option<String> {
    let marker = '\u{27f3}';
    if !line.contains(marker) {
        return None;
    }

    let raw = line.split(marker).nth(1).unwrap_or("").trim();
    if raw.is_empty() {
        None
    } else {
        Some(summarize_tool_header(raw))
    }
}

fn summarize_tool_header(raw: &str) -> String {
    if let Some((name, args)) = raw.split_once('(') {
        let args = args.trim_end_matches(')').trim();
        if args.is_empty() {
            name.trim().to_string()
        } else {
            format!("{} {}", name.trim(), compact_args(args))
        }
    } else {
        raw.to_string()
    }
}

fn compact_args(args: &str) -> String {
    let compact = args.replace('\n', " ").replace('"', "");
    if compact.len() > 42 {
        format!("{}...", &compact[..42])
    } else {
        compact
    }
}

fn parse_reasoning_header(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.starts_with("Reasoning:") {
        Some(trimmed.trim_start_matches("Reasoning:").trim().to_string())
    } else {
        None
    }
}

fn classify_tool_state(
    summary: &str,
    body: Option<&str>,
    current_tool_state: ToolState,
) -> ToolState {
    if current_tool_state == ToolState::WaitingApproval {
        return ToolState::WaitingApproval;
    }
    if let Some(body) = body {
        let lowered = body.to_ascii_lowercase();
        if lowered.contains("denied") || lowered.contains("rejected") {
            return ToolState::Denied;
        }
        if lowered.starts_with("error:") || lowered.contains("tool_error") {
            return ToolState::Failed;
        }
    }
    if current_tool_state == ToolState::Running && body.is_none() && summary.contains(' ') {
        return ToolState::Running;
    }
    if current_tool_state == ToolState::Running && body.is_none() {
        return ToolState::Running;
    }
    ToolState::Completed
}

fn choose_tool_presentation(
    summary: &str,
    body: Option<&str>,
    state: ToolState,
) -> ToolPresentation {
    if state == ToolState::WaitingApproval {
        return ToolPresentation::Inline;
    }

    let name = tool_name_from_summary(summary);
    if matches!(
        name.as_str(),
        "cargo" | "cargo_check" | "get_diagnostics" | "write_file" | "read_file"
    ) {
        if body.is_some() {
            return ToolPresentation::Block;
        }
    }

    if state == ToolState::Running {
        if body.is_some() {
            return ToolPresentation::Block;
        }
        return ToolPresentation::Inline;
    }

    let Some(body) = body else {
        return ToolPresentation::Inline;
    };

    let line_count = body.lines().count();
    if body.contains("```")
        || body
            .lines()
            .any(|line| line.starts_with('+') || line.starts_with('-') || line.contains(" | "))
        || line_count > 4
        || body.len() > 160
    {
        ToolPresentation::Block
    } else {
        ToolPresentation::Inline
    }
}

fn tool_name_from_summary(summary: &str) -> String {
    summary.split_whitespace().next().unwrap_or("?").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_block_tools_for_diff_like_output() {
        let message = parse_assistant_message(
            "  ⟳ write_file(src/main.rs)\n+ fn main() {}\n- fn old() {}\n",
            ToolState::Completed,
        );

        let AssistantPart::Tool(part) = &message.parts[0] else {
            panic!("expected tool part");
        };

        assert_eq!(part.presentation, ToolPresentation::Block);
    }

    #[test]
    fn keeps_read_file_streaming_output_as_block() {
        let message = parse_assistant_message(
            "  ⟳ read_file(src/main.rs)\n   1 | fn main() {}\n",
            ToolState::Running,
        );

        let AssistantPart::Tool(part) = &message.parts[0] else {
            panic!("expected tool part");
        };

        assert_eq!(part.presentation, ToolPresentation::Block);
    }
}
