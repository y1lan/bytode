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
}

pub fn render(frame: &mut Frame, area: Rect, state: &StatusBarState) {
    let name = "bytode";
    let dir = shorten_path(&state.dir, 25);

    let git = {
        let branch = state.git_branch.as_deref().unwrap_or("?");
        let dirty = if state.git_dirty { " *" } else { " ✓" };
        format!("{}{}", branch, dirty)
    };

    let ctx_bar = draw_ctx_bar(state.ctx_used, state.ctx_total);

    let left = format!(
        "{} │ {} │ {} │ {}",
        name, dir, git, state.task
    );

    let right = format!(
        "{} │ ctx {}  ctrl+t tools:{}",
        state.model, ctx_bar, state.tool_count
    );

    let width = area.width as usize;
    let padding = width.saturating_sub(left.len() + right.len());
    let bar_text = format!("{}{}{}", left, " ".repeat(padding), right);

    let paragraph = Paragraph::new(bar_text)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(paragraph, area);
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
        return format!("...{}", &shortened[shortened.len().saturating_sub(max_len - 3)..]);
    }
    format!(".../{}", &parts[parts.len() - 2..].join("/"))
}
