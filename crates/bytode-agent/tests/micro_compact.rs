//! Micro compact safety invariants:
//! - never touches user messages
//! - never rewrites log history (only appends a MicroCompact entry)
//! - the overlay is rebuildable from the replayed log
//! - compacted entries render as previews
//! - repeated compaction of one entry resolves to the largest-seq view

use bytode_agent::session::compact::run_micro_compact;
use bytode_agent::session::{
    ArtifactId, ArtifactKind, ArtifactRef, CompactOverlay, EntryId, EntryMeta, MicroCompactEntry,
    MicroCompactPolicy, RenderedEntry, SessionEntry, SessionEntryKind, SessionId, SessionRuntime,
    SessionStore, ToolCallEntry, ToolResultEntry, ToolStatus, UserMessageEntry,
    render_context_entries,
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
    let rendered = render_context_entries(&entries, &overlay, store.artifacts());

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
