use crate::ui::events::{DispatchResult, Effect, KeyAction, PanelId, WindowSlot, WindowSpec};
use crate::ui::panels::{panel_block, BG, ORANGE, PanelContext, RenderContext, TXT_SUBTLE, UiContext};
use crate::ui::render;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

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
        let mut lines = self.input.matches('\n').count() as u16 + 1;
        if self.pending.is_some() || self.notice.is_some() {
            lines = lines.saturating_add(1);
        }
        WindowSpec { id: self.id(), z_index: 0, slot: WindowSlot::Bottom, size: lines.clamp(2, 9) }
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
            KeyAction::AltEnter | KeyAction::ShiftEnter => {
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

        if let Some(line) = self.pending_notice_line() {
            let notice_area = Rect::new(inner.x, inner.y, inner.width, 1);
            frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), notice_area);
        }

        let offset = if self.pending.is_some() || self.notice.is_some() { 1 } else { 0 };
        let input_area = Rect::new(
            inner.x,
            inner.y.saturating_add(offset),
            inner.width,
            inner.height.saturating_sub(offset).max(1),
        );
        render::render_input(frame, input_area, &self.input, self.cursor);
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
