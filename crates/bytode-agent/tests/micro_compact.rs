//! Micro compact safety invariants:
//! - never touches user messages
//! - never rewrites log history (only appends a MicroCompact entry)
//! - the overlay is rebuildable from the replayed log
//! - compacted entries render as previews
//! - repeated compaction of one entry resolves to the largest-seq view

use bytode_agent::session::compact::run_micro_compact;
use bytode_agent::session::{
    ArtifactId, ArtifactKind, ArtifactRef, AssistantMessageEntry, CompactOverlay, EntryId,
    EntryMeta, EntrySpan, InteractiveCompactEntry, InteractiveCompactOutcome,
    InteractiveCompactResult, MicroCompactEntry, MicroCompactPolicy, RenderedEntry, SessionEntry,
    SessionEntryKind, SessionId, SessionRuntime, SessionStore, ToolCallEntry, ToolResultEntry,
    ToolStatus, UserMessageEntry, render_context_entries,
};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("bytode-test-{tag}-{}-{nanos}", std::process::id()))
}

fn commit(store: &mut SessionStore, kind: SessionEntryKind) -> EntryId {
    let meta = store.next_meta();
    store.commit(SessionEntry { meta, kind }).unwrap()
}

/// Aggressive policy so a single old tool result becomes eligible.
fn tight_policy() -> MicroCompactPolicy {
    MicroCompactPolicy {
        keep_recent_entries: 1,
        keep_recent_turns: 1,
        max_inline_tool_output_chars: 10,
        ..MicroCompactPolicy::default()
    }
}

/// Build: user, tool call, big inline tool result, user. Returns the store and
/// the ids of (user1, user2, tool result).
fn build_store(tag: &str) -> (SessionStore, EntryId, EntryId, EntryId) {
    let mut store = SessionStore::open(SessionId("test".into()), temp_root(tag)).unwrap();
    let u1 = commit(
        &mut store,
        SessionEntryKind::UserMessage(UserMessageEntry {
            content: "u1".into(),
        }),
    );
    let call = commit(
        &mut store,
        SessionEntryKind::ToolCall(ToolCallEntry {
            tool_name: "web_search".into(),
            inline_args: Some(serde_json::json!({"q": "x"})),
            arg_artifacts: vec![],
            parent_assistant_entry_id: None,
        }),
    );
    let result = commit(
        &mut store,
        SessionEntryKind::ToolResult(ToolResultEntry {
            call_entry_id: call,
            status: ToolStatus::Ok,
            inline_content: Some("this inline content is well over ten chars".into()),
            artifacts: vec![],
            preview: None,
        }),
    );
    let u2 = commit(
        &mut store,
        SessionEntryKind::UserMessage(UserMessageEntry {
            content: "u2".into(),
        }),
    );
    (store, u1, u2, result)
}

