//! Renders `SessionEntry` + `CompactOverlay` into a flat view the ContextBuilder
//! turns into chat messages.
//!
//! Compacted and record-time-archived tool results are shown as stable preview
//! blocks; missing or corrupt artifacts produce explicit error previews.
//! Overlapping committed interactive-compact ranges return a diagnostic error.
//! This module never panics on data issues.

use crate::error::{BytodeError, Result};
use crate::session::compact::{CompactOverlay, kind_for_tool};
use crate::session::model::{
    ArtifactKind, ArtifactRef, EntryId, EntrySpan, InteractiveCompactOutcome, SessionEntry,
    SessionEntryKind, ToolStatus,
};
use crate::session::store::fs::ArtifactStore;
use std::collections::HashMap;

/// Role-tagged context item. Tool calls carry a synthetic `id` derived from the
/// tool-call entry id; the matching tool result reuses it so provider message
/// pairing stays consistent.
#[derive(Debug, Clone)]
pub enum RenderedEntry {
    User(String),
    Assistant(String),
    ToolCall {
        group_id: Option<String>,
        id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        group_id: Option<String>,
        call_id: String,
        content: String,
    },
}

/// Replay entries (with overlay applied) into context items.
///
/// Committed `InteractiveCompact` entries replace their source range with
/// `result.content` so the old entries are excluded from context.
/// Overlapping committed ranges return an error.
pub fn render_context_entries(
    entries: &[SessionEntry],
    overlay: &CompactOverlay,
    artifacts: &ArtifactStore,
) -> Result<Vec<RenderedEntry>> {
    let replacements = collect_committed_replacements(entries)?;
    let tool_names = tool_name_index(entries);
    let tool_call_ids = tool_call_id_index(entries);
    let mut out = Vec::new();
    let mut skip_until: Option<u64> = None;

    for entry in entries {
        if let Some(end) = skip_until {
            if entry.meta.seq >= end {
                skip_until = None;
            } else {
                continue; // still inside a replaced range
            }
        }

        // Check if this entry starts a new replacement range.
        for (span, content) in &replacements {
            if entry.meta.seq >= span.start_seq && entry.meta.seq < span.end_seq_exclusive {
                out.push(RenderedEntry::Assistant(content.clone()));
                skip_until = Some(span.end_seq_exclusive);
                break;
            }
        }
        if skip_until.is_some() {
            continue;
        }

        match &entry.kind {
            SessionEntryKind::UserMessage(u) => out.push(RenderedEntry::User(u.content.clone())),
            SessionEntryKind::AssistantMessage(a) => {
                out.push(RenderedEntry::Assistant(a.content.clone()))
            }
            SessionEntryKind::ToolCall(call) => {
                let args = match &call.inline_args {
                    Some(v) => v.clone(),
                    None => archived_args_placeholder(&call.arg_artifacts),
                };
                out.push(RenderedEntry::ToolCall {
                    group_id: call.tool_call_group_id.clone(),
                    id: call
                        .provider_tool_call_id
                        .clone()
                        .unwrap_or_else(|| entry.meta.id.0.clone()),
                    name: call.tool_name.clone(),
                    args,
                });
            }
            SessionEntryKind::ToolResult(result) => {
                let tool = tool_names
                    .get(&result.call_entry_id)
                    .map(String::as_str)
                    .unwrap_or("");
                let call_id = tool_call_ids
                    .get(&result.call_entry_id)
                    .cloned()
                    .unwrap_or_else(|| result.call_entry_id.0.clone());
                let content = render_tool_result(entry, result, tool, overlay, artifacts);
                out.push(RenderedEntry::ToolResult {
                    group_id: result.tool_call_group_id.clone(),
                    call_id,
                    content,
                });
            }
            SessionEntryKind::MicroCompact(_) => {}
            SessionEntryKind::InteractiveCompact(_) => {}
        }
    }

    Ok(out)
}

/// Collect committed InteractiveCompact source ranges and their replacement
/// content. Sorted by start_seq. Returns an error when overlapping ranges are
/// detected.
fn collect_committed_replacements(entries: &[SessionEntry]) -> Result<Vec<(EntrySpan, String)>> {
    let mut result: Vec<(EntrySpan, String)> = Vec::new();
    for entry in entries {
        if let SessionEntryKind::InteractiveCompact(ic) = &entry.kind {
            if matches!(ic.outcome, InteractiveCompactOutcome::Committed) {
                if let Some(ref r) = ic.result {
                    result.push((ic.source_range.clone(), r.content.clone()));
                }
            }
        }
    }
    // Sort by start_seq for deterministic rendering.
    result.sort_by(|a, b| a.0.start_seq.cmp(&b.0.start_seq));

    // Detect overlaps — return a diagnostic error, not a preview.
    for i in 1..result.len() {
        if result[i].0.start_seq < result[i - 1].0.end_seq_exclusive {
            return Err(BytodeError::Session(format!(
                "overlapping committed interactive compact ranges: seq {}-{} and {}-{}",
                result[i - 1].0.start_seq,
                result[i - 1].0.end_seq_exclusive,
                result[i].0.start_seq,
                result[i].0.end_seq_exclusive,
            )));
        }
    }

    Ok(result)
}

