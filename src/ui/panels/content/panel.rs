use super::components::{blank_line, render_history_entry, surface_line};
use super::model::{AssistantMessage, HistoryEntry, ScrollMode, UserMessage};
use super::parser::{parse_assistant_message, stream_tool_state};
use crate::ui::events::{DispatchResult, ExecState, KeyAction, PanelId, WindowSlot, WindowSpec};
use crate::ui::panels::{
    panel_block, PanelContext, RenderContext, TXT, TXT_SUBTLE, UiContext, BG,
};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};

pub struct ContentPanel {
    entries: Vec<HistoryEntry>,
    stream_buffer: String,
    streaming_visible: String,
    scroll: ScrollMode,
}

impl ContentPanel {
    pub fn new(entries: Vec<HistoryEntry>) -> Self {
        Self {
            entries,
            stream_buffer: String::new(),
            streaming_visible: String::new(),
            scroll: ScrollMode::Auto,
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
        let up_step = match key {
            KeyAction::Up => Some(1),
            KeyAction::PageUp => Some(20),
            KeyAction::Home => {
                self.scroll = ScrollMode::Manual(100_000);
                return DispatchResult::Consumed(Vec::new());
            }
            _ => None,
        };
        if let Some(step) = up_step {
            self.scroll = match self.scroll {
                ScrollMode::Auto => ScrollMode::Manual(step),
                ScrollMode::Manual(current) => {
                    ScrollMode::Manual(current.saturating_add(step).min(100_000))
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
                ScrollMode::Auto => ScrollMode::Auto,
                ScrollMode::Manual(current) => {
                    let next = current.saturating_sub(step);
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

        let lines = self.flatten(inner.height as usize, inner.width as usize, ctx.exec_state);
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(TXT).bg(BG))
            .wrap(Wrap { trim: false });
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
    }

    pub fn push_error(&mut self, text: String) {
        self.entries.push(HistoryEntry::Error(text));
    }

    pub fn begin_stream(&mut self) {
        self.stream_buffer.clear();
        self.streaming_visible.clear();
        self.scroll = ScrollMode::Auto;
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

    fn flatten(&self, visible: usize, width: usize, exec_state: &ExecState) -> Vec<Line<'static>> {
        let mut all = Vec::new();

        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                all.push(blank_line(width));
            }
            render_history_entry(entry, width, &mut all);
        }

        if !self.streaming_visible.trim().is_empty() {
            if !all.is_empty() {
                all.push(blank_line(width));
            }
            let streaming = HistoryEntry::Assistant(parse_assistant_message(
                &self.streaming_visible,
                stream_tool_state(Some(exec_state)),
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

        let total = all.len();
        let skip = match self.scroll {
            ScrollMode::Auto => 0,
            ScrollMode::Manual(lines) => lines.min(total.saturating_sub(1)),
        };
        let start = total.saturating_sub(skip.saturating_add(visible));
        all.into_iter()
            .skip(start)
            .take(visible.saturating_add(skip))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::panels::AssistantPart;

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
}