#[test]
fn micro_compact_never_touches_user_messages() {
    let (mut store, u1, u2, result_id) = build_store("no-user");
    let result = run_micro_compact(&mut store, &tight_policy()).unwrap();

    assert_eq!(result.compacted_entry_ids, vec![result_id]);
    assert!(!result.compacted_entry_ids.contains(&u1));
    assert!(!result.compacted_entry_ids.contains(&u2));

    // User content survives verbatim in the log.
    let entries = store.replay_entries().unwrap();
    let users: Vec<&str> = entries
        .iter()
        .filter_map(|e| match &e.kind {
            SessionEntryKind::UserMessage(u) => Some(u.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, vec!["u1", "u2"]);
}

#[test]
fn micro_compact_appends_and_never_rewrites_history() {
    let (mut store, _, _, result_id) = build_store("no-rewrite");
    let before = store.replay_entries().unwrap();
    run_micro_compact(&mut store, &tight_policy()).unwrap();
    let after = store.replay_entries().unwrap();

    // Exactly one new entry (the MicroCompact audit record) is appended.
    assert_eq!(after.len(), before.len() + 1);
    assert!(matches!(
        after.last().unwrap().kind,
        SessionEntryKind::MicroCompact(_)
    ));
    // The compacted tool result entry is unchanged in the log — inline content
    // is still present; the log is the audit source of truth.
    let original = after
        .iter()
        .find(|e| e.meta.id == result_id)
        .expect("result entry present");
    let SessionEntryKind::ToolResult(r) = &original.kind else {
        panic!("expected tool result");
    };
    assert!(r.inline_content.is_some());
}

#[test]
fn overlay_is_rebuildable_from_replayed_log() {
    let (mut store, _, _, result_id) = build_store("overlay-rebuild");
    run_micro_compact(&mut store, &tight_policy()).unwrap();

    let entries = store.replay_entries().unwrap();
    let overlay = CompactOverlay::from_entries(&entries);
    assert!(overlay.view_for(&result_id).is_some());
}

#[test]
fn compacted_entry_renders_as_preview() {
    let (mut store, _, _, _) = build_store("preview");
    run_micro_compact(&mut store, &tight_policy()).unwrap();

    let entries = store.replay_entries().unwrap();
    let overlay = CompactOverlay::from_entries(&entries);
    let rendered = render_context_entries(&entries, &overlay, store.artifacts()).unwrap();

    let has_preview = rendered.iter().any(|r| {
        matches!(r, RenderedEntry::ToolResult { content, .. }
            if content.starts_with("[tool output compacted]"))
    });
    assert!(has_preview);
}

#[test]
fn repeated_compact_uses_largest_seq() {
    let target = EntryId::from_seq(2);

    let art = |preview: &str| ArtifactRef {
        id: ArtifactId("sha".into()),
        kind: ArtifactKind::ToolOutput,
        relative_path: PathBuf::from("artifacts/tool/sha.txt"),
        source_entry_id: target.clone(),
        byte_len: 100,
        sha256: "sha".into(),
        preview: preview.into(),
    };
    let mc = |seq: u64, preview: &str| SessionEntry {
        meta: EntryMeta {
            id: EntryId::from_seq(seq),
            seq,
            created_at: 0,
        },
        kind: SessionEntryKind::MicroCompact(MicroCompactEntry {
            compacted_entry_ids: vec![target.clone()],
            archived_artifacts: vec![art(preview)],
            policy_snapshot: MicroCompactPolicy::default(),
            operation_digest: String::new(),
        }),
    };

    let entries = vec![mc(10, "OLD_PREVIEW"), mc(20, "NEW_PREVIEW")];
    let overlay = CompactOverlay::from_entries(&entries);
    let view = overlay.view_for(&target).unwrap();
    assert_eq!(view.replacement_preview, "NEW_PREVIEW");
    assert_eq!(view.compact_entry_id, EntryId::from_seq(20));
}

// ------------------------------------------------------------------
// Automatic trigger tests
// ------------------------------------------------------------------

fn runtime(tag: &str) -> SessionRuntime {
    SessionRuntime::open(SessionId("test".into()), temp_root(tag)).unwrap()
}

fn policy_with_auto(enabled: bool) -> MicroCompactPolicy {
    let mut p = MicroCompactPolicy::default();
    p.auto_enabled = enabled;
    p
}

fn record_bloat(rt: &mut SessionRuntime, content: &str) {
    let call = rt
        .record_tool_call("web_search", serde_json::json!({"q": "x"}), None)
        .unwrap();
    rt.record_tool_result(call, ToolStatus::Ok, ArtifactKind::ToolOutput, content)
        .unwrap();
}

/// Record a user message, then some bloat, then another user message. This
/// pushes the bloat into the "old" region so compact can reach it.
fn bloat_before_user(rt: &mut SessionRuntime) {
    rt.record_user_message("task").unwrap();
    record_bloat(rt, &"x".repeat(5_000));
    // A second user message pushes the bloat behind the recent-user cutoff.
    rt.record_user_message("continue").unwrap();
}

#[test]
fn auto_trigger_skipped_when_disabled() {
    let mut rt = runtime("auto-disabled");
    rt.set_policy(policy_with_auto(false));
    bloat_before_user(&mut rt);

    // Per-turn triggers should all skip.
    assert!(rt.maybe_micro_compact_for_turn().unwrap().is_none());
    // Per-request trigger should also skip.
    assert!(
        rt.maybe_micro_compact_for_request(800_000, 1_000_000)
            .unwrap()
            .is_none()
    );
}

#[test]
fn context_pressure_triggers_when_over_ratio() {
    let mut rt = runtime("ctx-pressure");
    bloat_before_user(&mut rt);

    // Force the compactable entry outside the recent window.
    rt.set_policy(MicroCompactPolicy {
        keep_recent_entries: 1,
        keep_recent_turns: 1,
        max_inline_tool_output_chars: 10,
        ..MicroCompactPolicy::default()
    });

    let result = rt
        .maybe_micro_compact_for_request(800_000, 1_000_000)
        .unwrap();
    assert!(result.is_some());
    let r = result.unwrap();
    assert!(!r.compacted_entry_ids.is_empty());
}

#[test]
fn context_pressure_skipped_when_under_ratio() {
    let mut rt = runtime("ctx-no-pressure");
    bloat_before_user(&mut rt);

    rt.set_policy(MicroCompactPolicy {
        keep_recent_entries: 1,
        max_inline_tool_output_chars: 10,
        ..MicroCompactPolicy::default()
    });

    // ctx_used / ctx_total = 0.5 < 0.75
    assert!(
        rt.maybe_micro_compact_for_request(500_000, 1_000_000)
            .unwrap()
            .is_none()
    );
}

#[test]
fn turn_interval_triggers_after_enough_turns() {
    let policy = MicroCompactPolicy {
        turn_interval: 2,
        keep_recent_entries: 1,
        keep_recent_turns: 1,
        max_inline_tool_output_chars: 10,
        ..MicroCompactPolicy::default()
    };

    let mut rt = runtime("turn-interval");
    rt.set_policy(policy);

    // First turn with bloat.
    rt.record_user_message("t1").unwrap();
    record_bloat(&mut rt, &"a".repeat(5_000));
    // Second turn pushes first bloat into "old" region.
    rt.record_user_message("t2").unwrap();

    // After 2 turns, TurnInterval should fire on the old bloat.
    let result = rt.maybe_micro_compact_for_turn().unwrap();
    assert!(result.is_some());
    assert!(!result.unwrap().compacted_entry_ids.is_empty());
}

#[test]
fn context_pressure_no_bloat_returns_none() {
    let mut rt = runtime("ctx-no-bloat");
    rt.record_user_message("hello").unwrap();

    // No tool results at all — nothing to compact.
    assert!(
        rt.maybe_micro_compact_for_request(800_000, 1_000_000)
            .unwrap()
            .is_none()
    );
}

// ------------------------------------------------------------------
// Interactive compact tests
// ------------------------------------------------------------------

fn compact_artifact(store: &SessionStore, content: &str) -> ArtifactRef {
    let meta = store.next_meta();
    store
        .artifacts()
        .write(&meta, ArtifactKind::CompactArchive, content)
        .unwrap()
}

#[test]
fn interactive_compact_entry_is_serializable() {
    let span = EntrySpan {
        start_seq: 5,
        end_seq_exclusive: 10,
    };
    let entry = InteractiveCompactEntry {
        source_range: span.clone(),
        compact_session_id: SessionId("cs".into()),
        outcome: InteractiveCompactOutcome::Committed,
        result: Some(InteractiveCompactResult {
            content: "compressed summary".into(),
            evidence_pack_ref: ArtifactRef {
                id: ArtifactId("abc".into()),
                kind: ArtifactKind::CompactArchive,
                relative_path: PathBuf::from("artifacts/compact/abc.txt"),
                source_entry_id: EntryId::from_seq(0),
                byte_len: 100,
                sha256: "abc".into(),
                preview: "preview".into(),
            },
        }),
        operation_digest: "compacted 5 entries".into(),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let _round: InteractiveCompactEntry = serde_json::from_str(&json).unwrap();
}

#[test]
fn interactive_compact_commit_writes_entry_to_log() {
    let mut store =
        SessionStore::open(SessionId("ic-commit".into()), temp_root("ic-commit")).unwrap();
    let art = compact_artifact(&store, "evidence");

    let meta = store.next_meta();
    let ic = InteractiveCompactEntry {
        source_range: EntrySpan {
            start_seq: 0,
            end_seq_exclusive: 3,
        },
        compact_session_id: SessionId("cs".into()),
        outcome: InteractiveCompactOutcome::Committed,
        result: Some(InteractiveCompactResult {
            content: "compressed".into(),
            evidence_pack_ref: art,
        }),
        operation_digest: "done".into(),
    };
    store
        .commit(SessionEntry {
            meta,
            kind: SessionEntryKind::InteractiveCompact(ic),
        })
        .unwrap();

    let entries = store.replay_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(matches!(
        entries[0].kind,
        SessionEntryKind::InteractiveCompact(_)
    ));
}

#[test]
fn interactive_compact_context_replaces_source_range() {
    let mut store = SessionStore::open(SessionId("ic-ctx".into()), temp_root("ic-ctx")).unwrap();

    commit(
        &mut store,
        SessionEntryKind::UserMessage(UserMessageEntry {
            content: "old message".into(),
        }),
    );
    commit(
        &mut store,
        SessionEntryKind::AssistantMessage(bytode_agent::session::AssistantMessageEntry {
            content: "old response".into(),
        }),
    );
    commit(
        &mut store,
        SessionEntryKind::UserMessage(UserMessageEntry {
            content: "recent message".into(),
        }),
    );

    let art = compact_artifact(&store, "evidence");
    let ic = InteractiveCompactEntry {
        source_range: EntrySpan {
            start_seq: 0,
            end_seq_exclusive: 2,
        },
        compact_session_id: SessionId("cs".into()),
        outcome: InteractiveCompactOutcome::Committed,
        result: Some(InteractiveCompactResult {
            content: "REPLACED".into(),
            evidence_pack_ref: art,
        }),
        operation_digest: "done".into(),
    };
    let meta = store.next_meta();
    store
        .commit(SessionEntry {
            meta,
            kind: SessionEntryKind::InteractiveCompact(ic),
        })
        .unwrap();

    let entries = store.replay_entries().unwrap();
    let overlay = CompactOverlay::from_entries(&entries);
    let rendered = render_context_entries(&entries, &overlay, store.artifacts()).unwrap();

    assert!(matches!(&rendered[0], RenderedEntry::Assistant(s) if s == "REPLACED"));
    assert!(matches!(&rendered[1], RenderedEntry::User(s) if s == "recent message"));
}

#[test]
fn interactive_compact_tail_preserves_micro_compact() {
    let mut store = SessionStore::open(SessionId("ic-tail".into()), temp_root("ic-tail")).unwrap();

    commit(
        &mut store,
        SessionEntryKind::UserMessage(UserMessageEntry {
            content: "user".into(),
        }),
    );
    commit(
        &mut store,
        SessionEntryKind::ToolCall(ToolCallEntry {
            tool_name: "web_search".into(),
            inline_args: Some(serde_json::json!({"q": "x"})),
            arg_artifacts: vec![],
            parent_assistant_entry_id: None,
        }),
    );
    commit(
        &mut store,
        SessionEntryKind::ToolResult(ToolResultEntry {
            call_entry_id: EntryId::from_seq(1),
            status: ToolStatus::Ok,
            inline_content: Some(
                "this is a big tool result that is well over the compact threshold".into(),
            ),
            artifacts: vec![],
            preview: None,
        }),
    );

    // Micro compact on the tool result
    // keep_recent_entries=0 so the tool result (last entry) is eligible
    run_micro_compact(
        &mut store,
        &MicroCompactPolicy {
            keep_recent_entries: 0,
            keep_recent_turns: 0,
            max_inline_tool_output_chars: 10,
            ..MicroCompactPolicy::default()
        },
    )
    .unwrap();

    // Interactive compact replaces only seq 0 (the user message)
    let art = compact_artifact(&store, "evidence");
    let ic = InteractiveCompactEntry {
        source_range: EntrySpan {
            start_seq: 0,
            end_seq_exclusive: 1,
        },
        compact_session_id: SessionId("cs".into()),
        outcome: InteractiveCompactOutcome::Committed,
        result: Some(InteractiveCompactResult {
            content: "REPLACED_USER".into(),
            evidence_pack_ref: art,
        }),
        operation_digest: "done".into(),
    };
    let meta = store.next_meta();
    store
        .commit(SessionEntry {
            meta,
            kind: SessionEntryKind::InteractiveCompact(ic),
        })
        .unwrap();

    let entries = store.replay_entries().unwrap();
    let overlay = CompactOverlay::from_entries(&entries);
    let rendered = render_context_entries(&entries, &overlay, store.artifacts()).unwrap();

    assert!(matches!(&rendered[0], RenderedEntry::Assistant(s) if s == "REPLACED_USER"));
    assert!(matches!(&rendered[1], RenderedEntry::ToolCall { name, .. } if name == "web_search"));
    assert!(
        matches!(&rendered[2], RenderedEntry::ToolResult { content, .. }
        if content.starts_with("[tool output compacted]"))
    );
}

#[test]
fn interactive_compact_overlap_returns_error() {
    let mut store =
        SessionStore::open(SessionId("ic-overlap".into()), temp_root("ic-overlap")).unwrap();

    commit(
        &mut store,
        SessionEntryKind::UserMessage(UserMessageEntry {
            content: "msg".into(),
        }),
    );

    let art = compact_artifact(&store, "evidence");

    for (start, end) in [(0, 2), (1, 3)] {
        let ic = InteractiveCompactEntry {
            source_range: EntrySpan {
                start_seq: start,
                end_seq_exclusive: end,
            },
            compact_session_id: SessionId("cs".into()),
            outcome: InteractiveCompactOutcome::Committed,
            result: Some(InteractiveCompactResult {
                content: "ignored".into(),
                evidence_pack_ref: art.clone(),
            }),
            operation_digest: "done".into(),
        };
        let meta = store.next_meta();
        store
            .commit(SessionEntry {
                meta,
                kind: SessionEntryKind::InteractiveCompact(ic),
            })
            .unwrap();
    }

    let entries = store.replay_entries().unwrap();
    let overlay = CompactOverlay::from_entries(&entries);
    // Overlapping committed ranges must return an error.
    let result = render_context_entries(&entries, &overlay, store.artifacts());
    assert!(result.is_err());
}

#[test]
fn interactive_compact_abort_does_not_affect_context() {
    let mut store =
        SessionStore::open(SessionId("ic-abort".into()), temp_root("ic-abort")).unwrap();

    commit(
        &mut store,
        SessionEntryKind::UserMessage(UserMessageEntry {
            content: "msg".into(),
        }),
    );

    let ic = InteractiveCompactEntry {
        source_range: EntrySpan {
            start_seq: 0,
            end_seq_exclusive: 1,
        },
        compact_session_id: SessionId("cs".into()),
        outcome: InteractiveCompactOutcome::Aborted,
        result: None,
        operation_digest: "aborted".into(),
    };
    let meta = store.next_meta();
    store
        .commit(SessionEntry {
            meta,
            kind: SessionEntryKind::InteractiveCompact(ic),
        })
        .unwrap();

    let entries = store.replay_entries().unwrap();
    let overlay = CompactOverlay::from_entries(&entries);
    let rendered = render_context_entries(&entries, &overlay, store.artifacts()).unwrap();

    assert!(matches!(&rendered[0], RenderedEntry::User(s) if s == "msg"));
}

#[test]
fn interactive_compact_evidence_pack_is_artifactual() {
    let mut store =
        SessionStore::open(SessionId("ic-evidence".into()), temp_root("ic-evidence")).unwrap();

    let evidence_content = r#"{"source_range":{"start_seq":0,"end_seq_exclusive":5}}"#;
    let art = compact_artifact(&store, evidence_content);

    assert!(store.artifacts().exists(&art));
    let read_back = store.artifacts().read_verified(&art).unwrap();
    assert_eq!(read_back, evidence_content);
}
