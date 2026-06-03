//! Session log + artifact store invariants: append/replay, archiving of large
//! payloads, Phase 1 context equivalence, and non-panicking artifact errors.

use bytode_agent::session::store::fs::{ArtifactStore, sha256_hex};
use bytode_agent::session::{
    ArtifactKind, EntryId, EntryMeta, RenderedEntry, SessionEntryKind, SessionId, SessionRuntime,
    ToolStatus,
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

fn open(tag: &str) -> (SessionRuntime, PathBuf) {
    let root = temp_root(tag);
    let rt = SessionRuntime::open(SessionId("test".into()), root.clone()).unwrap();
    (rt, root)
}

#[test]
fn append_and_replay_round_trips_entries() {
    let (mut rt, _root) = open("append-replay");
    rt.record_user_message("hello").unwrap();
    let call = rt
        .record_tool_call("read_file", serde_json::json!({"path": "a.rs"}), None)
        .unwrap();
    rt.record_tool_result(
        call,
        ToolStatus::Ok,
        ArtifactKind::FileSnapshot,
        "fn main() {}",
    )
    .unwrap();
    rt.record_assistant_message("done").unwrap();

    let entries = rt.replay_entries().unwrap();
    assert_eq!(entries.len(), 4);
    assert!(matches!(entries[0].kind, SessionEntryKind::UserMessage(_)));
    assert!(matches!(entries[1].kind, SessionEntryKind::ToolCall(_)));
    assert!(matches!(entries[2].kind, SessionEntryKind::ToolResult(_)));
    assert!(matches!(
        entries[3].kind,
        SessionEntryKind::AssistantMessage(_)
    ));
    // seqs are monotonic and contiguous.
    for (i, e) in entries.iter().enumerate() {
        assert_eq!(e.meta.seq, i as u64);
    }
}

#[test]
fn artifact_store_write_read_sha256() {
    let root = temp_root("artifact-io");
    let store = ArtifactStore::new(root);
    let meta = EntryMeta {
        id: EntryId::from_seq(1),
        seq: 1,
        created_at: 0,
    };
    let content = "the quick brown fox";
    let art = store
        .write(&meta, ArtifactKind::ToolOutput, content)
        .unwrap();

    assert_eq!(art.sha256, sha256_hex(content.as_bytes()));
    assert_eq!(art.byte_len, content.len() as u64);
    assert_eq!(art.source_entry_id, meta.id);
    assert!(!art.relative_path.is_absolute());
    assert_eq!(store.read_verified(&art).unwrap(), content);
}

#[test]
fn large_tool_result_is_archived() {
    let (mut rt, _root) = open("large-result");
    let call = rt
        .record_tool_call("web_search", serde_json::json!({"q": "x"}), None)
        .unwrap();
    let big = "x".repeat(20_000); // > 16 KiB tool output inline limit
    rt.record_tool_result(call, ToolStatus::Ok, ArtifactKind::ToolOutput, &big)
        .unwrap();

    let entries = rt.replay_entries().unwrap();
    let SessionEntryKind::ToolResult(result) = &entries[1].kind else {
        panic!("expected tool result");
    };
    assert!(result.inline_content.is_none());
    assert_eq!(result.artifacts.len(), 1);
    assert!(result.preview.is_some());
}

#[test]
fn large_tool_call_args_are_archived() {
    let (mut rt, _root) = open("large-args");
    let big_args = serde_json::json!({ "data": "y".repeat(20_000) });
    rt.record_tool_call("write_file", big_args, None).unwrap();

    let entries = rt.replay_entries().unwrap();
    let SessionEntryKind::ToolCall(call) = &entries[0].kind else {
        panic!("expected tool call");
    };
    assert!(call.inline_args.is_none());
    assert_eq!(call.arg_artifacts.len(), 1);
}

/// Phase 1 acceptance: the context view is sourced from the session log and the
/// recorded turn (user / tool call / tool result / assistant) is replayable in
/// order. Nothing is dropped.
#[test]
fn context_is_built_from_session_log() {
    let (mut rt, _root) = open("ctx-phase1");
    rt.record_user_message("do it").unwrap();
    let call = rt
        .record_tool_call("read_file", serde_json::json!({"path": "a.rs"}), None)
        .unwrap();
    rt.record_tool_result(call, ToolStatus::Ok, ArtifactKind::FileSnapshot, "code")
        .unwrap();
    rt.record_assistant_message("finished").unwrap();

    let rendered = rt.rendered_context_entries().unwrap();
    assert!(matches!(&rendered[0], RenderedEntry::User(s) if s == "do it"));
    assert!(matches!(&rendered[1], RenderedEntry::ToolCall { name, .. } if name == "read_file"));
    assert!(matches!(&rendered[2], RenderedEntry::ToolResult { content, .. } if content == "code"));
    assert!(matches!(&rendered[3], RenderedEntry::Assistant(s) if s == "finished"));
    // tool call id and tool result call_id match so provider pairing holds.
    let (RenderedEntry::ToolCall { id, .. }, RenderedEntry::ToolResult { call_id, .. }) =
        (&rendered[1], &rendered[2])
    else {
        panic!("unexpected shape");
    };
    assert_eq!(id, call_id);
}

#[test]
fn missing_artifact_renders_error_without_panic() {
    let (mut rt, root) = open("missing-artifact");
    let call = rt
        .record_tool_call("web_search", serde_json::json!({"q": "x"}), None)
        .unwrap();
    let big = "x".repeat(20_000);
    rt.record_tool_result(call, ToolStatus::Ok, ArtifactKind::ToolOutput, &big)
        .unwrap();

    // Delete the backing artifact file.
    let entries = rt.replay_entries().unwrap();
    let SessionEntryKind::ToolResult(result) = &entries[1].kind else {
        panic!("expected tool result");
    };
    let path = root.join(&result.artifacts[0].relative_path);
    std::fs::remove_file(&path).unwrap();

    let rendered = rt.rendered_context_entries().unwrap();
    let RenderedEntry::ToolResult { content, .. } = &rendered[1] else {
        panic!("expected tool result");
    };
    assert!(content.starts_with("[artifact missing]"));
}

#[test]
fn checksum_mismatch_renders_error_without_panic() {
    let (mut rt, root) = open("checksum-mismatch");
    let call = rt
        .record_tool_call("web_search", serde_json::json!({"q": "x"}), None)
        .unwrap();
    let big = "x".repeat(20_000);
    rt.record_tool_result(call, ToolStatus::Ok, ArtifactKind::ToolOutput, &big)
        .unwrap();

    let entries = rt.replay_entries().unwrap();
    let SessionEntryKind::ToolResult(result) = &entries[1].kind else {
        panic!("expected tool result");
    };
    let path = root.join(&result.artifacts[0].relative_path);
    std::fs::write(&path, "corrupted").unwrap();

    let rendered = rt.rendered_context_entries().unwrap();
    let RenderedEntry::ToolResult { content, .. } = &rendered[1] else {
        panic!("expected tool result");
    };
    assert!(content.starts_with("[artifact checksum mismatch]"));
}
