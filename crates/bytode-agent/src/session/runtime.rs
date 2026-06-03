//! `SessionRuntime` — the single facade the Agent uses for the session system.
//!
//! It composes the store, artifact store, and micro compact policy, and exposes
//! record / replay / render / micro-compact. It never generates task facts,
//! never runs an LLM summary, and never judges task completion.

use crate::error::Result;
use crate::session::compact::{CompactOverlay, MicroCompactPolicy, run_micro_compact};
use crate::session::context::{RenderedEntry, render_context_entries};
use crate::session::model::{
    ArtifactKind, EntryId, MicroCompactResult, SessionEntry, SessionEntryKind, SessionId,
    ToolCallEntry, ToolResultEntry, ToolStatus,
};
use crate::session::store::SessionStore;
use crate::session::store::fs::record_inline_limit;
use std::path::PathBuf;

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

    pub fn record_user_message(&mut self, content: &str) -> Result<EntryId> {
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
                status,
                inline_content,
                artifacts,
                preview,
            }),
        })
    }

    pub fn micro_compact(&mut self) -> Result<MicroCompactResult> {
        run_micro_compact(&mut self.store, &self.policy)
    }

    pub fn replay_entries(&self) -> Result<Vec<SessionEntry>> {
        self.store.replay_entries()
    }

    /// Replay the log, apply the compact overlay, and render context items.
    pub fn rendered_context_entries(&self) -> Result<Vec<RenderedEntry>> {
        let entries = self.store.replay_entries()?;
        let overlay = CompactOverlay::from_entries(&entries);
        Ok(render_context_entries(
            &entries,
            &overlay,
            self.store.artifacts(),
        ))
    }
}
