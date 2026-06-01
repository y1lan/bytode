use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const TXT: Color = Color::Rgb(76, 79, 105);
const BLUE: Color = Color::Rgb(30, 102, 245);
const GREEN: Color = Color::Rgb(64, 160, 43);

pub fn render_input(frame: &mut Frame, area: Rect, input: &str, cursor: usize) {
    let mut lines: Vec<Line> = Vec::new();
    let rendered = render_with_cursor(input, cursor);
    let split: Vec<&str> = if rendered.is_empty() { vec![""] } else { rendered.split('\n').collect() };

    for (index, part) in split.iter().enumerate() {
        if index == 0 {
            lines.push(Line::from(vec![
                Span::styled("bytode", Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
                Span::styled("> ", Style::default().fg(GREEN)),
                Span::styled(*part, Style::default().fg(TXT)),
            ]));
            continue;
        }
        if index == split.len() - 1 {
            lines.push(Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled(*part, Style::default().fg(TXT)),
            ]));
            continue;
        }

        lines.push(Line::from(Span::styled(
            format!("      {part}"),
            Style::default().fg(TXT),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_with_cursor(input: &str, cursor: usize) -> String {
    let mut chars = input.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    chars.insert(cursor, cursor_glyph());
    chars.into_iter().collect()
}

fn cursor_glyph() -> char {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    if (millis / 500).is_multiple_of(2) {
        '|'
    } else {
        ' '
    }
}
