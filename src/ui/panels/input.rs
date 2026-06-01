use crate::ui::events::{
    DispatchResult, Effect, KeyAction, MouseAction, PanelId, WindowSlot, WindowSpec,
};
use crate::ui::panels::{panel_block, BG, BLUE, ORANGE, PanelContext, RenderContext, TXT, TXT_SUBTLE, UiContext};
use crate::ui::render;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

const MAX_INPUT_LINES: u16 = 6;

pub struct InputPanel {
    input: String,
    cursor: usize,
    pending: Option<String>,
    notice: Option<String>,
}

impl InputPanel {
    pub fn new() -> Self {
        Self { input: String::new(), cursor: 0, pending: None, notice: None }
    }

    pub fn id(&self) -> PanelId {
        PanelId::Input
    }

    pub fn visible(&self, _ctx: &UiContext<'_>) -> bool {
        true
    }

    pub fn focusable(&self, _ctx: &UiContext<'_>) -> bool {
        true
    }

    pub fn window_spec(&self, _ctx: &UiContext<'_>) -> WindowSpec {
        let input_lines = self.visible_input_lines();
        let total_lines = input_lines.saturating_add(1);
        WindowSpec {
            id: self.id(),
            z_index: 0,
            slot: WindowSlot::Bottom,
            size: total_lines.saturating_add(2).clamp(4, MAX_INPUT_LINES + 3),
        }
    }

    pub fn handle_key(&mut self, key: KeyAction, ctx: &PanelContext<'_>) -> DispatchResult {
        match key {
            KeyAction::Char(ch) => {
                self.insert_char(ch);
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Backspace => {
                self.backspace();
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Delete => {
                self.delete();
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Left => {
                self.move_left();
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Right => {
                self.move_right();
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Home | KeyAction::CtrlA => {
                self.cursor = 0;
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::End | KeyAction::CtrlE => {
                self.cursor = self.len_chars();
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::CtrlU => {
                self.input.clear();
                self.cursor = 0;
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::ShiftEnter => {
                self.insert_char('\n');
                DispatchResult::Consumed(Vec::new())
            }
            KeyAction::Enter => {
                let input = std::mem::take(&mut self.input);
                self.cursor = 0;
                if input.is_empty() {
                    return DispatchResult::Consumed(Vec::new());
                }
                if ctx.exec_state.is_busy() {
                    self.pending = Some(input);
                    self.notice = Some("still streaming".into());
                    return DispatchResult::Consumed(Vec::new());
                }
                DispatchResult::Consumed(vec![Effect::StartTurn { turn_id: 0, input }])
            }
            _ => DispatchResult::Ignored,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext<'_>) {
        let block = panel_block("Input", ctx.focused == Some(self.id()));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let input_height = self.visible_input_lines().min(inner.height.saturating_sub(1)).max(1);
        let input_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            input_height,
        );
        let footer_area = Rect::new(
            inner.x,
            inner.y.saturating_add(input_height),
            inner.width,
            1,
        );

        render::render_input(frame, input_area, &self.input, self.cursor);
        frame.render_widget(Paragraph::new(self.footer_line(ctx)).style(Style::default().bg(BG)), footer_area);
    }

    pub fn handle_mouse(
        &mut self,
        _mouse: MouseAction,
        _area: Rect,
        _ctx: &PanelContext<'_>,
    ) -> DispatchResult {
        DispatchResult::Ignored
    }

    pub fn take_pending(&mut self) -> Option<String> {
        self.pending.take()
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub(crate) fn notice_text(&self) -> Option<&str> {
        self.pending.as_deref().or(self.notice.as_deref())
    }

    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    fn pending_notice_line(&self) -> Option<Line<'static>> {
        if let Some(pending) = &self.pending {
            return Some(Line::from(Span::styled(
                format!("  [pending] {pending}"),
                Style::default().fg(ORANGE).add_modifier(Modifier::DIM),
            )));
        }
        self.notice.as_ref().map(|notice| {
            Line::from(Span::styled(
                format!("  {notice}"),
                Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::DIM),
            ))
        })
    }

    fn footer_line(&self, ctx: &RenderContext<'_>) -> Line<'static> {
        let status = &ctx.runtime.status;
        let mode = if status.mode.is_empty() { "build" } else { status.mode.as_str() };
        let activity = if status.task.is_empty() {
            "idle".to_string()
        } else {
            format!("{} {}", status.spinner, status.task)
        };
        let notice = self
            .pending_notice_line()
            .map(|line| line.to_string())
            .unwrap_or_else(|| "tab focus  ctrl+/ help  ctrl+d exit".to_string());

        Line::from(vec![
            Span::styled("▎ ", Style::default().fg(BLUE)),
            Span::styled(format!("{mode}"), Style::default().fg(TXT).add_modifier(Modifier::BOLD)),
            Span::styled("  ", Style::default().bg(BG)),
            Span::styled(activity, Style::default().fg(ORANGE)),
            Span::styled("  ", Style::default().bg(BG)),
            Span::styled(notice, Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::DIM)),
        ])
    }

    fn visible_input_lines(&self) -> u16 {
        (self.input.matches('\n').count() as u16 + 1).clamp(1, MAX_INPUT_LINES)
    }

    fn len_chars(&self) -> usize {
        self.input.chars().count()
    }

    fn insert_char(&mut self, ch: char) {
        let mut chars = self.input.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(chars.len());
        chars.insert(cursor, ch);
        self.cursor = cursor + 1;
        self.input = chars.into_iter().collect();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut chars = self.input.chars().collect::<Vec<_>>();
        let remove_at = self.cursor - 1;
        if remove_at < chars.len() {
            chars.remove(remove_at);
            self.cursor -= 1;
            self.input = chars.into_iter().collect();
        }
    }

    fn delete(&mut self) {
        let mut chars = self.input.chars().collect::<Vec<_>>();
        if self.cursor >= chars.len() {
            return;
        }
        chars.remove(self.cursor);
        self.input = chars.into_iter().collect();
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len_chars());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::events::ExecState;
    use crate::ui::panels::PanelContext;

    #[test]
    fn queues_pending_while_busy() {
        let mut panel = InputPanel::new();
        let busy = ExecState::Streaming { turn_id: 1 };
        let ctx = PanelContext { exec_state: &busy };

        let _ = panel.handle_key(KeyAction::Char('h'), &ctx);
        let _ = panel.handle_key(KeyAction::Enter, &ctx);

        assert_eq!(panel.pending.as_deref(), Some("h"));
    }

    #[test]
    fn inserts_and_moves_cursor() {
        let mut panel = InputPanel::new();
        let idle = ExecState::Idle;
        let ctx = PanelContext { exec_state: &idle };

        let _ = panel.handle_key(KeyAction::Char('a'), &ctx);
        let _ = panel.handle_key(KeyAction::Char('b'), &ctx);
        let _ = panel.handle_key(KeyAction::Left, &ctx);
        let _ = panel.handle_key(KeyAction::Char('x'), &ctx);

        assert_eq!(panel.input, "axb");
        assert_eq!(panel.cursor, 2);
    }
}
