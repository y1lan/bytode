use crate::tools::ToolResult;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn tool_result(frame: &mut Frame, area: Rect, result: &ToolResult) {
    let (title, content) = match result {
        ToolResult::FileContent {
            path, content, ..
        } => (format!("📖 {}", path), content.clone()),
        ToolResult::Diagnostics {
            total,
            errors,
            warnings,
            list,
            ..
        } => {
            let mut s = format!(
                "📋 {} diagnostics ({} errors, {} warnings)\n",
                total, errors, warnings
            );
            for d in list.iter().filter(|d| d.severity == "error") {
                s.push_str(&format!(
                    "  ▸ {}: {} [{}:{}]\n",
                    d.code.as_deref().unwrap_or("?"),
                    d.message,
                    d.file,
                    d.line,
                ));
            }
            if *errors < *total {
                s.push_str(&format!(
                    "\n  ... {} warnings/hints",
                    *total - *errors
                ));
            }
            ("".into(), s)
        }
        ToolResult::Json {
            tool, count, data, ..
        } => {
            let s = serde_json::to_string_pretty(data).unwrap_or_default();
            (format!("📊 {} ({} results)", tool, count), s)
        }
        ToolResult::Matches {
            pattern,
            count,
            items,
            truncated,
        } => {
            let mut s = format!(
                "🔍 {}: {} matches{}\n",
                pattern,
                count,
                if *truncated { " (truncated)" } else { "" }
            );
            for m in items.iter().take(15) {
                s.push_str(&format!(
                    "  {}:{}:{}  {}\n",
                    m.file, m.line, m.column, m.text
                ));
            }
            ("".into(), s)
        }
        ToolResult::WriteConfirmation {
            path, diff, ..
        } => (format!("✏️  {}", path), diff.clone()),
        ToolResult::Text { content, .. } => ("".into(), content.clone()),
    };

    let block = if title.is_empty() {
        Block::default()
            .borders(Borders::NONE)
            .style(Style::default().bg(Color::White))
    } else {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(200, 200, 220)))
            .style(Style::default().bg(Color::White))
    };

    let paragraph = Paragraph::new(content)
        .block(block)
        .style(Style::default().fg(Color::Rgb(30, 30, 40)))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

pub fn llm_text(frame: &mut Frame, area: Rect, text: &str) {
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::Rgb(30, 30, 40)).bg(Color::White))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

pub fn user_input(frame: &mut Frame, area: Rect, input: &str) {
    let line = Line::from(vec![
        Span::styled(
            "bytode",
            Style::default()
                .fg(Color::Rgb(20, 60, 140))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("> ", Style::default().fg(Color::Rgb(0, 140, 0))),
        Span::styled(input, Style::default().fg(Color::Rgb(30, 30, 40))),
        Span::styled(
            " ",
            Style::default()
                .bg(Color::Rgb(30, 30, 40))
                .add_modifier(Modifier::SLOW_BLINK),
        ), // cursor
    ]);
    let paragraph = Paragraph::new(line).style(Style::default().bg(Color::White));
    frame.render_widget(paragraph, area);
}

pub fn approve(frame: &mut Frame, path: &str, diff: &str, preview_lines: usize) {
    let area = frame.area();
    let popup_width = std::cmp::min(80, area.width) as u16;
    let popup_height = std::cmp::min(20, area.height) as u16;
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the area behind the popup
    frame.render_widget(Clear, popup_area);

    let diff_preview: String = diff.lines().take(preview_lines).collect::<Vec<_>>().join("\n");

    let content = format!(
        "File: {}\n\n{}\n\n[Y] approve  [N] reject  [V] view full diff",
        path, diff_preview
    );

    let block = Block::default()
        .title("✏️  Confirm Write")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(200, 160, 0)))
        .style(Style::default().bg(Color::Rgb(255, 252, 240)));

    let paragraph = Paragraph::new(content)
        .block(block)
        .style(Style::default().fg(Color::Rgb(40, 40, 50)))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, popup_area);
}
