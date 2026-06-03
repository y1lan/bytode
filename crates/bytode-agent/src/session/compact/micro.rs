//! Deterministic `/compact micro` (execution) and its pre-flight estimate.
//!
//! Safety invariants (enforced here and covered by tests):
//! - never touches `UserMessage` entries (user words / corrections / acceptance)
//! - never rewrites `log.jsonl` history — only appends a `MicroCompactEntry`
//! - never summarizes task facts, never judges task completion, never calls an LLM
//! - only archives old, oversized runtime noise (tool output, stdout/stderr,
//!   file snapshots, diffs, diagnostics, search results)
//! - manual invocations are removed; only automatic triggers are permitted

use crate::error::Result;
use crate::session::compact::{
    CompactOverlay, MicroCompactEstimate, MicroCompactPolicy, select_micro_compact_candidates,
};
use crate::session::model::{
    MicroCompactEntry, MicroCompactResult, SessionEntry, SessionEntryKind,
};
use crate::session::store::SessionStore;

/// Deterministic pre-flight estimate. Uses the shared selection function so the
/// estimate and the real run agree on candidates.
pub fn estimate_micro_compact(
    entries: &[SessionEntry],
    overlay: &CompactOverlay,
    policy: &MicroCompactPolicy,
) -> MicroCompactEstimate {
    let candidates = select_micro_compact_candidates(entries, overlay, policy);

    let compactable_entry_ids: Vec<_> = candidates.iter().map(|c| c.entry_id.clone()).collect();
    let compactable_bytes: usize = candidates.iter().map(|c| c.content_len).sum();
    let estimated_compacted_bytes: usize = candidates.iter().map(|c| c.estimated_preview_len).sum();
    let estimated_saved_bytes = compactable_bytes.saturating_sub(estimated_compacted_bytes);
    let estimated_compression_ratio = if estimated_compacted_bytes > 0 {
        compactable_bytes as f64 / estimated_compacted_bytes as f64
    } else if compactable_bytes > 0 {
        f64::INFINITY
    } else {
        0.0
    };

    MicroCompactEstimate {
        compactable_entry_ids,
        compactable_bytes,
        estimated_compacted_bytes,
        estimated_saved_bytes,
        estimated_compression_ratio,
    }
}

/// Run one deterministic micro compact pass over the store's log using the
/// shared candidate selection. Only appends a `MicroCompactEntry`; never
/// rewrites history.
pub fn run_micro_compact(
    store: &mut SessionStore,
    policy: &MicroCompactPolicy,
) -> Result<MicroCompactResult> {
    let entries = store.replay_entries()?;
    let overlay = CompactOverlay::from_entries(&entries);
    let candidates = select_micro_compact_candidates(&entries, &overlay, policy);

    if candidates.is_empty() {
        let meta = store.next_meta();
        return Ok(MicroCompactResult {
            compact_entry_id: meta.id,
            compact_entry_seq: meta.seq,
            archived_artifacts: Vec::new(),
            compacted_entry_ids: Vec::new(),
            saved_bytes_estimate: 0,
            operation_digest: "MicroCompact: no eligible old bloat to compact.".into(),
        });
    }

    let mut archived_artifacts = Vec::new();
    let mut compacted_entry_ids = Vec::new();
    let mut saved_bytes_estimate: usize = 0;

    for candidate in &candidates {
        // Resolve the entry so we can read its inline_content.
        let entry = entries
            .iter()
            .find(|e| e.meta.id == candidate.entry_id)
            .expect("candidate entry exists");
        let SessionEntryKind::ToolResult(result) = &entry.kind else {
            continue;
        };
        let Some(content) = &result.inline_content else {
            continue;
        };

        let art = store
            .artifacts()
            .write(&entry.meta, candidate.kind, content)?;
        saved_bytes_estimate += content.len().saturating_sub(art.preview.len());
        archived_artifacts.push(art);
        compacted_entry_ids.push(entry.meta.id.clone());
    }

    let operation_digest = format!(
        "MicroCompact archived {} old tool outputs and replaced inline content with previews.",
        compacted_entry_ids.len()
    );

    let meta = store.next_meta();
    let compact_seq = meta.seq;
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
        compact_entry_seq: compact_seq,
        archived_artifacts,
        compacted_entry_ids,
        saved_bytes_estimate,
        operation_digest,
    })
}
