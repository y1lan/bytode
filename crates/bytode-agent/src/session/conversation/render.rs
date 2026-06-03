//! Canonical context view construction. Builds `CanonicalContext` from
//! replayed canonical records with compact replacements applied.

use crate::session::conversation::model::{
    CanonicalCompactReplacement, CanonicalRecord, CanonicalRecordKind,
};

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
pub fn build_canonical_context(records: &[CanonicalRecord]) -> CanonicalContext {
    let mut replacements: Vec<&CanonicalCompactReplacement> = records
        .iter()
        .filter_map(|r| match &r.kind {
            CanonicalRecordKind::CompactReplacement(cr) => Some(cr),
            _ => None,
        })
        .collect();

    // Stable sort by start_seq so overlapping detection is deterministic.
    replacements.sort_by_key(|cr| cr.source.start_seq);

    let mut replaced_until: Option<u64> = None;
    let mut effective: Vec<CanonicalRecord> = Vec::new();

    for record in records {
        let seq = record.meta.seq;

        // Consume any replacement whose source range starts at or before this seq.
        while let Some(cr) = replacements.first() {
            if cr.source.start_seq <= seq {
                let end = cr.source.end_seq_exclusive;
                replaced_until = Some(replaced_until.unwrap_or(0).max(end));
                // Emit the replacement as a synthetic AssistantResponse.
                effective.push(CanonicalRecord {
                    meta: record.meta.clone(),
                    kind: CanonicalRecordKind::AssistantResponse(
                        crate::session::conversation::model::CanonicalAssistantResponse {
                            turn_id: crate::session::conversation::model::TurnId("compact".into()),
                            response_id: crate::session::conversation::model::ResponseId(
                                "compact".into(),
                            ),
                            message_id: crate::session::conversation::model::CanonicalMessageId(
                                "compact".into(),
                            ),
                            parts: vec![
                                crate::session::conversation::model::CanonicalAssistantPart::Text {
                                    content: cr.content.clone(),
                                },
                            ],
                        },
                    ),
                });
                replacements.remove(0);
            } else {
                break;
            }
        }

        // Skip records that fall inside a replacement range.
        if let Some(until) = replaced_until {
            if seq < until {
                // Skip CompactReplacement records too — they're consumed above.
                if matches!(record.kind, CanonicalRecordKind::CompactReplacement(_)) {
                    continue;
                }
                // Still skip if within replaced range.
                if seq < until {
                    continue;
                }
            }
        }

        // Skip CompactReplacement records (already consumed).
        if matches!(record.kind, CanonicalRecordKind::CompactReplacement(_)) {
            continue;
        }

        effective.push(record.clone());
    }

    CanonicalContext { records: effective }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation::model::{
        CanonicalAssistantPart, CanonicalAssistantResponse, CanonicalContent, CanonicalMessageId,
        CanonicalMeta, CanonicalRecordKind, CanonicalSpan, CanonicalToolResultPart,
        CanonicalToolResults, CanonicalToolStatus, CanonicalTurnFinished, CanonicalTurnStarted,
        ResponseId, ToolCallId, TurnId,
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
        let ctx = build_canonical_context(&[]);
        assert!(ctx.records.is_empty());
    }

    #[test]
    fn records_without_replacements_pass_through() {
        let records = vec![turn_started(0, "t1", "hello")];
        let ctx = build_canonical_context(&records);
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

        let ctx = build_canonical_context(&records);
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

        let ctx = build_canonical_context(&records);
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
}
