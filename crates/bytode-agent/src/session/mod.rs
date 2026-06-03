//! Session subsystem: append-only log, artifact store, deterministic micro
//! compact, and context rendering. The Agent only ever touches this through
//! [`SessionRuntime`].

pub mod compact;
pub mod context;
pub mod model;
pub mod runtime;
pub mod store;

pub use compact::{CompactOverlay, MicroCompactPolicy};
pub use context::{RenderedEntry, render_context_entries};
pub use model::{
    ArtifactId, ArtifactKind, ArtifactRef, AssistantMessageEntry, CompactedEntryView, EntryId,
    EntryMeta, MicroCompactEntry, MicroCompactResult, SessionEntry, SessionEntryKind, SessionId,
    ToolCallEntry, ToolResultEntry, ToolStatus, UserMessageEntry,
};
pub use runtime::SessionRuntime;
pub use store::fs::{
    default_session_id, new_session_id, project_hash, session_root, sessions_root,
};
pub use store::{SessionState, SessionStore};