fn render_tool_result(
    entry: &SessionEntry,
    result: &crate::session::model::ToolResultEntry,
    tool: &str,
    overlay: &CompactOverlay,
    artifacts: &ArtifactStore,
) -> String {
    // Compacted by a micro compact pass.
    if let Some(view) = overlay.view_for(&entry.meta.id) {
        if let Some(art) = view.artifact_refs.first() {
            return compacted_block(&entry.meta.id, tool, result.status, art, artifacts);
        }
    }
    // Inline content that was small enough to keep verbatim.
    if let Some(content) = &result.inline_content {
        return content.clone();
    }
    // Archived at record time: render its artifact as a preview block.
    if let Some(art) = result.artifacts.first() {
        return compacted_block(&entry.meta.id, tool, result.status, art, artifacts);
    }
    result.preview.clone().unwrap_or_default()
}

fn compacted_block(
    entry_id: &EntryId,
    tool: &str,
    status: ToolStatus,
    art: &ArtifactRef,
    artifacts: &ArtifactStore,
) -> String {
    let rel = art.relative_path.display();
    if !artifacts.exists(art) {
        return format!(
            "[artifact missing]\nentry: {}\nartifact: {}",
            entry_id.0, rel
        );
    }
    if artifacts.read_verified(art).is_err() {
        return format!(
            "[artifact checksum mismatch]\nentry: {}\nartifact: {}",
            entry_id.0, rel
        );
    }

    let excerpt = indent(&art.preview);
    match art.kind {
        ArtifactKind::FileSnapshot => format!(
            "[file snapshot compacted]\noriginal_size: {} bytes\nartifact: {}\npreview:\n{}",
            art.byte_len, rel, excerpt
        ),
        ArtifactKind::Diff => format!(
            "[diff compacted]\noriginal_size: {} bytes\nartifact: {}\npreview:\n{}",
            art.byte_len,
            rel,
            diff_files(&art.preview)
        ),
        _ => format!(
            "[tool output compacted]\ntool: {}\nstatus: {}\noriginal_size: {} bytes\nartifact: {}\npreview:\n{}",
            tool,
            status_label(status),
            art.byte_len,
            rel,
            excerpt
        ),
    }
}

fn status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Ok => "ok",
        ToolStatus::Error => "error",
        ToolStatus::Cancelled => "cancelled",
    }
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Best-effort list of files changed, derived from a diff/write preview.
fn diff_files(preview: &str) -> String {
    let mut files: Vec<String> = Vec::new();
    for line in preview.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            if let Some(file) = rest.split(" b/").next() {
                files.push(file.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            files.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("Wrote ") {
            if let Some(file) = rest.split_whitespace().next() {
                files.push(file.to_string());
            }
        }
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        return format!("    files changed:\n{}", indent(preview));
    }
    let mut out = String::from("    files changed:");
    for f in files {
        out.push_str(&format!("\n    - {f}"));
    }
    out
}

fn archived_args_placeholder(arg_artifacts: &[ArtifactRef]) -> serde_json::Value {
    match arg_artifacts.first() {
        Some(art) => serde_json::json!({
            "_archived_args_artifact": art.relative_path.display().to_string(),
            "_preview": art.preview,
        }),
        None => serde_json::json!({}),
    }
}

fn tool_name_index(entries: &[SessionEntry]) -> HashMap<EntryId, String> {
    let mut map = HashMap::new();
    for entry in entries {
        if let SessionEntryKind::ToolCall(call) = &entry.kind {
            map.insert(entry.meta.id.clone(), call.tool_name.clone());
        }
    }
    map
}

fn tool_call_id_index(entries: &[SessionEntry]) -> HashMap<EntryId, String> {
    let mut map = HashMap::new();
    for entry in entries {
        if let SessionEntryKind::ToolCall(call) = &entry.kind {
            map.insert(
                entry.meta.id.clone(),
                call.provider_tool_call_id
                    .clone()
                    .unwrap_or_else(|| entry.meta.id.0.clone()),
            );
        }
    }
    map
}

/// Classify a tool's output for archiving (re-exported helper for the runtime).
pub fn artifact_kind_for_tool(tool_name: &str) -> ArtifactKind {
    kind_for_tool(tool_name)
}
