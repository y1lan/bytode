use super::components::{blank_line, render_history_entry, surface_line};
use super::model::{AssistantMessage, HistoryEntry, ScrollMode, UserMessage};
use super::parser::{parse_assistant_message, stream_tool_state};
use crate::ui::events::{DispatchResult, ExecState, KeyAction, PanelId, WindowSlot, WindowSpec};
use crate::ui::panels::{
    panel_block, PanelContext, RenderContext, TXT, TXT_SUBTLE, UiContext, BG,
};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use std::cell::Cell;

pub struct ContentPanel {
    entries: Vec<HistoryEntry>,
    stream_buffer: String,
    streaming_visible: String,
    scroll: ScrollMode,
    last_total_lines: Cell<usize>,
    last_visible_lines: Cell<usize>,
}

impl ContentPanel {
    pub fn new(entries: Vec<HistoryEntry>) -> Self {
        Self {
            entries,
            stream_buffer: String::new(),
            streaming_visible: String::new(),
            scroll: ScrollMode::Auto,
            last_total_lines: Cell::new(0),
            last_visible_lines: Cell::new(0),
        }
    }

    pub fn id(&self) -> PanelId {
        PanelId::Content
    }

    pub fn visible(&self, _ctx: &UiContext<'_>) -> bool {
        true
    }

    pub fn focusable(&self, _ctx: &UiContext<'_>) -> bool {
        true
    }

    pub fn window_spec(&self, _ctx: &UiContext<'_>) -> WindowSpec {
        WindowSpec { id: self.id(), z_index: 0, slot: WindowSlot::Content, size: 0 }
    }

