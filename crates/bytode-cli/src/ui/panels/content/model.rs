pub use bytode_agent::transcript::{
    AssistantMessage, AssistantPart, HistoryEntry, ReasoningPart, TextPart, ToolPart,
    ToolPresentation, ToolState, UserMessage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollMode {
    Auto,
    Manual(usize),
}
