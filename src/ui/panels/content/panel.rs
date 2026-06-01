use super::components::{
    blank_line, render_history_entry, render_history_entry_with_hits, surface_line,
    ContentHitRegion, ContentHitTarget,
};
use super::model::{AssistantMessage, AssistantPart, HistoryEntry, ScrollMode, UserMessage};
use super::parser::{parse_assistant_message, stream_tool_state};
use crate::ui::events::{
    DispatchResult, ExecState, KeyAction, MouseAction, MouseButton, PanelId, WindowSlot,
    WindowSpec,
};
use crate::ui::panels::{
    panel_block, PanelContext, RenderContext, TXT, TXT_SUBTLE, UiContext, BG,
};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use std::cell::{Cell, RefCell};

pub struct ContentPanel {
    entries: Vec<HistoryEntry>,
    stream_buffer: String,
    streaming_visible: String,
    scroll: ScrollMode,
    last_total_lines: Cell<usize>,
    last_visible_lines: Cell<usize>,
    last_hit_regions: RefCell<Vec<ContentHitRegion>>,
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
            last_hit_regions: RefCell::new(Vec::new()),
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
        let (lines, hit_regions) = self.collect_lines(width, ctx.exec_state);
        self.last_total_lines.set(lines.len());
        self.last_visible_lines.set(visible);
        self.last_hit_regions.replace(hit_regions);

        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(TXT).bg(BG))
            .scroll((self.scroll_offset(self.last_total_lines.get(), visible) as u16, 0));
        frame.render_widget(paragraph, inner);
    }

    pub fn handle_mouse(
        &mut self,
        mouse: MouseAction,
        area: Rect,
        _ctx: &PanelContext<'_>,
    ) -> DispatchResult {
        match mouse {
            MouseAction::ScrollUp { .. } => self.scroll_up(3),
            MouseAction::ScrollDown { .. } => self.scroll_down(3),
            MouseAction::Down {
                button: MouseButton::Left,
                column,
                row,
            } => {
                let inner = panel_block("Conversation", true).inner(area);
                if !point_in_rect(inner, column, row) {
                    return DispatchResult::Ignored;
                }

                if self.toggle_hit_at(inner, row) {
                    return DispatchResult::Consumed(Vec::new());
                }

                DispatchResult::Consumed(Vec::new())
            }
            MouseAction::Down { .. } => DispatchResult::Ignored,
        }
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

    fn collect_lines(
        &self,
        width: usize,
        _exec_state: &ExecState,
    ) -> (Vec<Line<'static>>, Vec<ContentHitRegion>) {
        let mut all = Vec::new();
        let mut hits = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                all.push(blank_line(width));
            }
            render_history_entry_with_hits(entry, index, width, &mut all, &mut hits);
        }

        if !self.streaming_visible.is_empty() {
            if !all.is_empty() {
                all.push(blank_line(width));
            }
            let streaming = HistoryEntry::Assistant(parse_assistant_message(
                &self.streaming_visible,
                stream_tool_state(Some(_exec_state)),
            ));
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
        (all, hits)
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

    fn scroll_up(&mut self, step: usize) -> DispatchResult {
        let max_scroll = self.max_scroll();
        if max_scroll == 0 {
            return DispatchResult::Ignored;
        }

        self.scroll = match self.scroll {
            ScrollMode::Auto => ScrollMode::Manual(step.min(max_scroll)),
            ScrollMode::Manual(current) => ScrollMode::Manual(current.saturating_add(step).min(max_scroll)),
        };
        DispatchResult::Consumed(Vec::new())
    }

    fn scroll_down(&mut self, step: usize) -> DispatchResult {
        self.scroll = match self.scroll {
            ScrollMode::Auto => return DispatchResult::Ignored,
            ScrollMode::Manual(current) => {
                let next = current.saturating_sub(step);
                if next == 0 {
                    ScrollMode::Auto
                } else {
                    ScrollMode::Manual(next)
                }
            }
        };
        DispatchResult::Consumed(Vec::new())
    }

    fn toggle_hit_at(&mut self, inner: Rect, row: u16) -> bool {
        if inner.height == 0 {
            return false;
        }

        let visible_row = row.saturating_sub(inner.y) as usize;
        let absolute_line = self
            .scroll_offset(self.last_total_lines.get(), inner.height as usize)
            .saturating_add(visible_row);

        let target = self
            .last_hit_regions
            .borrow()
            .iter()
            .find(|region| region.line == absolute_line)
            .map(|region| region.target);

        let Some(target) = target else {
            return false;
        };

        match target {
            ContentHitTarget::Tool {
                entry_index,
                part_index,
            } => self.toggle_tool_part(entry_index, part_index),
            ContentHitTarget::Reasoning {
                entry_index,
                part_index,
            } => self.toggle_reasoning_part(entry_index, part_index),
        }
    }

    fn toggle_tool_part(&mut self, entry_index: usize, part_index: usize) -> bool {
        let Some(HistoryEntry::Assistant(message)) = self.entries.get_mut(entry_index) else {
            return false;
        };
        let Some(AssistantPart::Tool(part)) = message.parts.get_mut(part_index) else {
            return false;
        };
        if part.body.is_none() || !matches!(part.presentation, super::model::ToolPresentation::Block) {
            return false;
        }
        part.collapsed = !part.collapsed;
        true
    }

    fn toggle_reasoning_part(&mut self, entry_index: usize, part_index: usize) -> bool {
        let Some(HistoryEntry::Assistant(message)) = self.entries.get_mut(entry_index) else {
            return false;
        };
        let Some(AssistantPart::Reasoning(part)) = message.parts.get_mut(part_index) else {
            return false;
        };
        part.collapsed = !part.collapsed;
        true
    }

    #[cfg(test)]
    pub(crate) fn rendered_text_for_test(
        &self,
        visible: usize,
        width: usize,
        exec_state: &ExecState,
    ) -> Vec<String> {
        let all = self.collect_lines(width, exec_state);
        self.last_total_lines.set(all.0.len());
        self.last_visible_lines.set(visible);
        let start = self.scroll_offset(all.0.len(), visible);
        all.0.into_iter()
            .skip(start)
            .take(visible)
            .map(|line| line.to_string())
            .collect()
    }
}

