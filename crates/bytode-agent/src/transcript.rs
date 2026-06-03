#[derive(Clone, Debug)]
pub enum HistoryEntry {
    User(UserMessage),
    Assistant(AssistantMessage),
    Error(String),
}

#[derive(Clone, Debug)]
pub struct UserMessage {
    pub body: String,
    pub meta: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AssistantMessage {
    pub parts: Vec<AssistantPart>,
}

impl AssistantMessage {
    pub fn from_text(text: String) -> Self {
        Self {
            parts: vec![AssistantPart::Text(TextPart { content: text })],
        }
    }
}

#[derive(Clone, Debug)]
pub enum AssistantPart {
    Text(TextPart),
    Tool(ToolPart),
    Reasoning(ReasoningPart),
}

#[derive(Clone, Debug)]
pub struct TextPart {
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct ReasoningPart {
    pub summary: String,
    pub content: String,
    pub collapsed: bool,
}

#[derive(Clone, Debug)]
pub struct ToolPart {
    pub name: String,
    pub summary: String,
    pub body: Option<String>,
    pub state: ToolState,
    pub presentation: ToolPresentation,
    pub collapsed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Completed,
    WaitingApproval,
    Failed,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPresentation {
    Inline,
    Block,
}
