//! Interactive compact session management.
//!
//! An interactive compact session has its own `SessionStore` (and therefore
//! its own `log.jsonl`) so intermediate messages never pollute the main
//! session. The compact session receives no project tools and the LLM is
//! called without a ReAct tool-loop — it only produces text responses.

use crate::error::Result;
use crate::session::compact::{CompactOverlay, MicroCompactPolicy, preserve_cutoff};
use crate::session::model::{
    ArtifactKind, ArtifactRef, EntryMeta, EntrySpan, InteractiveCompactEntry,
    InteractiveCompactOutcome, InteractiveCompactResult, SessionEntry, SessionEntryKind, SessionId,
};
use crate::session::store::SessionStore;
use crate::session::store::fs::{new_session_id, sessions_root, sha256_hex};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Output from one turn of an interactive compact session.
#[derive(Debug, Clone)]
pub struct InteractiveCompactOutput {
    pub content: String,
}

/// In-memory state for an active interactive compact session. The compact
/// session has its own `SessionStore` for its own `log.jsonl`; messages
/// exchanged during compaction are persisted there, not in the main session.
pub struct InteractiveCompactState {
    pub source_range: EntrySpan,
    pub evidence_pack_ref: ArtifactRef,
    pub compact_store: SessionStore,
    /// Accumulated LLM conversation for the compact session (system prompt +
    /// user turns + assistant responses). Mirrors what is in `compact_store`.
    pub messages: Vec<async_openai::types::ChatCompletionRequestMessage>,
    /// Latest assistant response — the candidate for `/commit`.
    pub current_content: String,
}

/// Select a contiguous source range suitable for interactive compact.
/// Returns `None` when all old entries are already covered or when the only
/// remaining entries are within the recent-turn window.
pub fn select_source_range(
    entries: &[SessionEntry],
    policy: &MicroCompactPolicy,
) -> Option<EntrySpan> {
    if entries.is_empty() {
        return None;
    }

    let mut covered_until: u64 = entries.first().map(|e| e.meta.seq).unwrap_or(0);
    for entry in entries {
        if let SessionEntryKind::InteractiveCompact(ic) = &entry.kind {
            if matches!(ic.outcome, InteractiveCompactOutcome::Committed) {
                covered_until = covered_until.max(ic.source_range.end_seq_exclusive);
            }
        }
    }

    let cutoff_idx = preserve_cutoff(entries, policy);
    let cutoff_seq = if cutoff_idx < entries.len() {
        entries[cutoff_idx].meta.seq
    } else {
        entries
            .last()
            .map(|e| e.meta.seq.saturating_add(1))
            .unwrap_or(0)
    };

    if covered_until >= cutoff_seq {
        return None;
    }

    Some(EntrySpan {
        start_seq: covered_until,
        end_seq_exclusive: cutoff_seq,
    })
}

/// Generate an evidence pack artifact containing entry excerpts, artifact
/// refs, and checksum metadata for every entry in the source range.
pub fn generate_evidence_pack(
    entries: &[SessionEntry],
    source_range: &EntrySpan,
    overlay: &CompactOverlay,
    artifact_store: &crate::session::store::fs::ArtifactStore,
    session_id: &SessionId,
) -> Result<ArtifactRef> {
    let _tool_names = tool_name_index(entries);
    let mut ev_entries: Vec<Value> = Vec::new();
    let mut total_byte_len = 0usize;

    for entry in entries {
        if entry.meta.seq < source_range.start_seq
            || entry.meta.seq >= source_range.end_seq_exclusive
        {
            continue;
        }
        let (kind_str, excerpt) = entry_excerpt(entry, overlay);
        let artifact_refs = entry_artifact_refs(entry);
        let mut item = serde_json::json!({
            "seq": entry.meta.seq,
            "kind": kind_str,
            "excerpt": excerpt,
        });
        if !artifact_refs.is_empty() {
            let refs: Vec<Value> = artifact_refs
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "relative_path": a.relative_path.display().to_string(),
                        "sha256": a.sha256,
                        "byte_len": a.byte_len,
                    })
                })
                .collect();
            item["artifact_refs"] = serde_json::Value::Array(refs);
        }
        // Record overlay information when this entry was micro-compacted.
        if let Some(view) = overlay.view_for(&entry.meta.id) {
            item["overlay_compact_entry_id"] =
                serde_json::Value::String(view.compact_entry_id.0.clone());
            if !view.artifact_refs.is_empty() {
                let ov_refs: Vec<Value> = view
                    .artifact_refs
                    .iter()
                    .map(|a| {
                        serde_json::json!({
                            "relative_path": a.relative_path.display().to_string(),
                            "sha256": a.sha256,
                            "byte_len": a.byte_len,
                        })
                    })
                    .collect();
                item["overlay_artifact_refs"] = serde_json::Value::Array(ov_refs);
            }
        }
        // Per-entry checksum for integrity tracking.
        item["excerpt_sha256"] = serde_json::Value::String(sha256_hex(excerpt.as_bytes()));
        ev_entries.push(item);
        total_byte_len += excerpt.len();
    }

    let pack_str = {
        let pack = serde_json::json!({
            "source_session_id": session_id.as_str(),
            "source_range": {
                "start_seq": source_range.start_seq,
                "end_seq_exclusive": source_range.end_seq_exclusive,
            },
            "entry_count": ev_entries.len(),
            "total_excerpt_bytes": total_byte_len,
            "entries": ev_entries,
        });
        serde_json::to_string_pretty(&pack).unwrap_or_default()
    };

    let meta = EntryMeta {
        id: crate::session::model::EntryId::from_seq(0),
        seq: 0,
        created_at: now_secs(),
    };
    artifact_store.write(&meta, ArtifactKind::CompactArchive, &pack_str)
}

