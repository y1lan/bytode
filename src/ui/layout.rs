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

    // Global 4-char padding around content
    let scroll_area = Rect::new(
        scroll_area_raw.x + 4,
        scroll_area_raw.y + 1,
        scroll_area_raw.width.saturating_sub(8),
        scroll_area_raw.height.saturating_sub(2),
    );

    let [output_area_raw, input_area_raw] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(1)]).areas(scroll_area);

    // Output area with horizontal padding for message blocks
    let output_area = Rect::new(
        output_area_raw.x,
        output_area_raw.y,
        output_area_raw.width,
        output_area_raw.height,
    );

    let content_bg = Block::default().style(Style::default().bg(Color::Rgb(239, 241, 245)));
    frame.render_widget(content_bg, scroll_area);

    if let Some(ref result) = state.tool_result {
        render::tool_result(frame, output_area, result);
    } else {
        let _y = output_area.y;

        // Collect blocks with is_user flag
        let streaming = state.streaming.as_deref();
        let mut blocks: Vec<(&str, bool)> = state.history.iter().map(|s| (s.as_str(), false)).collect();
        for (text, is_user) in &mut blocks {
            if text.starts_with('\u{25b8}') || text.starts_with('\u{203a}') {
                *is_user = true;
            }
        }
        if let Some(s) = streaming {
            blocks.push((s, false));
        }

        // Render from the LAST block backward
        let visible_rows = output_area.height;
        let mut y = output_area.y + output_area.height;
        let mut first_visible = blocks.len();
        let mut skip = state.scroll_offset as u16;
        let mut partial = 0u16;
        let mut need = visible_rows;

        for i in (0..blocks.len()).rev() {
            let h = (blocks[i].0.lines().count() + 2) as u16;
            if skip > 0 {
                if h <= skip {
                    skip -= h;
                    continue;
                }
                partial = skip;
                skip = 0;
            }
            if need == 0 {
                break;
            }
            first_visible = i;
            need = need.saturating_sub(h);
            y = y.saturating_sub(h);
        }

        // Render from first_visible to end
        for i in first_visible..blocks.len() {
            if y >= output_area.y + output_area.height {
                break;
            }

            let (text, is_user) = blocks[i];
            let mut lines: Vec<&str> = text.lines().collect();
            let total_block_height = (lines.len() + 2) as u16;

            // Apply partial skip
            if partial > 0 {
                let skip_lines = partial.saturating_sub(1).min(lines.len() as u16) as usize;
                if skip_lines > 0 {
                    lines = lines[skip_lines..].to_vec();
                }
                partial = 0;
            }

            let remaining = (output_area.y + output_area.height).saturating_sub(y);
            let avail = (remaining as usize).saturating_sub(2);
            let show_lines: Vec<&str> = if lines.len() > avail {
                lines[..avail].to_vec()
            } else {
                lines
            };

            if show_lines.is_empty() {
                y = y.saturating_add(total_block_height);
                continue;
            }

            let content = show_lines.join("\n");
            let block_height = (show_lines.len() + 2) as u16;
            let block_area = Rect::new(output_area.x, y, output_area.width, block_height.min(output_area.height));
            render::message_block(frame, block_area, &content, is_user);
            y = y.saturating_add(block_height);
        }
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
        "─".repeat(22),
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
