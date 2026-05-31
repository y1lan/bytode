use crate::tools::ToolResult;
use crate::ui::{render, statusbar};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

#[derive(Clone)]
pub struct UiState {
    pub sidebar_visible: bool,
    pub tool_names: Vec<String>,
    pub status: statusbar::StatusBarState,
    pub tool_result: Option<ToolResult>,
    pub history: Vec<String>,
    pub streaming: Option<String>,
    pub scroll_offset: usize,
    pub user_input: String,
    pub approval: Option<(String, String)>,
    pub primary_language: String,
    pub detection_source: String,
}

pub fn render_ui(frame: &mut Frame, state: &UiState) {
    let main_area = frame.area();

    let bg = Block::default().style(Style::default().bg(Color::Rgb(239, 241, 245)));
    frame.render_widget(bg, main_area);

    let (content_area, sidebar_area) = if state.sidebar_visible {
        let chunks =
            Layout::horizontal([Constraint::Min(40), Constraint::Length(24)]).split(main_area);
        (chunks[0], Some(chunks[1]))
    } else {
        (main_area, None)
    };

    let [scroll_area_raw, status_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(content_area);

    let scroll_area = Rect::new(
        scroll_area_raw.x + 4,
        scroll_area_raw.y + 1,
        scroll_area_raw.width.saturating_sub(8),
        scroll_area_raw.height.saturating_sub(2),
    );

    let [output_area, input_area_raw] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(1)]).areas(scroll_area);

    if let Some(ref result) = state.tool_result {
        render::tool_result(frame, output_area, result);
    } else {
        let all = flatten_and_scroll(state, output_area.height as usize);
        let paragraph = Paragraph::new(all)
            .style(Style::default().fg(Color::Rgb(76, 79, 105)))
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, output_area);
    }

    render::user_input(frame, input_area_raw, &state.user_input);
    statusbar::render(frame, status_area, &state.status);

    if let Some(area) = sidebar_area {
        render_sidebar(frame, area, state);
    }

    if let Some((ref path, ref diff)) = state.approval {
        render::approve(frame, path, diff, 30);
    }
}