fn point_in_rect(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
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

    #[test]
    fn streaming_read_file_renders_as_block_with_body() {
        let mut panel = ContentPanel::new(Vec::new());
        let exec_state = ExecState::Streaming { turn_id: 1 };

        panel.begin_stream();
        panel.append_stream_chunk(
            "  ⟳ read_file(/home/aromatic/Applications/OwnProject/bytode/src/tools/file.rs)\n   1 | use crate::error::{BytodeError, Result};\n   2 | use crate::tools::{Tool, ToolResult};\n",
        );

        let lines = panel.rendered_text_for_test(6, 120, &exec_state);
        assert!(lines.iter().any(|line| line.contains("read_file")));
        assert!(lines.iter().any(|line| line.contains("1 | use crate::error")));
    }

    #[test]
    fn mouse_click_toggles_collapsed_tool_block() {
        let mut panel = ContentPanel::new(Vec::new());
        panel.entries.push(HistoryEntry::Assistant(parse_assistant_message(
            "  ⟳ read_file(src/main.rs)\n   1 | fn main() {}\n   2 | println!(\"hi\");\n   3 | let x = 1;\n   4 | let y = 2;\n   5 | let z = 3;\n   6 | done();\n   7 | extra();\n",
            stream_tool_state(None),
        )));

        let exec_state = ExecState::Idle;
        let (lines, hits) = panel.collect_lines(80, &exec_state);
        panel.last_total_lines.set(lines.len());
        panel.last_visible_lines.set(lines.len());
        panel.last_hit_regions.replace(hits);

        let area = Rect::new(0, 0, 80, 12);
        let result = panel.handle_mouse(
            MouseAction::Down {
                button: MouseButton::Left,
                column: 2,
                row: 1,
            },
            area,
            &idle_panel_ctx(),
        );

        assert!(matches!(result, DispatchResult::Consumed(_)));
        let HistoryEntry::Assistant(message) = &panel.entries[0] else {
            panic!("expected assistant entry");
        };
        let AssistantPart::Tool(part) = &message.parts[0] else {
            panic!("expected tool part");
        };
        assert!(!part.collapsed);
    }
}
