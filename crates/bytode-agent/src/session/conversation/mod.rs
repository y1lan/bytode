pub mod compact;
pub mod model;
pub mod render;
pub mod store;

pub use compact::{canonical_evidence, select_canonical_source_span};
pub use model::{
    CanonicalAssistantPart, CanonicalAssistantResponse, CanonicalCompactReplacement,
    CanonicalContent, CanonicalMessageId, CanonicalMeta, CanonicalRecord, CanonicalRecordKind,
    CanonicalSpan, CanonicalToolResultCompacted, CanonicalToolResultPart, CanonicalToolResults,
    CanonicalToolStatus, CanonicalTurnFinished, CanonicalTurnStarted, ResponseId, ToolCallId,
    TurnId,
};
pub use render::{CanonicalContext, build_canonical_context};
pub use store::CanonicalConversationStore;
