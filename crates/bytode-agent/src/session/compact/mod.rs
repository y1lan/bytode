//! Deterministic micro compact and the derived compact overlay. No disk IO and
//! no agent instrumentation live here.

pub mod micro;
pub mod overlay;

use crate::session::model::ArtifactKind;
use serde::{Deserialize, Serialize};

pub use micro::run_micro_compact;
pub use overlay::CompactOverlay;

/// Tunables for a micro compact pass. Snapshotted into every `MicroCompactEntry`
/// so the audit log records exactly which policy produced a given result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicroCompactPolicy {
    pub keep_recent_entries: usize,
    pub keep_recent_turns: usize,

    pub max_inline_tool_output_chars: usize,
    pub max_inline_command_output_chars: usize,
    pub max_inline_file_snapshot_chars: usize,
    pub max_inline_diff_chars: usize,
    pub max_inline_diagnostics_chars: usize,

    pub preserve_user_messages: bool,
    pub preserve_recent_tool_results: bool,
}

impl Default for MicroCompactPolicy {
    fn default() -> Self {
        MicroCompactPolicy {
            keep_recent_turns: 6,
            keep_recent_entries: 40,
            max_inline_tool_output_chars: 4000,
            max_inline_command_output_chars: 4000,
            max_inline_file_snapshot_chars: 6000,
            max_inline_diff_chars: 8000,
            max_inline_diagnostics_chars: 4000,
            preserve_user_messages: true,
            preserve_recent_tool_results: true,
        }
    }
}

impl MicroCompactPolicy {
    /// Compact-time inline char budget for the given payload kind.
    pub fn inline_limit(&self, kind: ArtifactKind) -> usize {
        match kind {
            ArtifactKind::FileSnapshot => self.max_inline_file_snapshot_chars,
            ArtifactKind::Diagnostics => self.max_inline_diagnostics_chars,
            ArtifactKind::Diff => self.max_inline_diff_chars,
            ArtifactKind::CommandOutput => self.max_inline_command_output_chars,
            _ => self.max_inline_tool_output_chars,
        }
    }
}

/// Maps a tool name to the artifact kind its output is archived under. Used to
/// classify inline tool results that were never archived at record time.
pub fn kind_for_tool(tool_name: &str) -> ArtifactKind {
    match tool_name {
        "read_file" => ArtifactKind::FileSnapshot,
        "get_diagnostics" => ArtifactKind::Diagnostics,
        "search_code" => ArtifactKind::SearchResult,
        "git_diff" | "write_file" => ArtifactKind::Diff,
        "cargo" | "cargo_check" => ArtifactKind::CommandOutput,
        _ => ArtifactKind::ToolOutput,
    }
}
