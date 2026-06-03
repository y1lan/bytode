//! Serializable artifact references. Large payloads live on disk; the log only
//! ever stores an `ArtifactRef` that can be resolved back to the original bytes.

use super::entry::EntryId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable, content-addressed artifact identifier (sha256 hex of the payload).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Classification of an archived payload. Drives the `artifacts/` sub-directory
/// and the rendered preview header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    ToolOutput,
    ToolArgs,
    CommandOutput,
    FileSnapshot,
    Diff,
    Diagnostics,
    SearchResult,
    CompactArchive,
}

impl ArtifactKind {
    /// `artifacts/<subdir>/` for this kind.
    pub fn subdir(self) -> &'static str {
        match self {
            ArtifactKind::ToolArgs => "args",
            ArtifactKind::ToolOutput => "tool",
            ArtifactKind::CommandOutput => "command",
            ArtifactKind::FileSnapshot => "file",
            ArtifactKind::Diff => "diff",
            ArtifactKind::Diagnostics => "diagnostics",
            ArtifactKind::SearchResult => "search",
            ArtifactKind::CompactArchive => "compact",
        }
    }
}

/// A pointer to a payload stored under the session root. Never stores an
/// absolute path; `relative_path` is resolved against the session root and
/// `sha256` guards integrity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub relative_path: PathBuf,
    pub source_entry_id: EntryId,
    pub byte_len: u64,
    pub sha256: String,
    pub preview: String,
}
