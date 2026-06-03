//! `SessionRuntime` — the single facade the Agent uses for the session system.
//!
//! It composes the store, artifact store, and micro compact policy, and exposes
//! record / replay / render / automatic-micro-compact. It never generates task
//! facts, never runs an LLM summary, and never judges task completion.

use crate::error::Result;
use crate::session::compact::{
    CompactOverlay, MicroCompactPolicy, MicroCompactTriggerReason, estimate_micro_compact,
    run_micro_compact,
};
use crate::session::context::{RenderedEntry, render_context_entries};
use crate::session::model::{
    ArtifactKind, ArtifactRef, EntryId, EntryMeta, MicroCompactResult, SessionEntry,
    SessionEntryKind, SessionId, ToolCallEntry, ToolResultEntry, ToolStatus,
};
use crate::session::store::fs::{record_inline_limit, ArtifactStore};
use crate::session::store::SessionStore;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SessionRuntime {
    store: SessionStore,
    policy: MicroCompactPolicy,
}

impl SessionRuntime {
    pub fn open(session_id: SessionId, session_root: PathBuf) -> Result<Self> {
        let store = SessionStore::open(session_id, session_root)?;
        Ok(SessionRuntime {
            store,
            policy: MicroCompactPolicy::default(),
        })
    }

    pub fn session_id(&self) -> &SessionId {
        self.store.session_id()
    }

    /// Override the default policy (primarily for testing).
    pub fn set_policy(&mut self, policy: MicroCompactPolicy) {
        self.policy = policy;
    }

    pub fn policy(&self) -> &MicroCompactPolicy {
        &self.policy
    }

    /// Expose the next entry meta without bumping seq (for artifact writes
    /// that precede entry commits, e.g. evidence packs).
    pub fn store_next_meta(&self) -> EntryMeta {
        self.store.next_meta()
    }

    /// Write an artifact without committing a log entry. The returned
    /// `ArtifactRef` can later be embedded in a committed entry.
    pub fn write_artifact(
        &mut self,
        meta: &EntryMeta,
        kind: ArtifactKind,
        content: &str,
    ) -> Result<ArtifactRef> {
        self.store.artifacts().write(meta, kind, content)
    }

    /// Commit a fully-built entry (used by interactive compact).
    pub fn commit_entry(&mut self, entry: SessionEntry) -> Result<EntryId> {
        self.store.commit(entry)
    }

    pub fn artifact_store(&self) -> &ArtifactStore {
        self.store.artifacts()
    }

    pub fn store_mut(&mut self) -> &mut SessionStore {
        &mut self.store
    }

    // ------------------------------------------------------------------
    // Recording
    // ------------------------------------------------------------------

    pub fn record_user_message(&mut self, content: &str) -> Result<EntryId> {
        self.store.increment_turns_since_last_micro_compact()?;
        let meta = self.store.next_meta();
        self.store.commit(SessionEntry {
            meta,
            kind: SessionEntryKind::UserMessage(crate::session::model::UserMessageEntry {
                content: content.to_string(),
            }),
        })
    }

    pub fn record_assistant_message(&mut self, content: &str) -> Result<EntryId> {
        let meta = self.store.next_meta();
        self.store.commit(SessionEntry {
            meta,
            kind: SessionEntryKind::AssistantMessage(
                crate::session::model::AssistantMessageEntry {
                    content: content.to_string(),
                },
            ),
        })
    }

    /// Record a tool call. Oversized args are archived as `ToolArgs`.
    pub fn record_tool_call(
        &mut self,
        tool_name: &str,
        provider_tool_call_id: Option<String>,
        tool_call_group_id: Option<String>,
        args: serde_json::Value,
        parent_assistant_entry_id: Option<EntryId>,
    ) -> Result<EntryId> {
        let meta = self.store.next_meta();
        let serialized = serde_json::to_string(&args).unwrap_or_default();
        let (inline_args, arg_artifacts) =
            if serialized.len() > record_inline_limit(ArtifactKind::ToolArgs) {
                let art =
                    self.store
                        .artifacts()
                        .write(&meta, ArtifactKind::ToolArgs, &serialized)?;
                (None, vec![art])
            } else {
                (Some(args), Vec::new())
            };

        self.store.commit(SessionEntry {
            meta,
            kind: SessionEntryKind::ToolCall(ToolCallEntry {
                tool_name: tool_name.to_string(),
                provider_tool_call_id,
                tool_call_group_id,
                inline_args,
                arg_artifacts,
                parent_assistant_entry_id,
            }),
        })
    }

