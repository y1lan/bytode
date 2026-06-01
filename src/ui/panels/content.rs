use crate::ui::events::{DispatchResult, ExecState, KeyAction, PanelId, WindowSlot, WindowSpec};
use crate::ui::panels::{
    panel_block, BG, BLUE, BORDER, CYAN, ORANGE, PanelContext, RED, RenderContext, TXT,
    TXT_SUBTLE, UiContext,
};
use crate::ui::render;
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};

const CARD_BG: Color = Color::Rgb(230, 233, 239);
const BLOCK_BG: Color = Color::Rgb(220, 224, 232);

#[derive(Clone, Debug)]
pub enum HistoryEntry {
    User(UserMessage),
    Assistant(AssistantMessage),
    Error(String),
}

#[derive(Clone, Debug)]
pub struct UserMessage {
    pub body: String,
    pub meta: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AssistantMessage {
    pub parts: Vec<AssistantPart>,
}

#[derive(Clone, Debug)]
pub enum AssistantPart {
    Text(TextPart),
    Tool(ToolPart),
    Reasoning(ReasoningPart),
}

#[derive(Clone, Debug)]
pub struct TextPart {
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct ReasoningPart {
    pub summary: String,
    pub content: String,
    pub collapsed: bool,
}

#[derive(Clone, Debug)]
pub struct ToolPart {
    pub name: String,
    pub summary: String,
    pub body: Option<String>,
    pub state: ToolState,
    pub presentation: ToolPresentation,
    pub collapsed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Completed,
    WaitingApproval,
    Failed,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPresentation {
    Inline,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollMode {
    Auto,
    Manual(usize),
}

pub struct ContentPanel {
    entries: Vec<HistoryEntry>,
    stream_buffer: String,
    streaming_visible: String,
    scroll: ScrollMode,
}

impl ContentPanel {
    pub fn new(entries: Vec<HistoryEntry>) -> Self {
        Self {
            entries,
            stream_buffer: String::new(),
            streaming_visible: String::new(),
            scroll: ScrollMode::Auto,
        }
    }

    pub fn id(&self) -> PanelId {
        PanelId::Content
    }

    pub fn visible(&self, _ctx: &UiContext<'_>) -> bool {
        true
    }

    pub fn focusable(&self, _ctx: &UiContext<'_>) -> bool {
        true
    }

    pub fn window_spec(&self, _ctx: &UiContext<'_>) -> WindowSpec {
        WindowSpec { id: self.id(), z_index: 0, slot: WindowSlot::Content, size: 0 }
    }

    pub fn handle_key(&mut self, key: KeyAction, _ctx: &PanelContext<'_>) -> DispatchResult {
        let up_step = match key {
            KeyAction::Up => Some(1),
            KeyAction::PageUp => Some(20),
            KeyAction::Home => {
                self.scroll = ScrollMode::Manual(100_000);
                return DispatchResult::Consumed(Vec::new());
            }
            _ => None,
        };
        if let Some(step) = up_step {
            self.scroll = match self.scroll {
                ScrollMode::Auto => ScrollMode::Manual(step),
                ScrollMode::Manual(current) => {
                    ScrollMode::Manual(current.saturating_add(step).min(100_000))
                }
            };
            return DispatchResult::Consumed(Vec::new());
        }

        let down_step = match key {
            KeyAction::Down => Some(1),
            KeyAction::PageDown => Some(20),
            KeyAction::End => {
                self.scroll = ScrollMode::Auto;
                return DispatchResult::Consumed(Vec::new());
            }
            KeyAction::CtrlL => {
                return DispatchResult::Consumed(Vec::new());
            }
            _ => None,
        };
        if let Some(step) = down_step {
            self.scroll = match self.scroll {
                ScrollMode::Auto => ScrollMode::Auto,
                ScrollMode::Manual(current) => {
                    let next = current.saturating_sub(step);
                    if next == 0 {
                        ScrollMode::Auto
                    } else {
                        ScrollMode::Manual(next)
                    }
                }
            };
            return DispatchResult::Consumed(Vec::new());
        }

        DispatchResult::Ignored
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext<'_>) {
        let block = panel_block("Conversation", ctx.focused == Some(self.id()));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = self.flatten(inner.height as usize, inner.width as usize, ctx.exec_state);
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(TXT).bg(BG))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }

    pub fn push_user(&mut self, text: String) {
        self.entries.push(HistoryEntry::User(UserMessage { body: text, meta: None }));
        self.scroll = ScrollMode::Auto;
    }

    pub fn push_assistant(&mut self, text: String) {
        self.entries
            .push(HistoryEntry::Assistant(AssistantMessage::from_text(text)));
    }

    pub fn push_error(&mut self, text: String) {
        self.entries.push(HistoryEntry::Error(text));
    }

    pub fn begin_stream(&mut self) {
        self.stream_buffer.clear();
        self.streaming_visible.clear();
        self.scroll = ScrollMode::Auto;
    }

    pub fn append_stream_chunk(&mut self, chunk: &str) {
        self.stream_buffer.push_str(chunk);
        self.streaming_visible.push_str(chunk);
    }

    pub fn finalize_stream(&mut self) {
        let final_text = std::mem::take(&mut self.stream_buffer);
        self.streaming_visible.clear();
        if final_text.trim().is_empty() {
            return;
        }

        self.entries.push(HistoryEntry::Assistant(parse_assistant_message(
            &final_text,
            stream_tool_state(None),
        )));
    }

    fn flatten(
        &self,
        visible: usize,
        width: usize,
        exec_state: &ExecState,
    ) -> Vec<Line<'static>> {
        let mut all = Vec::new();

        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                all.push(blank_line(width));
            }
            render_history_entry(entry, width, &mut all);
        }

