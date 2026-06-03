//! Deterministic micro compact and the derived compact overlay. No disk IO and
//! no agent instrumentation live here.
//!
//! Prefix-cache cost principle (design note only — execution uses a
//! provider-neutral heuristic, never a price model):
//!
//! MicroCompact changes old context entries near the beginning of the reusable
//! prefix. Frequent small compactions can invalidate prefix cache repeatedly.
//! Unbounded delay can make a future compaction invalidate an even larger
//! suffix. Automatic triggering therefore combines pressure, cold-resume,
//! periodic turn interval, and bloat heuristics.

pub mod micro;
pub mod overlay;

use crate::session::model::{ArtifactKind, EntryId};
use serde::{Deserialize, Serialize};

pub use micro::{estimate_micro_compact, run_micro_compact};
pub use overlay::CompactOverlay;

/// Why an automatic micro compact pass was attempted. Reported back from the
/// runtime for tracing; never decided by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroCompactTriggerReason {
    ContextPressure,
    BloatPressure,
    ColdResume,
    TurnInterval,
}

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

    pub auto_enabled: bool,
    pub context_pressure_ratio: f64,
    pub min_compactable_bytes: usize,
    pub min_saved_bytes: usize,
    pub min_compression_ratio: f64,
    pub min_entries_since_last_compact: usize,
    pub turn_interval: usize,
    pub cold_resume_after_secs: u64,
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
            auto_enabled: true,
            context_pressure_ratio: 0.75,
            min_compactable_bytes: 64 * 1024,
            min_saved_bytes: 32 * 1024,
            min_compression_ratio: 3.0,
            min_entries_since_last_compact: 20,
            turn_interval: 8,
            cold_resume_after_secs: 600,
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

/// One eligible entry that can be compacted. Yielded by the shared selection
/// function so estimate and execution operate on identical candidates.
#[derive(Debug, Clone)]
pub struct MicroCompactCandidate {
    pub entry_id: EntryId,
    pub kind: ArtifactKind,
    pub content_len: usize,
    pub estimated_preview_len: usize,
}

/// Deterministic pre-flight estimate of a micro compact pass. Computed from the
/// same candidates that `run_micro_compact` would process.
#[derive(Debug, Clone)]
pub struct MicroCompactEstimate {
    pub compactable_entry_ids: Vec<EntryId>,
    pub compactable_bytes: usize,
    pub estimated_compacted_bytes: usize,
    pub estimated_saved_bytes: usize,
    pub estimated_compression_ratio: f64,
}

/// Shared selection of compactable entries. Estimate and execution both call
/// this so they agree on exactly which entries are eligible.
///
/// Excludes: user messages, assistant messages, entries already compacted,
/// entries within `keep_recent_entries` / `keep_recent_turns`, and entries
/// whose inline content is within the kind-specific char budget.
pub fn select_micro_compact_candidates(
    entries: &[crate::session::model::SessionEntry],
    overlay: &overlay::CompactOverlay,
    policy: &MicroCompactPolicy,
) -> Vec<MicroCompactCandidate> {
    use crate::session::model::SessionEntryKind;

    let preserve_from = preserve_cutoff(entries, policy);
    let tool_names = tool_name_index(entries);

    let mut candidates = Vec::new();

    for (idx, entry) in entries.iter().enumerate() {
        if idx >= preserve_from {
            break;
        }
        let SessionEntryKind::ToolResult(result) = &entry.kind else {
            continue;
        };
        if overlay.view_for(&entry.meta.id).is_some() {
            continue;
        }
        let Some(content) = &result.inline_content else {
            continue;
        };
        let tool = tool_names
            .get(&result.call_entry_id)
            .map(String::as_str)
            .unwrap_or("");
        let kind = kind_for_tool(tool);
        let char_count = content.chars().count();
        if char_count <= policy.inline_limit(kind) {
            continue;
        }

        let preview_len = estimate_preview_chars(content);
        candidates.push(MicroCompactCandidate {
            entry_id: entry.meta.id.clone(),
            kind,
            content_len: char_count,
            estimated_preview_len: preview_len,
        });
    }

    candidates
}

fn estimate_preview_chars(content: &str) -> usize {
    const MAX_CHARS: usize = 800;
    const MAX_LINES: usize = 20;
    let mut count = 0;
    let mut lines = 0;
    for ch in content.chars() {
        if count >= MAX_CHARS || lines >= MAX_LINES {
            break;
        }
        if ch == '\n' {
            lines += 1;
        }
        count += 1;
    }
    if count < content.chars().count() {
        count += "...".len();
    }
    count
}

/// Index below which entries are eligible for compaction. Entries at or after
/// this index are preserved because they fall within the recent-entry window
/// or the recent-turn window (whichever preserves more).
fn preserve_cutoff(
    entries: &[crate::session::model::SessionEntry],
    policy: &MicroCompactPolicy,
) -> usize {
    use crate::session::model::SessionEntryKind;

    let by_entries = entries.len().saturating_sub(policy.keep_recent_entries);

    let user_idxs: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, SessionEntryKind::UserMessage(_)))
        .map(|(i, _)| i)
        .collect();
    let by_turns = if policy.keep_recent_turns == 0 {
        entries.len()
    } else if user_idxs.len() > policy.keep_recent_turns {
        user_idxs[user_idxs.len() - policy.keep_recent_turns]
    } else {
        0
    };

    by_entries.min(by_turns)
}

fn tool_name_index(
    entries: &[crate::session::model::SessionEntry],
) -> std::collections::HashMap<EntryId, String> {
    use crate::session::model::SessionEntryKind;
    let mut map = std::collections::HashMap::new();
    for entry in entries {
        if let SessionEntryKind::ToolCall(call) = &entry.kind {
            map.insert(entry.meta.id.clone(), call.tool_name.clone());
        }
    }
    map
}
