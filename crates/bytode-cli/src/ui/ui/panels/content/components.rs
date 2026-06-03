use super::model::{
    AssistantMessage, AssistantPart, HistoryEntry, ReasoningPart, TextPart, ToolPart, ToolState,
    UserMessage,
};
use crate::ui::panels::{BG, BLUE, BORDER, CYAN, ORANGE, RED, TXT, TXT_SUBTLE};
use crate::ui::render;
use ratatui::prelude::*;

const CARD_BG: Color = Color::Rgb(230, 233, 239);
const BLOCK_BG: Color = Color::Rgb(220, 224, 232);

pub(crate) trait TranscriptComponent<T> {
    fn render(&self, value: &T, width: usize, out: &mut Vec<Line<'static>>);
}

pub(crate) struct UserCard;
pub(crate) struct AssistantTextBlock;
pub(crate) struct ToolPartBlock;
pub(crate) struct ReasoningBlock;
pub(crate) struct ErrorBlock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentHitTarget {
    Tool {
        entry_index: usize,
        part_index: usize,
    },
    Reasoning {
        entry_index: usize,
        part_index: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContentHitRegion {
    pub line: usize,
    pub target: ContentHitTarget,
}

impl TranscriptComponent<UserMessage> for UserCard {
    fn render(&self, value: &UserMessage, width: usize, out: &mut Vec<Line<'static>>) {
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

        push_markdown_lines(out, render::render_md(&value.body), width, card_markdown_line);

        if let Some(meta) = &value.meta {
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
}

impl TranscriptComponent<TextPart> for AssistantTextBlock {
    fn render(&self, value: &TextPart, width: usize, out: &mut Vec<Line<'static>>) {
        push_markdown_lines(out, render::render_md(&value.content), width, text_markdown_line);
    }
}

impl TranscriptComponent<ToolPart> for ToolPartBlock {
    fn render(&self, value: &ToolPart, width: usize, out: &mut Vec<Line<'static>>) {
        match value.presentation {
            super::model::ToolPresentation::Inline => out.push(tool_row_line(value, width)),
            super::model::ToolPresentation::Block => {
                let (icon, title_style, border_color) = tool_state_visual(value.state);
                out.push(block_line(
                    vec![
                        Span::styled("▎", Style::default().fg(border_color).bg(BLOCK_BG)),
                        Span::styled(
                            format!(" {} {}", icon, value.summary),
                            title_style.bg(BLOCK_BG).add_modifier(Modifier::BOLD),
                        ),
                    ],
                    width,
                ));

                if let Some(body) = &value.body {
                    for raw in collapse_lines(body, value.collapsed) {
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
}

impl TranscriptComponent<ReasoningPart> for ReasoningBlock {
    fn render(&self, value: &ReasoningPart, width: usize, out: &mut Vec<Line<'static>>) {
        out.push(block_line(
            vec![
                Span::styled("▎", Style::default().fg(BORDER).bg(BLOCK_BG)),
                Span::styled(
                    format!(" reasoning {}", value.summary),
                    Style::default()
                        .fg(TXT_SUBTLE)
                        .bg(BLOCK_BG)
                        .add_modifier(Modifier::DIM),
                ),
            ],
            width,
        ));

        if !value.collapsed {
            for raw in collapse_lines(&value.content, false) {
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
}

impl TranscriptComponent<String> for ErrorBlock {
    fn render(&self, value: &String, width: usize, out: &mut Vec<Line<'static>>) {
        out.push(block_line(
            vec![
                Span::styled("▎", Style::default().fg(RED).bg(BLOCK_BG)),
                Span::styled(
                    format!(" error {}", value),
                    Style::default().fg(RED).bg(BLOCK_BG),
                ),
            ],
            width,
        ));
    }
}

pub(crate) fn render_history_entry(entry: &HistoryEntry, width: usize, out: &mut Vec<Line<'static>>) {
    match entry {
        HistoryEntry::User(message) => UserCard.render(message, width, out),
        HistoryEntry::Assistant(message) => render_assistant_message(message, width, out),
        HistoryEntry::Error(message) => ErrorBlock.render(message, width, out),
    }
}

pub(crate) fn render_history_entry_with_hits(
    entry: &HistoryEntry,
    entry_index: usize,
    width: usize,
    out: &mut Vec<Line<'static>>,
    hits: &mut Vec<ContentHitRegion>,
) {
    match entry {
        HistoryEntry::Assistant(message) => {
            render_assistant_message_with_hits(message, entry_index, width, out, hits)
        }
        _ => render_history_entry(entry, width, out),
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
            AssistantPart::Text(part) => AssistantTextBlock.render(part, width, out),
            AssistantPart::Tool(part) => ToolPartBlock.render(part, width, out),
            AssistantPart::Reasoning(part) => ReasoningBlock.render(part, width, out),
        }
    }
}

fn render_assistant_message_with_hits(
    message: &AssistantMessage,
    entry_index: usize,
    width: usize,
    out: &mut Vec<Line<'static>>,
    hits: &mut Vec<ContentHitRegion>,
) {
    let mut first = true;
    for (part_index, part) in message.parts.iter().enumerate() {
        if !first {
            out.push(blank_line(width));
        }
        first = false;

        let header_line = out.len();
        match part {
            AssistantPart::Text(part) => AssistantTextBlock.render(part, width, out),
            AssistantPart::Tool(part) => {
                ToolPartBlock.render(part, width, out);
                if matches!(part.presentation, super::model::ToolPresentation::Block)
                    && part.body.is_some()
                {
                    hits.push(ContentHitRegion {
                        line: header_line,
                        target: ContentHitTarget::Tool {
                            entry_index,
                            part_index,
                        },
                    });
                }
            }
            AssistantPart::Reasoning(part) => {
                ReasoningBlock.render(part, width, out);
                hits.push(ContentHitRegion {
                    line: header_line,
                    target: ContentHitTarget::Reasoning {
                        entry_index,
                        part_index,
                    },
                });
            }
        }
    }
}

pub(crate) fn blank_line(width: usize) -> Line<'static> {
    surface_line(vec![], width)
}

pub(crate) fn surface_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = spans;
    let used = Line::from(spans.clone()).width();
    let padding = width.saturating_sub(used);
    spans.push(Span::styled(" ".repeat(padding), Style::default().bg(BG)));
    Line::from(spans)
}

fn card_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    padded_line(spans, width, CARD_BG)
}

fn block_line(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    padded_line(spans, width, BLOCK_BG)
}

fn padded_line(mut spans: Vec<Span<'static>>, width: usize, bg: Color) -> Line<'static> {
    let used = Line::from(spans.clone()).width();
    let padding = width.saturating_sub(used);
    spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
    Line::from(spans)
}

fn card_markdown_line(line: Line<'_>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled("▎", Style::default().fg(BLUE).bg(CARD_BG))];
    spans.push(Span::styled(" ", Style::default().bg(CARD_BG)));
    spans.extend(
        line.spans
            .iter()
            .map(|span| Span::styled(span.content.to_string(), retint_style(span.style, CARD_BG, TXT))),
    );
    card_line(spans, width)
}

fn text_markdown_line(line: Line<'_>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled("  ", Style::default().fg(TXT).bg(BG))];
    spans.extend(
        line.spans
            .iter()
            .map(|span| Span::styled(span.content.to_string(), retint_style(span.style, BG, TXT))),
    );
    surface_line(spans, width)
}

fn retint_style(style: Style, bg: Color, default_fg: Color) -> Style {
    style.bg(bg).fg(style.fg.unwrap_or(default_fg))
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

fn collapse_lines(body: &str, collapsed: bool) -> Vec<String> {
    let lines = body.lines().map(|line| line.to_string()).collect::<Vec<_>>();
    if !collapsed || lines.len() <= 6 {
        return lines;
    }

    let mut visible = lines.into_iter().take(5).collect::<Vec<_>>();
    visible.push("...".into());
    visible
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
    line.spans.iter().all(|span| span.content.trim().is_empty())
}