        if !self.streaming_visible.trim().is_empty() {
            if !all.is_empty() {
                all.push(blank_line(width));
            }
            let streaming = HistoryEntry::Assistant(parse_assistant_message(
                &self.streaming_visible,
                stream_tool_state(Some(exec_state)),
            ));
            render_history_entry(&streaming, width, &mut all);
        }

        if all.is_empty() {
            all.push(surface_line(
                vec![Span::styled(
                    "  Ask for a change, inspect a file, or run a project task.",
                    Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::DIM),
                )],
                width,
            ));
        }

        let total = all.len();
        let skip = match self.scroll {
            ScrollMode::Auto => 0,
            ScrollMode::Manual(lines) => lines.min(total.saturating_sub(1)),
        };
        let start = total.saturating_sub(skip.saturating_add(visible));
        all.into_iter()
            .skip(start)
            .take(visible.saturating_add(skip))
            .collect()
    }
}

impl AssistantMessage {
    fn from_text(text: String) -> Self {
        Self {
            parts: vec![AssistantPart::Text(TextPart { content: text })],
        }
    }
}

fn render_history_entry(entry: &HistoryEntry, width: usize, out: &mut Vec<Line<'static>>) {
    match entry {
        HistoryEntry::User(message) => render_user_message(message, width, out),
        HistoryEntry::Assistant(message) => render_assistant_message(message, width, out),
        HistoryEntry::Error(message) => render_error_message(message, width, out),
    }
}

fn render_user_message(message: &UserMessage, width: usize, out: &mut Vec<Line<'static>>) {
    out.push(card_line(
        vec![
            Span::styled("▎", Style::default().fg(BLUE).bg(CARD_BG)),
            Span::styled(
                " you",
                Style::default().fg(BLUE).bg(CARD_BG).add_modifier(Modifier::BOLD),
            ),
        ],
        width,
    ));

    push_markdown_lines(out, render::render_md(&message.body), width, card_markdown_line);

    if let Some(meta) = &message.meta {
        out.push(card_line(
            vec![
                Span::styled("▎", Style::default().fg(BLUE).bg(CARD_BG)),
                Span::styled(
                    format!(" {}", meta),
                    Style::default()
                        .fg(TXT_SUBTLE)
                        .bg(CARD_BG)
                        .add_modifier(Modifier::DIM),
                ),
            ],
            width,
        ));
    }
}

fn render_assistant_message(message: &AssistantMessage, width: usize, out: &mut Vec<Line<'static>>) {
    let mut first = true;
    for part in &message.parts {
        if !first {
            out.push(blank_line(width));
        }
        first = false;

        match part {
            AssistantPart::Text(part) => render_text_part(part, width, out),
            AssistantPart::Tool(part) => render_tool_part(part, width, out),
            AssistantPart::Reasoning(part) => render_reasoning_part(part, width, out),
        }
    }
}

fn render_text_part(part: &TextPart, width: usize, out: &mut Vec<Line<'static>>) {
    push_markdown_lines(out, render::render_md(&part.content), width, text_markdown_line);
}

