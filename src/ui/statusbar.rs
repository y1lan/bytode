use ratatui::{prelude::*, widgets::Paragraph};

#[derive(Clone)]
pub struct StatusBarState {
    pub dir: String,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub task: String,
    pub model: String,
    pub ctx_used: u64,
    pub ctx_total: u64,
    pub tool_count: usize,
    pub session_cost: f64,
    pub session_calls: u64,
    pub mode: String,
    pub elapsed: String,
    pub spinner: char,
}

pub fn render(frame: &mut Frame, area: Rect, state: &StatusBarState) {
    let dir = shorten_path(&state.dir, 25);

    let git_branch = state.git_branch.as_deref().unwrap_or("?");
    let (git_icon, git_style) = if state.git_dirty {
        ("*", Style::default().fg(Color::Rgb(223, 142, 29)))
    } else {
        ("✓", Style::default().fg(Color::Rgb(64, 160, 43)))
    };

    let (task_text, task_style) = if state.task.contains("thinking") {
        (
            " thinking...",
            Style::default()
                .fg(Color::Rgb(223, 142, 29))
                .add_modifier(Modifier::SLOW_BLINK),
        )
    } else if state.task.contains("idle") {
        (" idle", Style::default().fg(Color::Rgb(172, 176, 190)))
    } else {
        ("", Style::default().fg(Color::Rgb(76, 79, 105)))
    };

    let username = std::env::var("USER").unwrap_or_else(|_| "?".to_string());
    let left = Line::from(vec![
        Span::styled(
            format!(" {} ", username),
            Style::default()
                .fg(Color::Rgb(76, 79, 105))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("│", Style::default().fg(Color::Rgb(172, 176, 190))),
        Span::styled(
            " bytode ",
            Style::default()
                .fg(Color::Rgb(30, 102, 245))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("│", Style::default().fg(Color::Rgb(172, 176, 190))),
        Span::styled(
            format!(" {} ", dir),
            Style::default().fg(Color::Rgb(108, 111, 133)),
        ),
        Span::styled("│", Style::default().fg(Color::Rgb(172, 176, 190))),
        Span::styled(
            format!(" {} ", git_branch),
            Style::default().fg(Color::Rgb(124, 127, 147)),
        ),
        Span::styled(git_icon, git_style),
        Span::styled(" │", Style::default().fg(Color::Rgb(172, 176, 190))),
        Span::styled(
            format!(" {} ", state.mode),
            if state.mode == "plan" {
                Style::default()
                    .fg(Color::Rgb(136, 57, 239))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(156, 160, 176))
            },
        ),
        Span::styled("│", Style::default().fg(Color::Rgb(172, 176, 190))),
        Span::styled(task_text, task_style),
    ]);

    let ctx_ratio = (state.ctx_used as f64 / state.ctx_total as f64).min(1.0);
    let ctx_color = if ctx_ratio > 0.9 {
        Color::Rgb(210, 15, 57)
    } else if ctx_ratio > 0.6 {
        Color::Rgb(223, 142, 29)
    } else {
        Color::Rgb(156, 160, 176)
    };

    let right = Line::from(vec![
        Span::styled(
            format!("{} {}", state.spinner, state.elapsed),
            if state.elapsed.contains('m') {
                Style::default().fg(Color::Rgb(223, 142, 29))
            } else {
                Style::default().fg(Color::Rgb(156, 160, 176))
            },
        ),
        Span::styled(" │", Style::default().fg(Color::Rgb(172, 176, 190))),
        Span::styled(
            format!("{} ", state.model),
            Style::default().fg(Color::Rgb(136, 57, 239)),
        ),
        Span::styled("│", Style::default().fg(Color::Rgb(172, 176, 190))),
        Span::styled(" ctx ", Style::default().fg(Color::Rgb(156, 160, 176))),
        Span::styled(draw_ctx_bar(state.ctx_used, state.ctx_total), ctx_color),
        Span::styled(
            format!("  ${:.4}", state.session_cost),
            Style::default().fg(if state.session_cost > 0.01 {
                Color::Rgb(254, 100, 11)
            } else {
                Color::Rgb(156, 160, 176)
            }),
        ),
        Span::styled(
            format!("  ctrl+t tools:{}", state.tool_count),
            Style::default().fg(Color::Rgb(156, 160, 176)),
        ),
    ]);

    let bg_color = Color::Rgb(230, 233, 239);
    let paragraph = Paragraph::new(left).style(Style::default().bg(bg_color));
    frame.render_widget(paragraph, area);

    // Right-aligned portion
    let right_width = right.width() as u16;
    if area.width > right_width {
        let right_area = Rect::new(
            area.width.saturating_sub(right_width),
            area.y,
            right_width.min(area.width),
            1,
        );
        let right_p = Paragraph::new(right).style(Style::default().bg(bg_color));
        frame.render_widget(right_p, right_area);
    }
}

fn draw_ctx_bar(used: u64, total: u64) -> String {
    let ratio = (used as f64 / total as f64).min(1.0);
    let bar_width = 10;
    let filled = (ratio * bar_width as f64).round() as usize;
    let empty = bar_width - filled;

    let bar: String = "█".repeat(filled) + &"░".repeat(empty);
    let used_human = if used >= 1_000_000 {
        format!("{:.1}M", used as f64 / 1_000_000.0)
    } else {
        format!("{}K", used / 1000)
    };
    let total_human = format!("{:.0}M", total as f64 / 1_000_000.0);
    let warn = if ratio > 0.9 { " ⚠" } else { "" };

    format!("{} {}{}/{}", bar, used_human, warn, total_human)
}

fn shorten_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let shortened = path.replace(&home, "~");
    if shortened.len() <= max_len {
        return shortened;
    }
    let parts: Vec<&str> = shortened.split('/').collect();
    if parts.len() <= 2 {
        return format!(
            "...{}",
            &shortened[shortened.len().saturating_sub(max_len - 3)..]
        );
    }
    format!(".../{}", parts[parts.len() - 2..].join("/"))
}