    /// Record a tool result. Oversized content is archived under `kind`.
    pub fn record_tool_result(
        &mut self,
        call_entry_id: EntryId,
        tool_call_group_id: Option<String>,
        status: ToolStatus,
        kind: ArtifactKind,
        content: &str,
    ) -> Result<EntryId> {
        let meta = self.store.next_meta();
        let (inline_content, artifacts, preview) = if content.len() > record_inline_limit(kind) {
            let art = self.store.artifacts().write(&meta, kind, content)?;
            let preview = art.preview.clone();
            (None, vec![art], Some(preview))
        } else {
            (Some(content.to_string()), Vec::new(), None)
        };

        self.store.commit(SessionEntry {
            meta,
            kind: SessionEntryKind::ToolResult(ToolResultEntry {
                call_entry_id,
                tool_call_group_id,
                status,
                inline_content,
                artifacts,
                preview,
            }),
        })
    }

    // ------------------------------------------------------------------
    // Automatic micro compact
    // ------------------------------------------------------------------

    /// Update `last_model_request_at` after every LLM request completes.
    pub fn update_last_model_request_at(&mut self) -> Result<()> {
        self.store.update_last_model_request_at(now_secs())
    }

    /// Per-user-turn trigger. Called once after `record_user_message`, before
    /// the first LLM request of the turn. Evaluates ColdResume → TurnInterval →
    /// BloatPressure in order; the first that yields eligible candidates wins.
    /// Errors from non-ContextPressure triggers are logged, not propagated.
    pub fn maybe_micro_compact_for_turn(&mut self) -> Result<Option<MicroCompactResult>> {
        if !self.policy.auto_enabled {
            return Ok(None);
        }

        let entries = self.store.replay_entries()?;
        let overlay = CompactOverlay::from_entries(&entries);

        // ColdResume — cheapest when the prefix cache is already stale.
        match self.try_cold_resume(&entries, &overlay) {
            Ok(Some(result)) => return Ok(Some(result)),
            Ok(None) => {}
            Err(e) => tracing::error!(%e, "ColdResume micro compact failed"),
        }

        // TurnInterval — bounded worst-case accumulator.
        match self.try_turn_interval(&entries, &overlay) {
            Ok(Some(result)) => return Ok(Some(result)),
            Ok(None) => {}
            Err(e) => tracing::error!(%e, "TurnInterval micro compact failed"),
        }

        // BloatPressure — routine cost optimisation.
        match self.try_bloat_pressure(&entries, &overlay) {
            Ok(Some(result)) => return Ok(Some(result)),
            Ok(None) => {}
            Err(e) => tracing::error!(%e, "BloatPressure micro compact failed"),
        }

        Ok(None)
    }

    /// Per-request trigger for context-pressure. The caller supplies the
    /// already-computed token estimates so the runtime doesn't need to guess.
    ///
    /// ContextPressure can only compact eligible old runtime bloat.
    /// It cannot rewrite user messages, recent turns, or assistant final
    /// conclusions. If the context is still too large after micro compact,
    /// the caller must handle the overflow itself.
    pub fn maybe_micro_compact_for_request(
        &mut self,
        ctx_used: u64,
        ctx_total: u64,
    ) -> Result<Option<MicroCompactResult>> {
        if !self.policy.auto_enabled {
            return Ok(None);
        }
        let ratio = ctx_used as f64 / ctx_total as f64;
        if ratio < self.policy.context_pressure_ratio {
            return Ok(None);
        }

        let entries = self.store.replay_entries()?;
        let overlay = CompactOverlay::from_entries(&entries);
        let estimate = estimate_micro_compact(&entries, &overlay, &self.policy);
        if estimate.compactable_bytes == 0 {
            return Ok(None);
        }

        let result = run_micro_compact(&mut self.store, &self.policy)?;
        if result.compacted_entry_ids.is_empty() {
            return Ok(None);
        }

        tracing::info!(
            reason = ?MicroCompactTriggerReason::ContextPressure,
            compacted = result.compacted_entry_ids.len(),
            saved_bytes = result.saved_bytes_estimate,
            "auto micro compact"
        );
        self.store
            .update_after_micro_compact(result.compact_entry_seq)?;
        Ok(Some(result))
    }