fn render_tool_part(part: &ToolPart, width: usize, out: &mut Vec<Line<'static>>) {
    match part.presentation {
        ToolPresentation::Inline => {
            out.push(tool_row_line(part, width));
        }
        ToolPresentation::Block => {
            let (icon, title_style, border_color) = tool_state_visual(part.state);
            out.push(block_line(
                vec![
                    Span::styled("▎", Style::default().fg(border_color).bg(BLOCK_BG)),
                    Span::styled(
                        format!(" {} {}", icon, part.summary),
                        title_style.bg(BLOCK_BG).add_modifier(Modifier::BOLD),
                    ),
                ],
                width,
            ));

            if let Some(body) = &part.body {
                let body_lines = collapse_lines(body, part.collapsed);
                for raw in body_lines {
                    out.push(block_line(
                        vec![
                            Span::styled("▎", Style::default().fg(border_color).bg(BLOCK_BG)),
                            Span::styled(
                                format!(" {}", raw),
                                Style::default().fg(TXT).bg(BLOCK_BG),
                            ),
                        ],
                        width,
                    ));
                }
            }
        }
    }
}

fn render_reasoning_part(part: &ReasoningPart, width: usize, out: &mut Vec<Line<'static>>) {
    out.push(block_line(
        vec![
            Span::styled("▎", Style::default().fg(BORDER).bg(BLOCK_BG)),
            Span::styled(
                format!(" reasoning {}", part.summary),
                Style::default()
                    .fg(TXT_SUBTLE)
                    .bg(BLOCK_BG)
                    .add_modifier(Modifier::DIM),
            ),
        ],
        width,
    ));

    if !part.collapsed {
        for raw in collapse_lines(&part.content, false) {
            out.push(block_line(
                vec![
                    Span::styled("▎", Style::default().fg(BORDER).bg(BLOCK_BG)),
                    Span::styled(
                        format!(" {}", raw),
                        Style::default()
                            .fg(TXT_SUBTLE)
                            .bg(BLOCK_BG)
                            .add_modifier(Modifier::DIM),
                    ),
                ],
                width,
            ));
        }
    }
}

fn render_error_message(message: &str, width: usize, out: &mut Vec<Line<'static>>) {
    out.push(block_line(
        vec![
            Span::styled("▎", Style::default().fg(RED).bg(BLOCK_BG)),
            Span::styled(
                format!(" error {}", message),
                Style::default().fg(RED).bg(BLOCK_BG),
            ),
        ],
        width,
    ));
}

fn text_markdown_line(line: Line<'_>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "  ",
        Style::default().fg(TXT).bg(BG),
    )];
    spans.extend(
        line.spans
            .iter()
            .map(|span| Span::styled(span.content.to_string(), retint_style(span.style, BG, TXT))),
    );
    surface_line(spans, width)
}

fn tool_row_line(part: &ToolPart, width: usize) -> Line<'static> {
    let (icon, summary_style, status_color) = tool_state_visual(part.state);
    let status_text = match part.state {
        ToolState::Running => "running",
        ToolState::Completed => "done",
        ToolState::WaitingApproval => "needs approval",
        ToolState::Failed => "failed",
        ToolState::Denied => "denied",
    };

    surface_line(
        vec![
            Span::styled(
                format!("{:>2}", icon),
                Style::default().fg(status_color).bg(BG),
            ),
            Span::styled(" ", Style::default().bg(BG)),
            Span::styled(part.summary.clone(), summary_style.bg(BG)),
            Span::styled("  ", Style::default().bg(BG)),
            Span::styled(
                status_text,
                Style::default()
                    .fg(status_color)
                    .bg(BG)
                    .add_modifier(Modifier::DIM),
            ),
        ],
        width,
    )
}

fn tool_state_visual(state: ToolState) -> (&'static str, Style, Color) {
    match state {
        ToolState::Running => ("◌", Style::default().fg(ORANGE), ORANGE),
        ToolState::Completed => ("●", Style::default().fg(CYAN), CYAN),
        ToolState::WaitingApproval => ("!", Style::default().fg(ORANGE), ORANGE),
        ToolState::Failed => ("×", Style::default().fg(RED), RED),
        ToolState::Denied => ("-", Style::default().fg(RED), RED),
    }
}