fn render_sidebar(frame: &mut Frame, area: Rect, state: &UiState) {
    let mut lines = vec![
        Line::from(Span::styled(
            " Tools",
            Style::default()
                .fg(Color::Rgb(30, 102, 245))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for name in &state.tool_names {
        let color = match name.as_str() {
            "read_file" => Color::Rgb(64, 160, 43),
            "write_file" => Color::Rgb(223, 142, 29),
            "search_code" => Color::Rgb(32, 159, 181),
            "run_check" => Color::Rgb(136, 57, 239),
            "get_diagnostics" => Color::Rgb(210, 15, 57),
            _ => Color::Rgb(156, 160, 176),
        };
        lines.push(Line::from(Span::styled(
            format!(" {}", name),
            Style::default().fg(color),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "\u{2500}".repeat(22),
        Style::default().fg(Color::Rgb(172, 176, 190)),
    )));
    lines.push(Line::from(vec![
        Span::styled(" L: ", Style::default().fg(Color::Rgb(140, 143, 161))),
        Span::styled(
            &state.primary_language,
            Style::default().fg(Color::Rgb(76, 79, 105)),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" D: ", Style::default().fg(Color::Rgb(140, 143, 161))),
        Span::styled(
            &state.detection_source,
            Style::default().fg(Color::Rgb(108, 111, 133)),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ctrl+t toggle",
        Style::default().fg(Color::Rgb(156, 160, 176)),
    )));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(Color::Rgb(188, 192, 204)))
                .style(Style::default().bg(Color::Rgb(239, 241, 245))),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn flatten_and_scroll(state: &UiState, visible: usize) -> Vec<Line<'static>> {
    let mut all: Vec<Line<'static>> = Vec::new();
    for msg in &state.history {
        if let Some(text) = msg.strip_prefix('\u{25b8}') {
            all.push(Line::from(Span::styled(
                format!("   \u{25b8} {text}"),
                Style::default().fg(Color::Rgb(30, 102, 245)).add_modifier(Modifier::BOLD),
            )));
        } else {
            for line in render::render_md(msg) {
                let owned: Vec<Span<'static>> = line.spans.iter().map(|s| {
                    Span::styled(s.content.to_string(), s.style)
                }).collect();
                all.push(Line::from(owned));
            }
        }
        // Visual separator between messages
        all.push(Line::from(Span::styled(" ", Style::default())));
    }
    if let Some(s) = &state.streaming {
        for line in render::render_md(s) {
            let owned: Vec<Span<'static>> = line.spans.iter().map(|s| {
                Span::styled(s.content.to_string(), s.style)
            }).collect();
            all.push(Line::from(owned));
        }
    }
    let total = all.len();
    let skip = state.scroll_offset.min(total.saturating_sub(1));
    let start = total.saturating_sub(skip + visible);
    all.into_iter().skip(start).take(visible + skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::statusbar::StatusBarState;

    fn mk(hist: Vec<String>, scroll: usize, streaming: Option<String>) -> UiState {
        UiState {
            sidebar_visible: false,
            tool_names: vec![],
            status: StatusBarState {
                dir: String::new(), git_branch: None, git_dirty: false,
                task: String::new(), model: String::new(),
                ctx_used: 0, ctx_total: 0, tool_count: 0,
                session_cost: 0.0, session_calls: 0,
                mode: String::new(), elapsed: String::new(), spinner: ' ',
            },
            tool_result: None, history: hist, streaming,
            scroll_offset: scroll, user_input: String::new(),
            approval: None, primary_language: String::new(),
            detection_source: String::new(),
        }
    }

    fn line_text(lines: &[Line]) -> String {
        lines.iter().flat_map(|l| l.spans.iter().map(|s| s.content.as_ref())).collect()
    }

    #[test]
    fn scroll_zero_shows_bottom() {
        let lines = flatten_and_scroll(&mk(vec!["a".into(), "b".into(), "c".into()], 0, None), 3);
        let t = line_text(&lines);
        assert!(t.contains("c"), "{t:?}");
    }

    #[test]
    fn scroll_offset_hides_bottom() {
        let l0 = flatten_and_scroll(&mk(vec!["a".into(), "b".into(), "c".into()], 0, None), 3);
        let l2 = flatten_and_scroll(&mk(vec!["a".into(), "b".into(), "c".into()], 4, None), 3);
        assert_ne!(line_text(&l0), line_text(&l2));
    }

    #[test]
    fn empty_history_is_empty() {
        assert!(flatten_and_scroll(&mk(vec![], 0, None), 10).is_empty());
    }

    #[test]
    fn user_message_has_arrow() {
        let lines = flatten_and_scroll(&mk(vec!["\u{25b8} hello".into()], 0, None), 10);
        let t = line_text(&lines);
        assert!(t.contains("hello"), "{t:?}");
    }

    #[test]
    fn streaming_appended() {
        let lines = flatten_and_scroll(&mk(vec!["old".into()], 0, Some("new".into())), 10);
        let t = line_text(&lines);
        assert!(t.contains("old") && t.contains("new"), "{t:?}");
    }

    #[test]
    fn huge_scroll_does_not_panic() {
        flatten_and_scroll(&mk(vec!["x".into()], 99999, None), 5);
    }

    #[test]
    fn visible_clipped_to_height() {
        let msgs: Vec<String> = (0..50).map(|i| format!("{i}")).collect();
        let lines = flatten_and_scroll(&mk(msgs, 0, None), 3);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn long_multiline_message_scrollable() {
        let long = "1\n2\n3\n4\n5\n6\n7\n8\n9";
        let lines = flatten_and_scroll(&mk(vec![long.into()], 3, None), 3);
        let t = line_text(&lines);
        assert!(!t.contains("   ") && !t.is_empty(), "scrolled: {t:?}");
    }
}