    // ------------------------------------------------------------------
    // Replay / render
    // ------------------------------------------------------------------

    pub fn replay_entries(&self) -> Result<Vec<SessionEntry>> {
        self.store.replay_entries()
    }

    /// Replay the log, apply the compact overlay, and render context items.
    pub fn rendered_context_entries(&self) -> Result<Vec<RenderedEntry>> {
        let entries = self.store.replay_entries()?;
        let overlay = CompactOverlay::from_entries(&entries);
        render_context_entries(&entries, &overlay, self.store.artifacts())
    }

    // ------------------------------------------------------------------
    // Trigger helpers
    // ------------------------------------------------------------------

    fn try_cold_resume(
        &mut self,
        entries: &[SessionEntry],
        overlay: &CompactOverlay,
    ) -> Result<Option<MicroCompactResult>> {
        let state = self.store.state();
        let now = now_secs();
        let threshold = self.policy.cold_resume_after_secs as i64;

        let is_cold = match state.last_model_request_at {
            None => true,
            Some(ts) if now.saturating_sub(ts) >= threshold => true,
            _ => false,
        };
        if !is_cold {
            return Ok(None);
        }

        let estimate = estimate_micro_compact(entries, overlay, &self.policy);
        if estimate.compactable_bytes == 0 {
            return Ok(None);
        }

        let result = run_micro_compact(&mut self.store, &self.policy)?;
        if result.compacted_entry_ids.is_empty() {
            return Ok(None);
        }

        tracing::info!(
            reason = ?MicroCompactTriggerReason::ColdResume,
            compacted = result.compacted_entry_ids.len(),
            saved_bytes = result.saved_bytes_estimate,
            "auto micro compact"
        );
        self.store
            .update_after_micro_compact(result.compact_entry_seq)?;
        Ok(Some(result))
    }

    fn try_turn_interval(
        &mut self,
        entries: &[SessionEntry],
        overlay: &CompactOverlay,
    ) -> Result<Option<MicroCompactResult>> {
        let state = self.store.state();
        if state.turns_since_last_micro_compact < self.policy.turn_interval {
            return Ok(None);
        }

        let estimate = estimate_micro_compact(entries, overlay, &self.policy);
        if estimate.estimated_saved_bytes == 0 {
            return Ok(None);
        }

        let result = run_micro_compact(&mut self.store, &self.policy)?;
        if result.compacted_entry_ids.is_empty() {
            return Ok(None);
        }

        tracing::info!(
            reason = ?MicroCompactTriggerReason::TurnInterval,
            compacted = result.compacted_entry_ids.len(),
            saved_bytes = result.saved_bytes_estimate,
            "auto micro compact"
        );
        self.store
            .update_after_micro_compact(result.compact_entry_seq)?;
        Ok(Some(result))
    }

    fn try_bloat_pressure(
        &mut self,
        entries: &[SessionEntry],
        overlay: &CompactOverlay,
    ) -> Result<Option<MicroCompactResult>> {
        let state = self.store.state();
        let entries_since = match state.last_micro_compact_seq {
            None => entries.len(),
            Some(seq) => entries.len().saturating_sub(seq as usize + 1),
        };
        if entries_since < self.policy.min_entries_since_last_compact {
            return Ok(None);
        }

        let estimate = estimate_micro_compact(entries, overlay, &self.policy);
        if estimate.compactable_bytes < self.policy.min_compactable_bytes
            || estimate.estimated_saved_bytes < self.policy.min_saved_bytes
            || estimate.estimated_compression_ratio < self.policy.min_compression_ratio
        {
            return Ok(None);
        }

        let result = run_micro_compact(&mut self.store, &self.policy)?;
        if result.compacted_entry_ids.is_empty() {
            return Ok(None);
        }

        tracing::info!(
            reason = ?MicroCompactTriggerReason::BloatPressure,
            compacted = result.compacted_entry_ids.len(),
            saved_bytes = result.saved_bytes_estimate,
            "auto micro compact"
        );
        self.store
            .update_after_micro_compact(result.compact_entry_seq)?;
        Ok(Some(result))
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