/// Create an independent compact session with its own directory and log.
pub fn create_compact_store(project_root: &std::path::Path) -> Result<SessionStore> {
    let root = sessions_root()?;
    let compact_id = new_session_id(project_root, now_secs() as u64);
    let compact_root = root.join(compact_id.as_str());
    SessionStore::open(compact_id, compact_root)
}

/// Build the initial system prompt for the compact session.
pub fn build_compact_system_msg(
    entries: &[SessionEntry],
    source_range: &EntrySpan,
    overlay: &CompactOverlay,
) -> async_openai::types::ChatCompletionRequestMessage {
    let mut evidence = String::new();
    for entry in entries {
        if entry.meta.seq < source_range.start_seq
            || entry.meta.seq >= source_range.end_seq_exclusive
        {
            continue;
        }
        let (kind, excerpt) = entry_excerpt(entry, overlay);
        evidence.push_str(&format!(
            "  [seq {}] {}: {}\n",
            entry.meta.seq, kind, excerpt
        ));
    }

    let prompt = format!(
        r#"You are compressing a range of conversation history. Produce a concise
replacement that preserves all essential facts, decisions, constraints, and
unfinished items from the source range. Rules:

1. Do NOT fabricate facts not present in the evidence.
2. Do NOT judge task completion or declare work finished.
3. Retain: user requests, tool results, file modifications, diagnostics,
   error messages, and unfinished items.
4. Omit: redundant tool output, repeated search results, verbose diffs
   when the outcome is already stated.
5. Output ONLY the compressed content — no preamble, no commentary.

Source range: seq {}-{}

Evidence:
{}"#,
        source_range.start_seq,
        source_range.end_seq_exclusive.saturating_sub(1),
        evidence,
    );

    crate::llm::build_system_message(&prompt)
}

/// Commit an `InteractiveCompactEntry` to the main session store.
pub fn commit_to_main_store(
    main_store: &mut SessionStore,
    state: &InteractiveCompactState,
) -> Result<InteractiveCompactEntry> {
    let ic_entry = InteractiveCompactEntry {
        source_range: state.source_range.clone(),
        compact_session_id: state.compact_store.session_id().clone(),
        outcome: InteractiveCompactOutcome::Committed,
        result: Some(InteractiveCompactResult {
            content: state.current_content.clone(),
            evidence_pack_ref: state.evidence_pack_ref.clone(),
        }),
        operation_digest: format!(
            "InteractiveCompact committed: replaced seq {}-{}",
            state.source_range.start_seq, state.source_range.end_seq_exclusive
        ),
    };

    let meta = main_store.next_meta();
    let entry = SessionEntry {
        meta,
        kind: SessionEntryKind::InteractiveCompact(ic_entry.clone()),
    };
    main_store.commit(entry)?;

    Ok(ic_entry)
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn entry_excerpt(entry: &SessionEntry, overlay: &CompactOverlay) -> (&'static str, String) {
    // If a MicroCompact overlay provides a view, use its preview as the excerpt.
    if let Some(view) = overlay.view_for(&entry.meta.id) {
        return ("ToolResult-compacted", view.replacement_preview.clone());
    }

    match &entry.kind {
        SessionEntryKind::UserMessage(u) => ("UserMessage", truncate_str(&u.content, 400)),
        SessionEntryKind::AssistantMessage(a) => {
            ("AssistantMessage", truncate_str(&a.content, 400))
        }
        SessionEntryKind::ToolCall(c) => (
            "ToolCall",
            format!(
                "{} {}",
                c.tool_name,
                truncate_str(
                    &c.inline_args
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    200,
                )
            ),
        ),
        SessionEntryKind::ToolResult(r) => {
            let preview = r
                .preview
                .clone()
                .unwrap_or_else(|| truncate_str(r.inline_content.as_deref().unwrap_or(""), 300));
            let tool = r.call_entry_id.0.clone(); // approximate — caller has tool_name_index
            ("ToolResult", format!("[{}] {}", tool, preview))
        }
        SessionEntryKind::MicroCompact(mc) => ("MicroCompact", mc.operation_digest.clone()),
        SessionEntryKind::InteractiveCompact(ic) => {
            ("InteractiveCompact", ic.operation_digest.clone())
        }
    }
}

fn entry_artifact_refs(entry: &SessionEntry) -> Vec<&ArtifactRef> {
    match &entry.kind {
        SessionEntryKind::ToolCall(c) => c.arg_artifacts.iter().collect(),
        SessionEntryKind::ToolResult(r) => r.artifacts.iter().collect(),
        _ => Vec::new(),
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push_str("...");
        out
    }
}

fn tool_name_index(entries: &[SessionEntry]) -> HashMap<crate::session::model::EntryId, String> {
    let mut map = HashMap::new();
    for entry in entries {
        if let SessionEntryKind::ToolCall(call) = &entry.kind {
            map.insert(entry.meta.id.clone(), call.tool_name.clone());
        }
    }
    map
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
