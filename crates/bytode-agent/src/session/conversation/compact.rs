//! Canonical-boundary source span selection and evidence construction for
//! interactive compact. Operates purely on canonical records — never on flat
//! `SessionEntry`.

use crate::session::conversation::model::{CanonicalRecord, CanonicalRecordKind, CanonicalSpan};

/// Number of most-recent completed turns kept out of any compact span.
const KEEP_RECENT_TURNS: usize = 6;

/// Select a contiguous source span for interactive compact, on canonical
/// boundaries. The span is a half-open canonical seq range `[start, end)` that:
/// - begins after any already-committed `CompactReplacement` range,
/// - ends at a `TurnFinished` boundary (so an `AssistantResponse` and its
///   `ToolResults` are never split),
/// - excludes the most recent `KEEP_RECENT_TURNS` completed turns,
/// - excludes any unfinished trailing turn.
///
/// Returns `None` when nothing outside the recent window remains to compact.
pub fn select_canonical_source_span(records: &[CanonicalRecord]) -> Option<CanonicalSpan> {
    if records.is_empty() {
        return None;
    }

    // Start after the furthest already-replaced range.
    let mut covered_until: u64 = records.first().map(|r| r.meta.seq).unwrap_or(0);
    for record in records {
        if let CanonicalRecordKind::CompactReplacement(cr) = &record.kind {
            covered_until = covered_until.max(cr.source.end_seq_exclusive);
        }
    }

    // Collect the exclusive-end seq of every completed turn (one past its
    // `TurnFinished` record). These are the only legal span boundaries.
    let finished_ends: Vec<u64> = records
        .iter()
        .filter(|r| matches!(r.kind, CanonicalRecordKind::TurnFinished(_)))
        .map(|r| r.meta.seq + 1)
        .collect();

    if finished_ends.len() <= KEEP_RECENT_TURNS {
        return None;
    }

    // Cut off the most recent KEEP_RECENT_TURNS completed turns.
    let cutoff = finished_ends[finished_ends.len() - KEEP_RECENT_TURNS - 1];

    if covered_until >= cutoff {
        return None;
    }

    Some(CanonicalSpan {
        start_seq: covered_until,
        end_seq_exclusive: cutoff,
    })
}

/// Build an evidence excerpt string from the canonical records inside `span`.
/// Used as the source material the compact LLM session compresses.
pub fn canonical_evidence(records: &[CanonicalRecord], span: &CanonicalSpan) -> String {
    let mut out = String::new();
    for record in records {
        let seq = record.meta.seq;
        if seq < span.start_seq || seq >= span.end_seq_exclusive {
            continue;
        }
        match &record.kind {
            CanonicalRecordKind::TurnStarted(ts) => {
                out.push_str(&format!(
                    "[seq {seq}] User: {}\n",
                    truncate(&ts.content, 400)
                ));
            }
            CanonicalRecordKind::AssistantResponse(ar) => {
                for part in &ar.parts {
                    match part {
                        crate::session::conversation::model::CanonicalAssistantPart::Text {
                            content,
                        } => {
                            out.push_str(&format!(
                                "[seq {seq}] Assistant: {}\n",
                                truncate(content, 400)
                            ));
                        }
                        crate::session::conversation::model::CanonicalAssistantPart::ToolCall {
                            name,
                            ..
                        } => {
                            out.push_str(&format!("[seq {seq}] ToolCall: {name}\n"));
                        }
                    }
                }
            }
            CanonicalRecordKind::ToolResults(tr) => {
                for part in &tr.results {
                    let preview = match &part.content {
                        crate::session::conversation::model::CanonicalContent::Inline(s) => {
                            truncate(s, 300)
                        }
                        crate::session::conversation::model::CanonicalContent::Artifact(a) => {
                            truncate(&a.preview, 300)
                        }
                    };
                    out.push_str(&format!("[seq {seq}] ToolResult: {preview}\n"));
                }
            }
            CanonicalRecordKind::TurnFinished(_)
            | CanonicalRecordKind::CompactReplacement(_)
            | CanonicalRecordKind::ToolResultCompacted(_) => {}
        }
    }
    out
}