    pub fn handle_key(&mut self, key: KeyAction, _ctx: &PanelContext<'_>) -> DispatchResult {
        let max_scroll = self.max_scroll();
        let up_step = match key {
            KeyAction::Up => Some(1),
            KeyAction::PageUp => Some(20),
            KeyAction::Home => {
                if max_scroll == 0 {
                    return DispatchResult::Ignored;
                }
                self.scroll = ScrollMode::Manual(max_scroll);
                return DispatchResult::Consumed(Vec::new());
            }
            _ => None,
        };
        if let Some(step) = up_step {
            if max_scroll == 0 {
                return DispatchResult::Ignored;
            }
            self.scroll = match self.scroll {
                ScrollMode::Auto => ScrollMode::Manual(step.min(max_scroll)),
                ScrollMode::Manual(current) => {
                    let next = current.saturating_add(step).min(max_scroll);
                    if next == current {
                        return DispatchResult::Ignored;
                    }
                    ScrollMode::Manual(next)
                }
            };
            return DispatchResult::Consumed(Vec::new());
        }

        let down_step = match key {
            KeyAction::Down => Some(1),
            KeyAction::PageDown => Some(20),
            KeyAction::End => {
                self.scroll = ScrollMode::Auto;
                return DispatchResult::Consumed(Vec::new());
            }
            KeyAction::CtrlL => {
                return DispatchResult::Consumed(Vec::new());
            }
            _ => None,
        };
        if let Some(step) = down_step {
            self.scroll = match self.scroll {
                ScrollMode::Auto => return DispatchResult::Ignored,
                ScrollMode::Manual(current) => {
                    let next = current.saturating_sub(step);
                    if next == current {
                        return DispatchResult::Ignored;
                    }
                    if next == 0 {
                        ScrollMode::Auto
                    } else {
                        ScrollMode::Manual(next)
                    }
                }
            };
            return DispatchResult::Consumed(Vec::new());
        }

        DispatchResult::Ignored
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, ctx: &RenderContext<'_>) {
        let block = panel_block("Conversation", ctx.focused == Some(self.id()));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let width = inner.width as usize;
        let visible = inner.height as usize;
        let lines = self.collect_lines(width, ctx.exec_state);
        self.last_total_lines.set(lines.len());
        self.last_visible_lines.set(visible);

        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(TXT).bg(BG))
            .scroll((self.scroll_offset(self.last_total_lines.get(), visible) as u16, 0));
        frame.render_widget(paragraph, inner);
    }

    pub fn push_user(&mut self, text: String) {
        self.entries
            .push(HistoryEntry::User(UserMessage { body: text, meta: None }));
        self.scroll = ScrollMode::Auto;
    }

    pub fn push_assistant(&mut self, text: String) {
        self.entries
            .push(HistoryEntry::Assistant(AssistantMessage::from_text(text)));
        self.scroll = ScrollMode::Auto;
    }

    pub fn push_error(&mut self, text: String) {
        self.entries.push(HistoryEntry::Error(text));
        self.scroll = ScrollMode::Auto;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll = ScrollMode::Auto;
    }

    pub fn begin_stream(&mut self) {
        self.stream_buffer.clear();
        self.streaming_visible.clear();
        self.scroll_to_bottom();
    }

    pub fn append_stream_chunk(&mut self, chunk: &str) {
        self.stream_buffer.push_str(chunk);
        self.streaming_visible.push_str(chunk);
    }

    pub fn finalize_stream(&mut self) {
        let final_text = std::mem::take(&mut self.stream_buffer);
        self.streaming_visible.clear();
        if final_text.trim().is_empty() {
            return;
        }

        self.entries.push(HistoryEntry::Assistant(parse_assistant_message(
            &final_text,
            stream_tool_state(None),
        )));
    }

    fn collect_lines(&self, width: usize, _exec_state: &ExecState) -> Vec<Line<'static>> {
        let mut all = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                all.push(blank_line(width));
            }
            render_history_entry(entry, width, &mut all);
        }

        if !self.streaming_visible.is_empty() {
            if !all.is_empty() {
                all.push(blank_line(width));
            }
            // Stream raw text directly so every arriving chunk is immediately visible.
            // We only parse into structured tool/text blocks once the turn finalizes.
            let streaming =
                HistoryEntry::Assistant(AssistantMessage::from_text(self.streaming_visible.clone()));
            render_history_entry(&streaming, width, &mut all);
        }

        if all.is_empty() {
            all.push(surface_line(
                vec![Span::styled(
                    "  Ask for a change, inspect a file, or run a project task.",
                    Style::default().fg(TXT_SUBTLE).add_modifier(Modifier::DIM),
                )],
                width,
            ));
        }
        all
    }

    fn scroll_offset(&self, total_visual_lines: usize, visible: usize) -> usize {
        let max_scroll = total_visual_lines.saturating_sub(visible);
        let skip = match self.scroll {
            ScrollMode::Auto => 0,
            ScrollMode::Manual(lines) => lines.min(max_scroll),
        };
        total_visual_lines.saturating_sub(skip.saturating_add(visible))
    }

    fn max_scroll(&self) -> usize {
        self.last_total_lines
            .get()
            .saturating_sub(self.last_visible_lines.get())
    }

    #[cfg(test)]
    pub(crate) fn rendered_text_for_test(
        &self,
        visible: usize,
        width: usize,
        exec_state: &ExecState,
    ) -> Vec<String> {
        let all = self.collect_lines(width, exec_state);
        self.last_total_lines.set(all.len());
        self.last_visible_lines.set(visible);
        let start = self.scroll_offset(all.len(), visible);
        all.into_iter()
            .skip(start)
            .take(visible)
            .map(|line| line.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::panels::AssistantPart;

    fn idle_panel_ctx() -> PanelContext<'static> {
        static EXEC_STATE: ExecState = ExecState::Idle;
        PanelContext {
            exec_state: &EXEC_STATE,
        }
    }

    #[test]
    fn finalize_stream_builds_structured_assistant_message() {
        let mut panel = ContentPanel::new(Vec::new());
        panel.begin_stream();
        panel.append_stream_chunk("hello\n\n  ⟳ read_file(src/main.rs)\n   1 | fn main() {}\n");
        panel.finalize_stream();

        let HistoryEntry::Assistant(message) = &panel.entries[0] else {
            panic!("expected assistant entry");
        };

        assert!(matches!(message.parts[0], AssistantPart::Text(_)));
        assert!(matches!(message.parts[1], AssistantPart::Tool(_)));
    }

    #[test]
    fn streaming_chunks_are_visible_before_finalize() {
        let mut panel = ContentPanel::new(Vec::new());
        let exec_state = ExecState::Streaming { turn_id: 1 };

        panel.begin_stream();
        panel.append_stream_chunk("hello");

        let lines = panel.rendered_text_for_test(4, 80, &exec_state);
        assert!(lines.iter().any(|line| line.contains("hello")));
    }

    #[test]
    fn manual_scroll_stays_detached_until_user_returns_to_bottom() {
        let mut panel = ContentPanel::new(Vec::new());
        let exec_state = ExecState::Streaming { turn_id: 1 };

        panel.begin_stream();
        panel.append_stream_chunk("line 1\nline 2\nline 3\nline 4");

        let _ = panel.rendered_text_for_test(2, 80, &exec_state);
        let _ = panel.handle_key(KeyAction::Up, &idle_panel_ctx());

        assert!(matches!(panel.scroll, ScrollMode::Manual(1)));

        panel.append_stream_chunk("\nline 5");
        let detached = panel.rendered_text_for_test(2, 80, &exec_state);
        assert!(!detached.iter().any(|line| line.contains("line 5")));

        let _ = panel.handle_key(KeyAction::End, &idle_panel_ctx());
        assert!(matches!(panel.scroll, ScrollMode::Auto));

        let followed = panel.rendered_text_for_test(2, 80, &exec_state);
        assert!(followed.iter().any(|line| line.contains("line 5")));
    }

    #[test]
    fn pushing_user_message_rejoins_bottom_follow() {
        let mut panel = ContentPanel::new(Vec::new());
        let exec_state = ExecState::Streaming { turn_id: 1 };

        panel.begin_stream();
        panel.append_stream_chunk("line 1\nline 2\nline 3\nline 4");
        let _ = panel.rendered_text_for_test(2, 80, &exec_state);
        let _ = panel.handle_key(KeyAction::Up, &idle_panel_ctx());
        assert!(matches!(panel.scroll, ScrollMode::Manual(1)));

        panel.push_user("new task".into());
        panel.begin_stream();
        let visible = panel.rendered_text_for_test(2, 80, &exec_state);

        assert!(matches!(panel.scroll, ScrollMode::Auto));
        assert!(visible.iter().any(|line| line.contains("new task")));
    }

    #[test]
    fn auto_follow_anchors_to_bottom_of_multiline_user_message() {
        let mut panel = ContentPanel::new(Vec::new());
        let exec_state = ExecState::Streaming { turn_id: 1 };

        panel.push_user("line 1\nline 2\nline 3".into());
        panel.begin_stream();

        let visible = panel.rendered_text_for_test(2, 80, &exec_state);
        let joined = visible.join(" ");

        assert!(matches!(panel.scroll, ScrollMode::Auto));
        assert_eq!(visible.len(), 2);
        assert!(!joined.contains("line 1"));
        assert!(joined.contains("line 2"));
        assert!(joined.contains("line 3"));
    }
}
