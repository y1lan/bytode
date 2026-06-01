use crate::ui::events::{DispatchResult, KeyAction, PanelId, WindowSlot, WindowSpec};
use crate::ui::panels::{panel_block, BLUE, CYAN, RED, RenderContext, UiContext, PanelContext, TXT};
use crate::ui::render;
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};

#[derive(Clone, Debug)]
pub enum HistoryEntry {
    User(String),
    Assistant(String),
    Tool { name: String, summary: String },
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollMode {
    Auto,
    Manual(usize),
}

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
                ScrollMode::Manual(current) => ScrollMode::Manual(current.saturating_add(step).min(100_000)),
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
                    if next == 0 { ScrollMode::Auto } else { ScrollMode::Manual(next) }
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

        let lines = self.flatten(inner.height as usize);
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(TXT))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }

    pub fn push_user(&mut self, text: String) {
        self.entries.push(HistoryEntry::User(text));
        self.scroll = ScrollMode::Auto;
    }

    pub fn push_assistant(&mut self, text: String) {
        self.entries.push(HistoryEntry::Assistant(text));
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
        if final_text.is_empty() {
            return;
        }

        let mut assistant = String::new();
        let mut tool_summary = String::new();
        let mut tool_name = String::new();
        let mut in_tool = false;

        for line in final_text.lines() {
            if line.contains('\u{27f3}') {
                if in_tool && !tool_summary.trim().is_empty() {
                    self.entries.push(HistoryEntry::Tool { name: tool_name.clone(), summary: tool_summary.trim().to_string() });
                } else if !assistant.trim().is_empty() {
                    self.entries.push(HistoryEntry::Assistant(assistant.trim().to_string()));
                    assistant.clear();
                }
                in_tool = true;
                tool_name = extract_tool_name(line);
                tool_summary.clear();
                tool_summary.push_str(&format!("{}(", tool_name));
                tool_summary.push('\n');
                continue;
            }

            if in_tool {
                if line.trim().is_empty() {
                    self.entries.push(HistoryEntry::Tool { name: tool_name.clone(), summary: tool_summary.trim().to_string() });
                    tool_summary.clear();
                    tool_name.clear();
                    in_tool = false;
                } else {
                    tool_summary.push_str(line);
                    tool_summary.push('\n');
                }
                continue;
            }

            assistant.push_str(line);
            assistant.push('\n');
        }

        if in_tool && !tool_summary.trim().is_empty() {
            self.entries.push(HistoryEntry::Tool { name: tool_name, summary: tool_summary.trim().to_string() });
        } else if !assistant.trim().is_empty() {
            self.entries.push(HistoryEntry::Assistant(assistant.trim().to_string()));
        }
    }

    fn flatten(&self, visible: usize) -> Vec<Line<'static>> {
        let mut all = Vec::new();

        for entry in &self.entries {
            match entry {
                HistoryEntry::User(message) => {
                    all.push(Line::from(Span::styled(
                        format!("   \u{25b8} {message}"),
                        Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
                    )));
                }
                HistoryEntry::Assistant(text) => {
                    for line in render::render_md(text) {
                        all.push(own(line));
                    }
                }
                HistoryEntry::Tool { name, summary } => {
                    all.push(Line::from(Span::styled(
                        format!("   {name}(...)"),
                        Style::default().fg(CYAN).add_modifier(Modifier::DIM),
                    )));
                    if !summary.is_empty() {
                        for line in render::render_md(summary) {
                            all.push(own(line));
                        }
                    }
                }
                HistoryEntry::Error(message) => {
                    all.push(Line::from(Span::styled(format!("   {message}"), Style::default().fg(RED))));
                }
            }

            all.push(Line::from(Span::styled(
                "\u{2500}".repeat(60),
                Style::default().fg(Color::Rgb(204, 208, 218)),
            )));
        }

        if !self.streaming_visible.is_empty() {
            for line in render::render_md(&self.streaming_visible) {
                all.push(own(line));
            }
        }

        let total = all.len();
        let skip = match self.scroll {
            ScrollMode::Auto => 0,
            ScrollMode::Manual(lines) => lines.min(total.saturating_sub(1)),
        };
        let start = total.saturating_sub(skip.saturating_add(visible));
        all.into_iter().skip(start).take(visible.saturating_add(skip)).collect()
    }
}

fn own(line: Line<'_>) -> Line<'static> {
    Line::from(
        line.spans
            .iter()
            .map(|span| Span::styled(span.content.to_string(), span.style))
            .collect::<Vec<_>>(),
    )
}

fn extract_tool_name(line: &str) -> String {
    line.split('\u{27f3}')
        .nth(1)
        .unwrap_or("")
        .trim()
        .split('(')
        .next()
        .unwrap_or("?")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_stream_splits_tool_sections() {
        let mut panel = ContentPanel::new(Vec::new());
        panel.begin_stream();
        panel.append_stream_chunk("hello\n  ⟳ read_file(src/main.rs)\n  ok\n");
        panel.finalize_stream();

        assert!(matches!(panel.entries[0], HistoryEntry::Assistant(_)));
        assert!(matches!(panel.entries[1], HistoryEntry::Tool { .. }));
    }
}
