//! Interactive compact session management.
//!
//! An interactive compact session has its own `SessionStore` (and therefore
//! its own `log.jsonl`) so intermediate messages never pollute the main
//! session. The compact session receives no project tools and the LLM is
//! called without a ReAct tool-loop — it only produces text responses.

use crate::error::Result;
use crate::session::conversation::{CanonicalRecord, CanonicalSpan, canonical_evidence};
use crate::session::model::{
    ArtifactKind, ArtifactRef, EntryMeta, EntrySpan, InteractiveCompactEntry,
    InteractiveCompactOutcome, InteractiveCompactResult, SessionEntry, SessionEntryKind, SessionId,
};
use crate::session::store::SessionStore;
use crate::session::store::fs::{new_session_id, sessions_root, sha256_hex};
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
    pub source_range: CanonicalSpan,
    pub evidence_pack_ref: ArtifactRef,
    pub compact_store: SessionStore,
    /// Accumulated LLM conversation for the compact session (system prompt +
    /// user turns + assistant responses). Mirrors what is in `compact_store`.
    pub messages: Vec<async_openai::types::ChatCompletionRequestMessage>,
    /// Latest assistant response — the candidate for `/commit`.
    pub current_content: String,
}

/// Generate an evidence pack artifact containing entry excerpts, artifact
/// refs, and checksum metadata for every entry in the source range.
pub fn generate_canonical_evidence_pack(
    records: &[CanonicalRecord],
    source_range: &CanonicalSpan,
    artifact_store: &crate::session::store::fs::ArtifactStore,
    session_id: &SessionId,
) -> Result<ArtifactRef> {
    let evidence = canonical_evidence(records, source_range);
    let total_byte_len = evidence.len();
    let ev_entries = vec![serde_json::json!({
        "kind": "canonical_evidence",
        "excerpt": evidence,
        "excerpt_sha256": sha256_hex(evidence.as_bytes()),
    })];

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

/// Create an independent compact session with its own directory and log. The
/// session lives under `sessions/.compact/` so it never appears in the
/// per-project session picker (which only lists `project-*` directories).
pub fn create_compact_store(project_root: &std::path::Path) -> Result<SessionStore> {
    let root = sessions_root()?;
    let compact_id = new_session_id(project_root, now_secs() as u64);
    let compact_root = root.join(".compact").join(compact_id.as_str());
    SessionStore::open(compact_id, compact_root)
}

/// Build the initial system prompt for the compact session.
pub fn build_compact_system_msg(
    records: &[CanonicalRecord],
    source_range: &CanonicalSpan,
) -> async_openai::types::ChatCompletionRequestMessage {
    let evidence = canonical_evidence(records, source_range);

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
        source_range: EntrySpan {
            start_seq: state.source_range.start_seq,
            end_seq_exclusive: state.source_range.end_seq_exclusive,
        },
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
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
