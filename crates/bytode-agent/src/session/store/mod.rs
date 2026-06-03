//! Session persistence: state, append-only log, and artifact files. No compact
//! policy and no context rendering live here.

pub mod fs;
pub mod log;

use crate::error::Result;
use crate::session::model::{EntryId, EntryMeta, SessionEntry, SessionId};
use fs::ArtifactStore;
use log::{SessionLogReader, SessionLogWriter};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimal durable state. The log is authoritative; this only caches what is
/// needed to resume appending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: SessionId,
    pub next_seq: u64,
    pub session_root: PathBuf,
    pub log_path: PathBuf,
    pub artifact_dir: PathBuf,

    pub last_model_request_at: Option<i64>,
    pub last_micro_compact_seq: Option<u64>,
    pub turns_since_last_micro_compact: usize,
}

/// Combines durable state, the log writer/reader, and the artifact store.
pub struct SessionStore {
    state: SessionState,
    writer: SessionLogWriter,
    reader: SessionLogReader,
    artifacts: ArtifactStore,
}

impl SessionStore {
    /// Open (creating if needed) the session at `session_root`. `next_seq` is
    /// recovered by replaying the log so resumed sessions keep monotonic seqs.
    /// Runtime fields (`last_model_request_at`, etc.) are restored from any
    /// existing `state.json` so they survive restarts.
    pub fn open(session_id: SessionId, session_root: PathBuf) -> Result<Self> {
        fs::ensure_session_tree(&session_root)?;
        let log_path = session_root.join("events").join("log.jsonl");
        let artifact_dir = session_root.join("artifacts");

        let reader = SessionLogReader::new(log_path.clone());
        let next_seq = reader
            .read_all()?
            .iter()
            .map(|e| e.meta.seq)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);

        // Restore runtime fields from a prior run so they survive restarts.
        let prior = read_existing_state(&session_root);

        let state = SessionState {
            session_id,
            next_seq,
            session_root: session_root.clone(),
            log_path: log_path.clone(),
            artifact_dir,
            last_model_request_at: prior.as_ref().and_then(|s| s.last_model_request_at),
            last_micro_compact_seq: prior.as_ref().and_then(|s| s.last_micro_compact_seq),
            turns_since_last_micro_compact: prior
                .as_ref()
                .map(|s| s.turns_since_last_micro_compact)
                .unwrap_or(0),
        };
        let writer = SessionLogWriter::open(&log_path)?;
        let artifacts = ArtifactStore::new(session_root);

        let store = SessionStore {
            state,
            writer,
            reader,
            artifacts,
        };
        store.persist_state()?;
        Ok(store)
    }

    pub fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    pub fn session_id(&self) -> &SessionId {
        &self.state.session_id
    }

    /// Reserve the next entry meta without committing. Artifacts are written
    /// using this id before the entry is committed (crash-safe ordering).
    pub fn next_meta(&self) -> EntryMeta {
        let seq = self.state.next_seq;
        EntryMeta {
            id: EntryId::from_seq(seq),
            seq,
            created_at: now_secs(),
        }
    }

    /// Append a fully-built entry. Its seq must match the reserved seq.
    pub fn commit(&mut self, entry: SessionEntry) -> Result<EntryId> {
        debug_assert_eq!(entry.meta.seq, self.state.next_seq);
        let id = entry.meta.id.clone();
        self.writer.append(&entry)?;
        self.state.next_seq += 1;
        self.persist_state()?;
        Ok(id)
    }

    pub fn replay_entries(&self) -> Result<Vec<SessionEntry>> {
        self.reader.read_all()
    }

    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// Update `last_model_request_at` without appending a log entry.
    pub fn update_last_model_request_at(&mut self, ts: i64) -> Result<()> {
        self.state.last_model_request_at = Some(ts);
        self.persist_state()
    }

    /// Called after a successful MicroCompact pass.
    pub fn update_after_micro_compact(&mut self, seq: u64) -> Result<()> {
        self.state.last_micro_compact_seq = Some(seq);
        self.state.turns_since_last_micro_compact = 0;
        self.persist_state()
    }

    /// Increment the per-user-turn counter. Call once per `record_user_message`.
    pub fn increment_turns_since_last_micro_compact(&mut self) -> Result<()> {
        self.state.turns_since_last_micro_compact += 1;
        self.persist_state()
    }

    fn persist_state(&self) -> Result<()> {
        let path = self.state.session_root.join("state.json");
        let json = serde_json::to_string_pretty(&self.state)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn read_existing_state(session_root: &std::path::Path) -> Option<SessionState> {
    let path = session_root.join("state.json");
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}
