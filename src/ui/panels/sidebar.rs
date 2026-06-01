use crate::ui::events::{DispatchResult, KeyAction, PanelId, WindowSlot, WindowSpec};
use crate::ui::panels::{panel_block, BLUE, CYAN, GREEN, ORANGE, PanelContext, RED, RenderContext, TXT, TXT_SUBTLE, UiContext};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};

pub struct SidebarPanel {
    selected: usize,
}

impl SidebarPanel {
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    pub fn id(&self) -> PanelId {
        PanelId::Sidebar
    }

    pub fn visible(&self, ctx: &UiContext<'_>) -> bool {
        ctx.sidebar_visible
    }

    pub fn focusable(&self, ctx: &UiContext<'_>) -> bool {
        self.visible(ctx)
    }

    pub fn window_spec(&self, _ctx: &UiContext<'_>) -> WindowSpec {
        WindowSpec { id: self.id(), z_index: 0, slot: WindowSlot::Right, size: 24 }
    }

    pub fn handle_key(&mut self, key: KeyAction, _ctx: &PanelContext<'_>) -> DispatchResult {
        match key {
            KeyAction::Up => {
                self.selected = self.selected.saturating_sub(1);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Down => {
                self.selected = self.selected.saturating_add(1);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::PageUp => {
                self.selected = self.selected.saturating_sub(5);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::PageDown => {
                self.selected = self.selected.saturating_add(5);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Enter | KeyAction::Left | KeyAction::Right | KeyAction::Esc => {
                DispatchResult::Consumed(Vec::new())
            }
            _ => DispatchResult::Ignored,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext<'_>) {
        let block = panel_block("Sidebar", ctx.focused == Some(self.id()));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = vec![
            Line::from(Span::styled(" Tools", Style::default().fg(BLUE).add_modifier(Modifier::BOLD))),
            Line::from(""),
        ];

        for (index, name) in ctx.runtime.tool_names.iter().enumerate() {
            let color = match name.as_str() {
                "read_file" => GREEN,
                "write_file" => ORANGE,
                "search_code" => CYAN,
                "cargo_check" => Color::Rgb(136, 57, 239),
                "get_diagnostics" => RED,
                _ => Color::Rgb(156, 160, 176),
            };
            let prefix = if ctx.focused == Some(self.id()) && index == self.selected { "›" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(format!("{prefix} "), Style::default().fg(BLUE)),
                Span::styled(name.clone(), Style::default().fg(color)),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "\u{2500}".repeat(inner.width.saturating_sub(2) as usize),
            Style::default().fg(Color::Rgb(172, 176, 190)),
        )));
        lines.push(Line::from(vec![
            Span::styled(" L: ", Style::default().fg(Color::Rgb(140, 143, 161))),
            Span::styled(&ctx.runtime.primary_language, Style::default().fg(TXT)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" D: ", Style::default().fg(Color::Rgb(140, 143, 161))),
            Span::styled(&ctx.runtime.detection_source, Style::default().fg(TXT_SUBTLE)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(" tab cycle focus", Style::default().fg(TXT_SUBTLE))));

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }

    pub fn normalize(&mut self, ctx: &UiContext<'_>) {
        if ctx.runtime.tool_names.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(ctx.runtime.tool_names.len() - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::events::ExecState;
    use crate::ui::panels::{RuntimeSnapshot, UiContext};
    use crate::ui::statusbar::StatusBarState;

    #[test]
    fn clamps_selection() {
        let mut panel = SidebarPanel { selected: 99 };
        let runtime = RuntimeSnapshot {
            tool_names: vec!["read_file".into(), "search_code".into()],
            status: StatusBarState {
                dir: String::new(),
                git_branch: None,
                git_dirty: false,
                task: String::new(),
                model: String::new(),
                ctx_used: 0,
                ctx_total: 1,
                tool_count: 0,
                session_cost: 0.0,
                session_calls: 0,
                mode: String::new(),
                elapsed: String::new(),
                spinner: ' ',
            },
            primary_language: "rust".into(),
            detection_source: "auto".into(),
        };
        let ctx = UiContext { exec_state: &ExecState::Idle, runtime: &runtime, sidebar_visible: true };

        panel.normalize(&ctx);
        assert_eq!(panel.selected, 1);
    }
}