fn parse_assistant_message(content: &str, current_tool_state: ToolState) -> AssistantMessage {
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

    let raw = line
        .split(marker)
        .nth(1)
        .unwrap_or("")
        .trim();
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

fn classify_tool_state(summary: &str, body: Option<&str>, current_tool_state: ToolState) -> ToolState {
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

fn choose_tool_presentation(summary: &str, body: Option<&str>, state: ToolState) -> ToolPresentation {
    if state == ToolState::Running || state == ToolState::WaitingApproval {
        return ToolPresentation::Inline;
    }

    let name = tool_name_from_summary(summary);
    if matches!(name.as_str(), "cargo" | "cargo_check" | "get_diagnostics" | "write_file") {
        return ToolPresentation::Block;
    }

    let Some(body) = body else {
        return ToolPresentation::Inline;
    };

    let line_count = body.lines().count();
    if body.contains("```")
        || body.lines().any(|line| line.starts_with('+') || line.starts_with('-') || line.contains(" | "))
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

fn stream_tool_state(exec_state: Option<&ExecState>) -> ToolState {
    match exec_state {
        Some(ExecState::AwaitingApproval { .. }) => ToolState::WaitingApproval,
        Some(ExecState::ToolRunning { .. }) | Some(ExecState::Streaming { .. }) => ToolState::Running,
        Some(ExecState::Blocked { reason, .. }) if reason.to_ascii_lowercase().contains("denied") => {
            ToolState::Denied
        }
        _ => ToolState::Completed,
    }
}

fn collapse_lines(body: &str, collapsed: bool) -> Vec<String> {
    let lines = body.lines().map(|line| line.to_string()).collect::<Vec<_>>();
    if !collapsed || lines.len() <= 6 {
        return lines;
    }

    let mut visible = lines.into_iter().take(5).collect::<Vec<_>>();
    visible.push("...".into());
    visible
}

fn blank_line(width: usize) -> Line<'static> {
    surface_line(vec![], width)
}

fn surface_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = spans;
    let used = spans.iter().map(|span| span.content.chars().count()).sum::<usize>();
    let padding = width.saturating_sub(used);
    spans.push(Span::styled(
        " ".repeat(padding),
        Style::default().bg(BG),
    ));
    Line::from(spans)
}

fn card_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    padded_line(spans, width, CARD_BG)
}

fn block_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    padded_line(spans, width, BLOCK_BG)
}

fn padded_line(mut spans: Vec<Span<'static>>, width: usize, bg: Color) -> Line<'static> {
    let used = spans.iter().map(|span| span.content.chars().count()).sum::<usize>();
    let padding = width.saturating_sub(used);
    spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
    Line::from(spans)
}

fn card_markdown_line(line: Line<'_>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled("▎", Style::default().fg(BLUE).bg(CARD_BG))];
    if line.spans.is_empty() {
        spans.push(Span::styled(" ", Style::default().bg(CARD_BG)));
        return card_line(spans, width);
    }

    spans.push(Span::styled(" ", Style::default().bg(CARD_BG)));
    spans.extend(
        line.spans
            .iter()
            .map(|span| Span::styled(span.content.to_string(), retint_style(span.style, CARD_BG, TXT))),
    );
    card_line(spans, width)
}

fn retint_style(style: Style, bg: Color, default_fg: Color) -> Style {
    style.bg(bg).fg(style.fg.unwrap_or(default_fg))
}

fn push_markdown_lines<F>(
    out: &mut Vec<Line<'static>>,
    lines: Vec<Line<'_>>,
    width: usize,
    mut map_line: F,
) where
    F: FnMut(Line<'_>, usize) -> Line<'static>,
{
    for line in lines {
        if is_blank_markdown_line(&line) {
            continue;
        }
        out.push(map_line(line, width));
    }
}

fn is_blank_markdown_line(line: &Line<'_>) -> bool {
    line.spans
        .iter()
        .all(|span| span.content.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_stream_builds_structured_assistant_message() {
        let mut panel = ContentPanel::new(Vec::new());
        panel.begin_stream();
        panel.append_stream_chunk("hello\n\n  ⟳ read_file(src/main.rs)\n   1 | fn main() {}\n");
        panel.finalize_stream();

        let HistoryEntry::Assistant(message) = &panel.entries[0] else {
            panic!("expected assistant entry");
        };

        assert!(matches!(message.parts[0], AssistantPart::Text(_)));
        assert!(matches!(message.parts[1], AssistantPart::Tool(_)));
    }

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
}
