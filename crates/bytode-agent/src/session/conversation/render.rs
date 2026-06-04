//! Canonical context view construction. Builds `CanonicalContext` from
//! replayed canonical records with compact replacements applied.

use crate::error::Result;
use crate::session::conversation::model::{
    CanonicalAssistantPart, CanonicalAssistantResponse, CanonicalCompactReplacement,
    CanonicalContent, CanonicalMessageId, CanonicalRecord, CanonicalRecordKind,
    CanonicalToolResultCompacted, ResponseId, ToolCallId, TurnId,
};
use std::collections::{HashMap, HashSet};

/// The canonical context view presented to the provider adapter.
/// Compact replacements have already been applied — this is the
/// effective conversation to serialize as provider messages.
#[derive(Debug, Clone)]
pub struct CanonicalContext {
    pub records: Vec<CanonicalRecord>,
}

/// Build a `CanonicalContext` by replaying canonical records and applying
/// `CompactReplacement` records. A `CompactReplacement` replaces all records
/// whose seq falls within its source span with a single synthetic
/// `AssistantResponse` containing the replacement text.
pub fn build_canonical_context(records: &[CanonicalRecord]) -> Result<CanonicalContext> {
    let mut replacements: Vec<(&CanonicalCompactReplacement, u64)> = records
        .iter()
        .filter_map(|r| match &r.kind {
            CanonicalRecordKind::CompactReplacement(cr) => Some((cr, r.meta.seq)),
            _ => None,
        })
        .collect();
    let mut compacted_tool_results: HashMap<(TurnId, ResponseId, ToolCallId), (u64, &CanonicalToolResultCompacted)> =
        HashMap::new();

    for record in records {
        if let CanonicalRecordKind::ToolResultCompacted(compacted) = &record.kind {
            compacted_tool_results.insert(
                (
                    compacted.turn_id.clone(),
                    compacted.response_id.clone(),
                    compacted.tool_call_id.clone(),
                ),
                (record.meta.seq, compacted),
            );
        }
    }

    // Stable sort by start_seq so overlapping detection is deterministic.
    replacements.sort_by_key(|(cr, _)| cr.source.start_seq);
    for i in 1..replacements.len() {
        let prev = replacements[i - 1].0;
        let curr = replacements[i].0;
        if curr.source.start_seq < prev.source.end_seq_exclusive {
            return Err(crate::error::BytodeError::Session(format!(
                "overlapping canonical compact ranges: seq {}-{} and {}-{}",
                prev.source.start_seq,
                prev.source.end_seq_exclusive,
                curr.source.start_seq,
                curr.source.end_seq_exclusive,
            )));
        }
    }

    let mut replacement_idx = 0usize;
    let mut skip_until: Option<u64> = None;
    let mut effective: Vec<CanonicalRecord> = Vec::new();
    let mut seen_compacted_targets: HashSet<(TurnId, ResponseId, ToolCallId)> = HashSet::new();

    for record in records {
        let seq = record.meta.seq;

        if let Some(until) = skip_until {
            if seq < until {
                continue;
            }
            skip_until = None;
        }

        while let Some((cr, compact_seq)) = replacements.get(replacement_idx) {
            if cr.source.start_seq < seq {
                return Err(crate::error::BytodeError::Session(format!(
                    "canonical compact source start seq {} does not align to a record boundary",
                    cr.source.start_seq
                )));
            }
            if cr.source.start_seq == seq {
                effective.push(CanonicalRecord {
                    meta: record.meta.clone(),
                    kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                        turn_id: TurnId(format!("compact-{}", compact_seq)),
                        response_id: ResponseId(format!("compact-r-{}", compact_seq)),
                        message_id: CanonicalMessageId(format!("compact-m-{}", compact_seq)),
                        parts: vec![CanonicalAssistantPart::Text {
                            content: cr.content.clone(),
                        }],
                    }),
                });
                skip_until = Some(cr.source.end_seq_exclusive);
                replacement_idx += 1;
                break;
            }
            break;
        }
        if skip_until.is_some() {
            continue;
        }

        if matches!(
            record.kind,
            CanonicalRecordKind::CompactReplacement(_) | CanonicalRecordKind::ToolResultCompacted(_)
        ) {
            continue;
        }

        let mut record = record.clone();
        if let CanonicalRecordKind::ToolResults(tr) = &mut record.kind {
            for part in &mut tr.results {
                let key = (
                    tr.turn_id.clone(),
                    tr.response_id.clone(),
                    part.tool_call_id.clone(),
                );
                if let Some((_, compacted)) = compacted_tool_results.get(&key) {
                    part.content = CanonicalContent::Inline(compacted.preview.clone());
                    seen_compacted_targets.insert(key);
                }
            }
        }
        effective.push(record);
    }

    if let Some((cr, _)) = replacements.get(replacement_idx) {
        return Err(crate::error::BytodeError::Session(format!(
            "canonical compact source start seq {} does not align to a record boundary",
            cr.source.start_seq
        )));
    }

    for ((turn_id, response_id, tool_call_id), _) in compacted_tool_results {
        if !seen_compacted_targets.contains(&(
            turn_id.clone(),
            response_id.clone(),
            tool_call_id.clone(),
        )) && !records.iter().any(|record| {
            matches!(
                &record.kind,
                CanonicalRecordKind::ToolResults(tr)
                    if tr.turn_id == turn_id
                        && tr.response_id == response_id
                        && tr.results.iter().any(|part| part.tool_call_id == tool_call_id)
            )
        }) {
            return Err(crate::error::BytodeError::Session(format!(
                "canonical compacted tool result target not found: turn_id={}, response_id={}, tool_call_id={}",
                turn_id.0, response_id.0, tool_call_id.0
            )));
        }
    }

    Ok(CanonicalContext { records: effective })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation::model::{
        CanonicalAssistantPart, CanonicalAssistantResponse, CanonicalContent, CanonicalMessageId,
        CanonicalMeta, CanonicalRecordKind, CanonicalSpan, CanonicalToolResultCompacted,
        CanonicalToolResultPart, CanonicalToolResults, CanonicalToolStatus, CanonicalTurnFinished,
        CanonicalTurnStarted, ResponseId, ToolCallId, TurnId,
    };
    use crate::session::model::{ArtifactId, ArtifactKind, ArtifactRef, EntryId};

    fn now() -> i64 {
        1000
    }

    fn artifact_ref() -> ArtifactRef {
        ArtifactRef {
            id: ArtifactId("sha".into()),
            kind: ArtifactKind::CompactArchive,
            relative_path: std::path::PathBuf::from("artifacts/compact/sha.txt"),
            source_entry_id: EntryId::from_seq(0),
            byte_len: 100,
            sha256: "sha".into(),
            preview: "preview".into(),
        }
    }

    fn turn_started(seq: u64, turn_id: &str, content: &str) -> CanonicalRecord {
        CanonicalRecord {
            meta: CanonicalMeta {
                seq,
                created_at: now(),
            },
            kind: CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId(turn_id.into()),
                user_message_id: CanonicalMessageId(format!("m-{seq}")),
                content: content.into(),
            }),
        }
    }

    fn assistant_response(
        seq: u64,
        turn_id: &str,
        response_id: &str,
        content: &str,
    ) -> CanonicalRecord {
        CanonicalRecord {
            meta: CanonicalMeta {
                seq,
                created_at: now(),
            },
            kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId(turn_id.into()),
                response_id: ResponseId(response_id.into()),
                message_id: CanonicalMessageId(format!("m-{seq}")),
                parts: vec![CanonicalAssistantPart::Text {
                    content: content.into(),
                }],
            }),
        }
    }

    #[test]
    fn empty_records_yields_empty_context() {
        let ctx = build_canonical_context(&[]).unwrap();
        assert!(ctx.records.is_empty());
    }

    #[test]
    fn records_without_replacements_pass_through() {
        let records = vec![turn_started(0, "t1", "hello")];
        let ctx = build_canonical_context(&records).unwrap();
        assert_eq!(ctx.records.len(), 1);
    }

    #[test]
    fn compact_replacement_replaces_source_range() {
        let records = vec![
            turn_started(0, "t1", "old"),
            assistant_response(1, "t1", "r1", "old response"),
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 2,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::CompactReplacement(CanonicalCompactReplacement {
                    source: CanonicalSpan {
                        start_seq: 0,
                        end_seq_exclusive: 2,
                    },
                    content: "REPLACED".into(),
                    evidence_pack_ref: artifact_ref(),
                }),
            },
        ];

        let ctx = build_canonical_context(&records).unwrap();
        assert_eq!(ctx.records.len(), 1);
        let CanonicalRecordKind::AssistantResponse(ar) = &ctx.records[0].kind else {
            panic!("expected AssistantResponse");
        };
        let CanonicalAssistantPart::Text { content } = &ar.parts[0] else {
            panic!("expected Text part");
        };
        assert_eq!(content, "REPLACED");
    }

    #[test]
    fn compact_replacement_preserves_tool_call_boundary() {
        // A compact replacement that covers only the first turn preserves the
        // second turn's tool call/result boundary intact.
        let records = vec![
            turn_started(0, "t1", "old task"),
            assistant_response(1, "t1", "r1", "old answer"),
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 2,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::CompactReplacement(CanonicalCompactReplacement {
                    source: CanonicalSpan {
                        start_seq: 0,
                        end_seq_exclusive: 2,
                    },
                    content: "COMPRESSED".into(),
                    evidence_pack_ref: artifact_ref(),
                }),
            },
            turn_started(3, "t2", "new task"),
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 4,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                    turn_id: TurnId("t2".into()),
                    response_id: ResponseId("r2".into()),
                    message_id: CanonicalMessageId("m-4".into()),
                    parts: vec![CanonicalAssistantPart::ToolCall {
                        tool_call_id: ToolCallId("call_1".into()),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.rs"}),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 5,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResults(CanonicalToolResults {
                    turn_id: TurnId("t2".into()),
                    response_id: ResponseId("r2".into()),
                    results: vec![CanonicalToolResultPart {
                        tool_call_id: ToolCallId("call_1".into()),
                        status: CanonicalToolStatus::Ok,
                        content: CanonicalContent::Inline("file content".into()),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 6,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                    turn_id: TurnId("t2".into()),
                }),
            },
        ];

        let ctx = build_canonical_context(&records).unwrap();
        // Should have: compact replacement (1) + turn_started (1) + assistant (1) + tool_results (1) + turn_finished (1)
        assert_eq!(ctx.records.len(), 5);
        // The tool call / tool result pair for t2 must be intact.
        let kinds: Vec<&str> = ctx
            .records
            .iter()
            .map(|r| match &r.kind {
                CanonicalRecordKind::TurnStarted(_) => "TurnStarted",
                CanonicalRecordKind::AssistantResponse(_) => "AssistantResponse",
                CanonicalRecordKind::ToolResults(_) => "ToolResults",
                CanonicalRecordKind::TurnFinished(_) => "TurnFinished",
                CanonicalRecordKind::CompactReplacement(_) => "CompactReplacement",
                CanonicalRecordKind::ToolResultCompacted(_) => "ToolResultCompacted",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "AssistantResponse", // compact replacement
                "TurnStarted",
                "AssistantResponse",
                "ToolResults",
                "TurnFinished"
            ]
        );
    }

    #[test]
    fn tool_result_compacted_replaces_content_in_context() {
        let records = vec![
            turn_started(0, "t1", "task"),
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 1,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    message_id: CanonicalMessageId("m-1".into()),
                    parts: vec![CanonicalAssistantPart::ToolCall {
                        tool_call_id: ToolCallId("call_1".into()),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.rs"}),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 2,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResults(CanonicalToolResults {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    results: vec![CanonicalToolResultPart {
                        tool_call_id: ToolCallId("call_1".into()),
                        status: CanonicalToolStatus::Ok,
                        content: CanonicalContent::Inline("very long content".into()),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 3,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResultCompacted(CanonicalToolResultCompacted {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    tool_call_id: ToolCallId("call_1".into()),
                    preview: "short preview".into(),
                    artifact_ref: artifact_ref(),
                }),
            },
        ];

        let ctx = build_canonical_context(&records).unwrap();
        let CanonicalRecordKind::ToolResults(tr) = &ctx.records[2].kind else {
            panic!("expected ToolResults");
        };
        let CanonicalContent::Inline(content) = &tr.results[0].content else {
            panic!("expected inline compacted preview");
        };
        assert_eq!(content, "short preview");
    }

    #[test]
    fn missing_compacted_tool_result_target_returns_error() {
        let records = vec![
            turn_started(0, "t1", "task"),
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 1,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResultCompacted(CanonicalToolResultCompacted {
                    turn_id: TurnId("t-missing".into()),
                    response_id: ResponseId("r-missing".into()),
                    tool_call_id: ToolCallId("call-missing".into()),
                    preview: "short preview".into(),
                    artifact_ref: artifact_ref(),
                }),
            },
        ];

        let err = build_canonical_context(&records).unwrap_err();
        assert!(err
            .to_string()
            .contains("canonical compacted tool result target not found"));
    }

    #[test]
    fn missing_compacted_tool_result_response_returns_error() {
        let records = vec![
            turn_started(0, "t1", "task"),
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 1,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    message_id: CanonicalMessageId("m-1".into()),
                    parts: vec![CanonicalAssistantPart::ToolCall {
                        tool_call_id: ToolCallId("call_1".into()),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.rs"}),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 2,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResults(CanonicalToolResults {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    results: vec![CanonicalToolResultPart {
                        tool_call_id: ToolCallId("call_1".into()),
                        status: CanonicalToolStatus::Ok,
                        content: CanonicalContent::Inline("content".into()),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 3,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResultCompacted(CanonicalToolResultCompacted {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r-missing".into()),
                    tool_call_id: ToolCallId("call_1".into()),
                    preview: "preview".into(),
                    artifact_ref: artifact_ref(),
                }),
            },
        ];

        assert!(build_canonical_context(&records).is_err());
    }

    #[test]
    fn missing_compacted_tool_result_tool_call_returns_error() {
        let records = vec![
            turn_started(0, "t1", "task"),
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 1,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    message_id: CanonicalMessageId("m-1".into()),
                    parts: vec![CanonicalAssistantPart::ToolCall {
                        tool_call_id: ToolCallId("call_1".into()),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.rs"}),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 2,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResults(CanonicalToolResults {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    results: vec![CanonicalToolResultPart {
                        tool_call_id: ToolCallId("call_1".into()),
                        status: CanonicalToolStatus::Ok,
                        content: CanonicalContent::Inline("content".into()),
                    }],
                }),
            },
            CanonicalRecord {
                meta: CanonicalMeta {
                    seq: 3,
                    created_at: now(),
                },
                kind: CanonicalRecordKind::ToolResultCompacted(CanonicalToolResultCompacted {
                    turn_id: TurnId("t1".into()),
                    response_id: ResponseId("r1".into()),
                    tool_call_id: ToolCallId("call-missing".into()),
                    preview: "preview".into(),
                    artifact_ref: artifact_ref(),
                }),
            },
        ];

        assert!(build_canonical_context(&records).is_err());
    }
}