fn truncate(s: &str, max_chars: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= max_chars {
        one_line
    } else {
        let mut out: String = one_line.chars().take(max_chars).collect();
        out.push_str("...");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation::model::{
        CanonicalAssistantPart, CanonicalAssistantResponse, CanonicalCompactReplacement,
        CanonicalContent, CanonicalMessageId, CanonicalMeta, CanonicalToolResultPart,
        CanonicalToolResults, CanonicalToolStatus, CanonicalTurnFinished, CanonicalTurnStarted,
        ResponseId, ToolCallId, TurnId,
    };
    use crate::session::model::{ArtifactId, ArtifactKind, ArtifactRef, EntryId};

    fn meta(seq: u64) -> CanonicalMeta {
        CanonicalMeta { seq, created_at: 0 }
    }

    fn turn(seq_start: u64, turn_id: &str) -> Vec<CanonicalRecord> {
        vec![
            CanonicalRecord {
                meta: meta(seq_start),
                kind: CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                    turn_id: TurnId(turn_id.into()),
                    user_message_id: CanonicalMessageId(format!("m-{seq_start}")),
                    content: format!("task {turn_id}"),
                }),
            },
            CanonicalRecord {
                meta: meta(seq_start + 1),
                kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                    turn_id: TurnId(turn_id.into()),
                    response_id: ResponseId(format!("r-{turn_id}")),
                    message_id: CanonicalMessageId(format!("m-{}", seq_start + 1)),
                    parts: vec![CanonicalAssistantPart::Text {
                        content: "answer".into(),
                    }],
                }),
            },
            CanonicalRecord {
                meta: meta(seq_start + 2),
                kind: CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                    turn_id: TurnId(turn_id.into()),
                }),
            },
        ]
    }

    fn artifact() -> ArtifactRef {
        ArtifactRef {
            id: ArtifactId("sha".into()),
            kind: ArtifactKind::CompactArchive,
            relative_path: std::path::PathBuf::from("artifacts/compact/sha.txt"),
            source_entry_id: EntryId::from_seq(0),
            byte_len: 1,
            sha256: "sha".into(),
            preview: "p".into(),
        }
    }

    #[test]
    fn too_few_turns_yields_none() {
        let mut records = Vec::new();
        for i in 0..3 {
            records.extend(turn(i * 3, &format!("t{i}")));
        }
        // 3 completed turns <= KEEP_RECENT_TURNS, nothing to compact.
        assert!(select_canonical_source_span(&records).is_none());
    }

    #[test]
    fn span_excludes_recent_turns_and_lands_on_boundary() {
        let mut records = Vec::new();
        for i in 0..8 {
            records.extend(turn(i * 3, &format!("t{i}")));
        }
        // 8 completed turns; keep 6 most recent -> compact first 2 turns.
        let span = select_canonical_source_span(&records).expect("span");
        assert_eq!(span.start_seq, 0);
        // first 2 turns = seq 0..6 (turn 0: 0,1,2; turn 1: 3,4,5; cutoff = 6).
        assert_eq!(span.end_seq_exclusive, 6);
    }

    #[test]
    fn already_replaced_range_advances_start() {
        let mut records = Vec::new();
        for i in 0..9 {
            records.extend(turn(i * 3, &format!("t{i}")));
        }
        // A prior replacement covered seq 0..6.
        records.push(CanonicalRecord {
            meta: meta(100),
            kind: CanonicalRecordKind::CompactReplacement(CanonicalCompactReplacement {
                source: CanonicalSpan {
                    start_seq: 0,
                    end_seq_exclusive: 6,
                },
                content: "old".into(),
                evidence_pack_ref: artifact(),
            }),
        });
        let span = select_canonical_source_span(&records).expect("span");
        assert_eq!(span.start_seq, 6);
    }

    #[test]
    fn evidence_includes_only_span_records() {
        let mut records = Vec::new();
        records.extend(turn(0, "t0"));
        records.push(CanonicalRecord {
            meta: meta(3),
            kind: CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t0".into()),
                response_id: ResponseId("r-t0".into()),
                results: vec![CanonicalToolResultPart {
                    tool_call_id: ToolCallId("c1".into()),
                    status: CanonicalToolStatus::Ok,
                    content: CanonicalContent::Inline("INSIDE".into()),
                }],
            }),
        });
        records.extend(turn(10, "t1"));

        let ev = canonical_evidence(
            &records,
            &CanonicalSpan {
                start_seq: 0,
                end_seq_exclusive: 4,
            },
        );
        assert!(ev.contains("task t0"));
        assert!(ev.contains("INSIDE"));
        assert!(!ev.contains("task t1"));
    }
}
