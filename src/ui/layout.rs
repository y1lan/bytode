use crate::tools::ToolResult;
use crate::ui::{render, statusbar};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

#[derive(Clone)]
pub struct UiState {
    pub sidebar_visible: bool,
    pub tool_names: Vec<String>,
    pub status: statusbar::StatusBarState,
    pub tool_result: Option<ToolResult>,
    pub llm_output: Option<String>,
    pub user_input: String,
    pub approval: Option<(String, String)>,
    pub primary_language: String,
    pub detection_source: String,
}

pub fn render_ui(frame: &mut Frame, state: &UiState) {
    let main_area = frame.area();

    // White background fills the entire terminal
    let bg = Block::default().style(Style::default().bg(Color::White));
    frame.render_widget(bg, main_area);

    let (content_area, sidebar_area) = if state.sidebar_visible {
        let chunks = Layout::horizontal([Constraint::Min(40), Constraint::Length(24)])
            .split(main_area);
        (chunks[0], Some(chunks[1]))
    } else {
        (main_area, None)
    };

    let [scroll_area, status_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(content_area);

    let [output_area, input_area] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(1)]).areas(scroll_area);

    // Content area also white
    let content_bg = Block::default().style(Style::default().bg(Color::White));
    frame.render_widget(content_bg, scroll_area);

    if let Some(ref result) = state.tool_result {
        render::tool_result(frame, output_area, result);
    } else if let Some(ref text) = state.llm_output {
        render::llm_text(frame, output_area, text);
    }

    render::user_input(frame, input_area, &state.user_input);
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
                .fg(Color::Rgb(30, 30, 130))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for name in &state.tool_names {
        let (icon, color) = match name.as_str() {
            "read_file" => ("📖", Color::Rgb(0, 120, 0)),
            "write_file" => ("✏️", Color::Rgb(180, 120, 0)),
            "search_code" => ("🔍", Color::Rgb(0, 80, 180)),
            "run_check" => ("🔧", Color::Rgb(150, 0, 150)),
            "get_diagnostics" => ("💉", Color::Rgb(180, 40, 40)),
            _ => ("•", Color::Gray),
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", icon), Style::default()),
            Span::styled(name, Style::default().fg(color)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "─".repeat(22),
        Style::default().fg(Color::Rgb(200, 200, 210)),
    )));
    lines.push(Line::from(vec![
        Span::styled(" L: ", Style::default().fg(Color::Rgb(140, 140, 150))),
        Span::styled(
            &state.primary_language,
            Style::default().fg(Color::Rgb(50, 50, 60)),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(" D: ", Style::default().fg(Color::Rgb(140, 140, 150))),
        Span::styled(
            &state.detection_source,
            Style::default().fg(Color::Rgb(100, 100, 110)),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ctrl+t toggle",
        Style::default().fg(Color::Rgb(180, 180, 190)),
    )));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(Color::Rgb(220, 220, 230)))
                .style(Style::default().bg(Color::White)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}
