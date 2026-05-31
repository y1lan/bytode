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
        let chunks = Layout::horizontal([Constraint::Min(40), Constraint::Length(24)])
            .split(main_area);
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

    // Flatten all messages into a single Vec<Line> with markdown rendering
    let mut all_lines: Vec<Line> = Vec::new();
    for msg in &state.history {
        let prefix = if msg.starts_with('\u{25b8}') { "\u{25b8} " } else { "" };
        let is_user = !prefix.is_empty();
        if is_user {
            let text = msg.strip_prefix("\u{25b8} ").unwrap_or(msg);
            all_lines.push(Line::from(Span::styled(
                format!("\u{25b8} {text}"),
                Style::default().fg(Color::Rgb(30, 102, 245)).add_modifier(Modifier::BOLD),
            )));
            all_lines.push(Line::from(""));
        } else {
            let rendered = render::render_md(msg);
            all_lines.extend(rendered);
            all_lines.push(Line::from(""));
        }
    }
    if let Some(s) = &state.streaming {
        let rendered = render::render_md(s);
        all_lines.extend(rendered);
    }

    // Line-based scroll: scroll_offset lines from the bottom
    let total = all_lines.len();
    let visible = output_area.height as usize;
    let skip = state.scroll_offset.min(total.saturating_sub(1));
    let start = total.saturating_sub(skip + visible);
    let slice: Vec<Line> = all_lines.into_iter().skip(start).take(visible + skip).collect();

    let paragraph = Paragraph::new(slice)
        .block(Block::default())
        .style(Style::default().fg(Color::Rgb(76, 79, 105)))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, output_area);

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
