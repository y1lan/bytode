use ratatui::prelude::*;
use ratatui::text::Line;
use std::borrow::Cow;

/// A single chat message with type discrimination.
#[derive(Clone)]
pub enum ChatMessage {
    /// A user-typed command or question.
    User(String),
    /// Assistant response text.
    Assistant(String),
    /// A tool call (name + arguments).
    ToolCall { name: String, args: String },
    /// A tool result (raw text).
    ToolResult(String),
    /// An error message.
    Error(String),
    /// System / slash-command result.
    System(String),
}

impl ChatMessage {
    /// True if this message was sent by the user.
    pub fn is_user(&self) -> bool {
        matches!(self, ChatMessage::User(_))
    }
}

/// Convert borrowed `Line<'_>` into fully-owned `Line<'static>`.
fn to_static(lines: Vec<Line<'_>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let spans: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|span| Span {
                    content: Cow::Owned(span.content.into_owned()),
                    style: span.style,
                })
                .collect();
            Line {
                spans,
                alignment: line.alignment,
                style: line.style,
            }
        })
        .collect()
}

/// Pre-rendered line cache for a single chat message.
#[derive(Clone)]
struct CachedEntry {
    lines: Vec<Line<'static>>,
    line_count: usize,
    is_user: bool,
}

/// Grow-only chat history with pre-rendered line cache.
#[derive(Clone)]
pub struct ChatHistory {
    entries: Vec<CachedEntry>,
    cumulative: Vec<usize>,
    total_lines: usize,
}

impl ChatHistory {
    pub fn new() -> Self {
        ChatHistory {
            entries: Vec::new(),
            cumulative: Vec::new(),
            total_lines: 1,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Push a message, rendering and caching it immediately.
    pub fn push(&mut self, msg: ChatMessage) {
        let is_user = msg.is_user();
        let lines = match &msg {
            ChatMessage::User(text) => {
                vec![
                    Line::from(Span::styled(
                        format!("\u{25b8} {text}"),
                        Style::default()
                            .fg(Color::Rgb(30, 102, 245))
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                ]
            }
            ChatMessage::Assistant(text)
            | ChatMessage::ToolResult(text)
            | ChatMessage::Error(text)
            | ChatMessage::System(text) => {
                let mut v = super::render::render_md(text);
                v.push(Line::from(""));
                to_static(v)
            }
            ChatMessage::ToolCall { name, args } => {
                let text = format!("  \u{27f3} {name}({args})");
                let mut v = super::render::render_md(&text);
                v.push(Line::from(""));
                to_static(v)
            }
        };

        let lc = lines.len();
        self.cumulative.push(self.total_lines);
        self.total_lines += lc;
        self.entries.push(CachedEntry {
            lines,
            line_count: lc,
            is_user,
        });
    }

    /// Replace the last entry.
    pub fn replace_last(&mut self, msg: ChatMessage) {
        if let Some(e) = self.entries.last() {
            self.total_lines -= e.line_count;
        }
        self.entries.pop();
        self.cumulative.pop();
        self.push(msg);
    }

    /// Total number of rendered lines.
    pub fn total_lines(&self) -> usize {
        self.total_lines
    }

    /// Get a slice of pre-rendered lines for the visible area.
    /// `scroll_offset`: lines scrolled from bottom (0 = at bottom).
    pub fn visible_lines(&self, scroll_offset: usize, height: u16) -> Vec<Line<'static>> {
        let h = height as usize;
        if self.total_lines <= h || scroll_offset == 0 {
            let start = self.total_lines.saturating_sub(h);
            self.slice(start, h)
        } else {
            let bottom = self.total_lines.saturating_sub(scroll_offset);
            let start = bottom.saturating_sub(h);
            self.slice(start, h)
        }
    }

    fn slice(&self, start_line: usize, max: usize) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(max);
        let mut cursor = 0;
        for entry in &self.entries {
            let end = cursor + entry.line_count;
            if end <= start_line {
                cursor = end;
                continue;
            }
            if out.len() >= max {
                break;
            }
            let skip = start_line.saturating_sub(cursor);
            let take = (entry.line_count - skip).min(max - out.len());
            for l in entry.lines.iter().skip(skip).take(take) {
                out.push(l.clone());
            }
            cursor = end;
        }
        out
    }
}

/// Scroll state machine.
#[derive(Clone, Copy)]
pub enum ScrollState {
    Following,
    Pinned { offset: usize },
}

impl ScrollState {
    pub fn offset(&self) -> usize {
        match self {
            ScrollState::Following => 0,
            ScrollState::Pinned { offset } => *offset,
        }
    }

    pub fn scroll_up(&mut self, step: usize, max: usize) {
        let cur = self.offset();
        *self = ScrollState::Pinned {
            offset: (cur + step).min(max),
        };
    }

    pub fn scroll_down(&mut self, step: usize) {
        match self {
            ScrollState::Following => {}
            ScrollState::Pinned { offset } => {
                if *offset <= step {
                    *self = ScrollState::Following;
                } else {
                    *offset -= step;
                }
            }
        }
    }

    pub fn reset_to_follow(&mut self) {
        *self = ScrollState::Following;
    }

    pub fn is_following(&self) -> bool {
        matches!(self, ScrollState::Following)
    }
}

/// Real-time streaming state, published from LLM task to render loop.
#[derive(Clone)]
pub struct StreamingState {
    pub text: String,
    pub current_tool: String,
    pub error: Option<String>,
}

impl StreamingState {
    pub fn new() -> Self {
        StreamingState {
            text: String::new(),
            current_tool: String::new(),
            error: None,
        }
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.current_tool.clear();
        self.error = None;
    }

    pub fn task_label(&self) -> String {
        if self.current_tool.is_empty() {
            "thinking...".into()
        } else {
            format!("{}()...", self.current_tool)
        }
    }
}
