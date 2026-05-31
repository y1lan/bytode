use crate::tools::ToolResult;
use crate::ui::{render, statusbar};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

#[derive(Clone, Debug)]
pub enum HistoryEntry {
    User(String),
    Assistant(String),
    Tool { name: String, summary: String },
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollMode {
    Auto,
    Manual(usize),
}

#[derive(Clone)]
pub struct UiState {
    pub sidebar_visible: bool,
    pub tool_names: Vec<String>,
    pub status: statusbar::StatusBarState,
    pub tool_result: Option<ToolResult>,
    pub entries: Vec<HistoryEntry>,
    pub streaming: Option<String>,
    pub scroll: ScrollMode,
    pub user_input: String,
    pub pending: Option<String>,
    pub approval: Option<(String, String)>,
    pub primary_language: String,
    pub detection_source: String,
}

pub fn render_ui(frame: &mut Frame, state: &UiState) {
    let main_area = frame.area();
    let bg = Block::default().style(Style::default().bg(Color::Rgb(239, 241, 245)));
    frame.render_widget(bg, main_area);

    let (content_area, sidebar_area) = if state.sidebar_visible {
        let chunks = Layout::horizontal([Constraint::Min(40), Constraint::Length(24)]).split(main_area);
        (chunks[0], Some(chunks[1]))
    } else {
        (main_area, None)
    };

    let [scroll_area_raw, status_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(content_area);

    let scroll_area = Rect::new(
        scroll_area_raw.x + 4, scroll_area_raw.y + 1,
        scroll_area_raw.width.saturating_sub(8), scroll_area_raw.height.saturating_sub(2),
    );

    let [output_area, input_area_raw] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(1)]).areas(scroll_area);

    if let Some(ref result) = state.tool_result {
        render::tool_result(frame, output_area, result);
    } else {
        let lines = flatten(state, output_area.height as usize);
        let p = Paragraph::new(lines)
            .style(Style::default().fg(Color::Rgb(76, 79, 105)))
            .wrap(Wrap { trim: false });
        frame.render_widget(p, output_area);
    }

    render::user_input(frame, input_area_raw, &state.user_input);
    if let Some(ref pending) = state.pending {
        let pending_area = Rect::new(input_area_raw.x, input_area_raw.y.saturating_sub(1), input_area_raw.width, 1);
        let line = Line::from(Span::styled(
            format!("  [pending] {pending}"),
            Style::default().fg(Color::Rgb(223, 142, 29)).add_modifier(Modifier::DIM),
        ));
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(Color::Rgb(239, 241, 245))), pending_area);
    }
    statusbar::render(frame, status_area, &state.status);
    if let Some(area) = sidebar_area { render_sidebar(frame, area, state); }
    if let Some((ref path, ref diff)) = state.approval { render::approve(frame, path, diff, 30); }
}

fn flatten(state: &UiState, visible: usize) -> Vec<Line<'static>> {
    let mut all: Vec<Line<'static>> = Vec::new();

    for e in &state.entries {
        match e {
            HistoryEntry::User(msg) => {
                all.push(Line::from(Span::styled(
                    format!("   \u{25b8} {msg}"),
                    Style::default().fg(Color::Rgb(30, 102, 245)).add_modifier(Modifier::BOLD),
                )));
            }
            HistoryEntry::Assistant(text) => {
                for line in render::render_md(text) {
                    all.push(own(line));
                }
            }
            HistoryEntry::Tool { name, summary } => {
                all.push(Line::from(Span::styled(
                    format!("   {name}(...)"),
                    Style::default().fg(Color::Rgb(32, 159, 181)).add_modifier(Modifier::DIM),
                )));
                if !summary.is_empty() {
                    for line in render::render_md(summary) { all.push(own(line)); }
                }
            }
            HistoryEntry::Error(msg) => {
                all.push(Line::from(Span::styled(
                    format!("   {msg}"),
                    Style::default().fg(Color::Rgb(210, 15, 57)),
                )));
            }
        }
        all.push(Line::from(Span::styled(
            "\u{2500}".repeat(60),
            Style::default().fg(Color::Rgb(204, 208, 218)),
        )));
    }

    if let Some(s) = &state.streaming {
        for line in render::render_md(s) { all.push(own(line)); }
    }

    let total = all.len();
    let skip = match state.scroll {
        ScrollMode::Auto => 0,
        ScrollMode::Manual(n) => n.min(total.saturating_sub(1)),
    };
    let start = total.saturating_sub(skip.saturating_add(visible));
    all.into_iter().skip(start).take(visible.saturating_add(skip)).collect()
}

