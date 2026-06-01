use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const TXT: Color = Color::Rgb(76, 79, 105);
const BLUE: Color = Color::Rgb(30, 102, 245);
const GREEN: Color = Color::Rgb(64, 160, 43);

pub fn render_input(frame: &mut Frame, area: Rect, input: &str) {
    let mut lines: Vec<Line> = Vec::new();
    let split: Vec<&str> = if input.is_empty() {
        vec![""]
    } else {
        input.split('\n').collect()
    };

    for (index, part) in split.iter().enumerate() {
        if index == 0 {
            lines.push(Line::from(vec![
                Span::styled("bytode", Style::default().fg(BLUE).add_modifier(Modifier::BOLD)),
                Span::styled("> ", Style::default().fg(GREEN)),
                Span::styled(*part, Style::default().fg(TXT)),
                if split.len() == 1 {
                    Span::styled("|", Style::default().fg(TXT).add_modifier(Modifier::SLOW_BLINK))
                } else {
                    Span::raw("")
                },
            ]));
            continue;
        }
        if index == split.len() - 1 {
            lines.push(Line::from(vec![
                Span::styled("      ", Style::default()),
                Span::styled(*part, Style::default().fg(TXT)),
                Span::styled("|", Style::default().fg(TXT).add_modifier(Modifier::SLOW_BLINK)),
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
