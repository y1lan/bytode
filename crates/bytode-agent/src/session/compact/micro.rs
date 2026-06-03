//! Deterministic `/compact micro`.
//!
//! Safety invariants (enforced here and covered by tests):
//! - never touches `UserMessage` entries (user words / corrections / acceptance)
//! - never rewrites `log.jsonl` history — only appends a `MicroCompactEntry`
//! - never summarizes task facts, never judges task completion, never calls an LLM
//! - only archives old, oversized runtime noise (tool output, stdout/stderr,
//!   file snapshots, diffs, diagnostics, search results)
//! - is manual only; nothing here triggers automatically

use crate::error::Result;
use crate::session::compact::{MicroCompactPolicy, kind_for_tool};
use crate::session::model::{
    ArtifactRef, EntryId, MicroCompactEntry, MicroCompactResult, SessionEntry, SessionEntryKind,
};
use crate::session::store::SessionStore;
use std::collections::{HashMap, HashSet};

/// Run one deterministic micro compact pass over the store's log.
pub fn run_micro_compact(
    store: &mut SessionStore,
    policy: &MicroCompactPolicy,
) -> Result<MicroCompactResult> {
    let entries = store.replay_entries()?;

    let tool_names = tool_name_index(&entries);
    let already_compacted = already_compacted_ids(&entries);
    let preserve_from = preserve_cutoff(&entries, policy);

    let mut archived_artifacts: Vec<ArtifactRef> = Vec::new();
    let mut compacted_entry_ids: Vec<EntryId> = Vec::new();
    let mut saved_bytes_estimate: usize = 0;

    for (idx, entry) in entries.iter().enumerate() {
        if idx >= preserve_from {
            break;
        }
        let SessionEntryKind::ToolResult(result) = &entry.kind else {
            continue;
        };
        if already_compacted.contains(&entry.meta.id) {
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
        if content.chars().count() <= policy.inline_limit(kind) {
            continue;
        }

        let art = store.artifacts().write(&entry.meta, kind, content)?;
        saved_bytes_estimate += content.len().saturating_sub(art.preview.len());
        archived_artifacts.push(art);
        compacted_entry_ids.push(entry.meta.id.clone());
    }

    let operation_digest = format!(
        "MicroCompact archived {} old tool outputs and replaced inline content with previews.",
        compacted_entry_ids.len()
    );

    let meta = store.next_meta();
    let compact_entry = SessionEntry {
        meta,
        kind: SessionEntryKind::MicroCompact(MicroCompactEntry {
            compacted_entry_ids: compacted_entry_ids.clone(),
            archived_artifacts: archived_artifacts.clone(),
            policy_snapshot: policy.clone(),
            operation_digest: operation_digest.clone(),
        }),
    };
    let compact_entry_id = store.commit(compact_entry)?;

    Ok(MicroCompactResult {
        compact_entry_id,
        archived_artifacts,
        compacted_entry_ids,
        saved_bytes_estimate,
        operation_digest,
    })
}

/// Map each tool-call entry id to its tool name.
fn tool_name_index(entries: &[SessionEntry]) -> HashMap<EntryId, String> {
    let mut map = HashMap::new();
    for entry in entries {
        if let SessionEntryKind::ToolCall(call) = &entry.kind {
            map.insert(entry.meta.id.clone(), call.tool_name.clone());
        }
    }
    map
}

/// Entry ids already archived by a previous compact pass — never compact twice.
fn already_compacted_ids(entries: &[SessionEntry]) -> HashSet<EntryId> {
    let mut set = HashSet::new();
    for entry in entries {
        if let SessionEntryKind::MicroCompact(mc) = &entry.kind {
            for id in &mc.compacted_entry_ids {
                set.insert(id.clone());
            }
        }
    }
    set
}

/// Index below which entries are eligible for compaction. Entries at or after
/// this index are preserved because they fall within the recent-entry window
/// or the recent-turn window (whichever preserves more).
fn preserve_cutoff(entries: &[SessionEntry], policy: &MicroCompactPolicy) -> usize {
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