fn own(line: ratatui::text::Line<'_>) -> Line<'static> {
    Line::from(line.spans.iter().map(|s| Span::styled(s.content.to_string(), s.style)).collect::<Vec<_>>())
}

fn render_sidebar(frame: &mut Frame, area: Rect, state: &UiState) {
    let mut lines = vec![
        Line::from(Span::styled(" Tools", Style::default().fg(Color::Rgb(30, 102, 245)).add_modifier(Modifier::BOLD))),
        Line::from(""),
    ];
    for name in &state.tool_names {
        let color = match name.as_str() {
            "read_file" => Color::Rgb(64, 160, 43),
            "write_file" => Color::Rgb(223, 142, 29),
            "search_code" => Color::Rgb(32, 159, 181),
            "cargo_check" => Color::Rgb(136, 57, 239),
            "get_diagnostics" => Color::Rgb(210, 15, 57),
            _ => Color::Rgb(156, 160, 176),
        };
        lines.push(Line::from(Span::styled(format!(" {name}"), Style::default().fg(color))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("\u{2500}".repeat(22), Style::default().fg(Color::Rgb(172, 176, 190)))));
    lines.push(Line::from(vec![
        Span::styled(" L: ", Style::default().fg(Color::Rgb(140, 143, 161))),
        Span::styled(&state.primary_language, Style::default().fg(Color::Rgb(76, 79, 105))),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" D: ", Style::default().fg(Color::Rgb(140, 143, 161))),
        Span::styled(&state.detection_source, Style::default().fg(Color::Rgb(108, 111, 133))),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" ctrl+t toggle", Style::default().fg(Color::Rgb(156, 160, 176)))));
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::Rgb(188, 192, 204)))
        .style(Style::default().bg(Color::Rgb(239, 241, 245))))
        .wrap(Wrap { trim: false });
    frame.render_widget(p, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::statusbar::StatusBarState;

    fn mk(entries: Vec<HistoryEntry>, scroll: ScrollMode, streaming: Option<String>) -> UiState {
        UiState {
            sidebar_visible: false, tool_names: vec![],
            status: StatusBarState {
                dir: String::new(), git_branch: None, git_dirty: false,
                task: String::new(), model: String::new(),
                ctx_used: 0, ctx_total: 0, tool_count: 0,
                session_cost: 0.0, session_calls: 0,
                mode: String::new(), elapsed: String::new(), spinner: ' ',
            },
            tool_result: None, entries, streaming, scroll,
            user_input: String::new(), pending: None, approval: None,
            primary_language: String::new(), detection_source: String::new(),
        }
    }

    fn txt(lines: &[Line]) -> String {
        lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect()
    }

    #[test]
    fn auto_shows_bottom() {
        let e = vec![HistoryEntry::User("a".into()), HistoryEntry::Assistant("b".into())];
        let lines = flatten(&mk(e, ScrollMode::Auto, None), 10);
        assert!(txt(&lines).contains("b"), "should show last entry");
    }

    #[test]
    fn manual_hides_content() {
        let mut e = Vec::new();
        for i in 0..5 { e.push(HistoryEntry::Assistant(format!("line {i}"))); }
        let a = flatten(&mk(e.clone(), ScrollMode::Auto, None), 3);
        let m = flatten(&mk(e, ScrollMode::Manual(6), None), 3);
        assert_ne!(txt(&a), txt(&m), "manual should show different content");
    }

    #[test]
    fn empty_entries() {
        assert!(flatten(&mk(vec![], ScrollMode::Auto, None), 10).is_empty());
    }

    #[test]
    fn user_has_arrow() {
        let e = vec![HistoryEntry::User("hello".into())];
        assert!(txt(&flatten(&mk(e, ScrollMode::Auto, None), 10)).contains("hello"));
    }

    #[test]
    fn streaming_appended() {
        let e = vec![HistoryEntry::Assistant("old".into())];
        let lines = flatten(&mk(e, ScrollMode::Auto, Some("new".into())), 10);
        let t = txt(&lines);
        assert!(t.contains("old") && t.contains("new"), "{t:?}");
    }

    #[test]
    fn manual_zero_equals_auto() {
        let e = vec![HistoryEntry::Assistant("x".into())];
        assert_eq!(
            txt(&flatten(&mk(e.clone(), ScrollMode::Auto, None), 5)),
            txt(&flatten(&mk(e, ScrollMode::Manual(0), None), 5)),
        );
    }
}
