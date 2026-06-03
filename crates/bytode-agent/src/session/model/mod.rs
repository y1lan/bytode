//! Serializable session data structures. No disk IO, no compact execution, no
//! context rendering lives here.

pub mod artifact;
pub mod entry;

pub use artifact::{ArtifactId, ArtifactKind, ArtifactRef};
pub use entry::{
    AssistantMessageEntry, CompactedEntryView, EntryId, EntryMeta, MicroCompactEntry,
    MicroCompactResult, SessionEntry, SessionEntryKind, SessionId, ToolCallEntry, ToolResultEntry,
    ToolStatus, UserMessageEntry,
};
