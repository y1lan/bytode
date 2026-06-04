//! Provider adapter: converts `CanonicalContext` into OpenAI-compatible
//! `ChatCompletionRequestMessage` vec. All tool call grouping, tool result
//! binding, and boundary validation happens here — never by guessing from
//! flat `SessionEntry`.

use crate::error::Result;
use crate::llm;
use crate::session::conversation::{
    CanonicalAssistantPart, CanonicalContent, CanonicalContext, CanonicalRecordKind, ResponseId,
    ToolCallId, TurnId,
};
use async_openai::types::ChatCompletionRequestMessage;
use std::collections::HashMap;

pub struct ProviderAdapter;

impl ProviderAdapter {
    pub fn adapt(ctx: &CanonicalContext) -> Result<Vec<ChatCompletionRequestMessage>> {
        let mut messages: Vec<ChatCompletionRequestMessage> = Vec::new();

        // Build an index: ResponseId -> set of ToolCallIds from that response.
        let mut response_tool_calls: HashMap<ResponseId, Vec<ToolCallId>> = HashMap::new();
        for rec in ctx.records.iter() {
            if let CanonicalRecordKind::AssistantResponse(ar) = &rec.kind {
                let tc_ids: Vec<ToolCallId> = ar
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        CanonicalAssistantPart::ToolCall { tool_call_id, .. } => {
                            Some(tool_call_id.clone())
                        }
                        _ => None,
                    })
                    .collect();
                if !tc_ids.is_empty() {
                    response_tool_calls.insert(ar.response_id.clone(), tc_ids);
                }
            }
        }

        // Track the current turn for cross-turn detection.
        let mut current_turn: Option<TurnId> = None;

        for rec in ctx.records.iter() {
            match &rec.kind {
                CanonicalRecordKind::TurnStarted(ts) => {
                    current_turn = Some(ts.turn_id.clone());
                    messages.push(llm::build_user_message(&ts.content));
                }

                CanonicalRecordKind::AssistantResponse(ar) => {
                    // Validate turn consistency.
                    if let Some(ref turn) = current_turn {
                        if &ar.turn_id != turn {
                            return Err(crate::error::BytodeError::Session(format!(
                                "AssistantResponse turn_id {:?} does not match current turn {:?}",
                                ar.turn_id, turn
                            )));
                        }
                    }

                    let has_tool_calls = ar
                        .parts
                        .iter()
                        .any(|p| matches!(p, CanonicalAssistantPart::ToolCall { .. }));

                    if has_tool_calls {
                        let tool_calls: Vec<llm::ToolCall> = ar
                            .parts
                            .iter()
                            .filter_map(|p| match p {
                                CanonicalAssistantPart::ToolCall {
                                    tool_call_id,
                                    name,
                                    arguments,
                                } => Some(llm::ToolCall {
                                    id: tool_call_id.0.clone(),
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                }),
                                _ => None,
                            })
                            .collect();

                        messages.push(llm::build_assistant_tool_calls_message(&tool_calls));
                    } else {
                        // Text-only response.
                        let content = ar
                            .parts
                            .iter()
                            .filter_map(|p| match p {
                                CanonicalAssistantPart::Text { content } => Some(content.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("");
                        messages.push(llm::build_assistant_text_message(&content));
                    }
                }

                CanonicalRecordKind::ToolResults(tr) => {
                    // Validate cross-turn: ToolResults must belong to the current turn.
                    if let Some(ref turn) = current_turn {
                        if &tr.turn_id != turn {
                            return Err(crate::error::BytodeError::Session(format!(
                                "ToolResults turn_id {:?} does not match current turn {:?}",
                                tr.turn_id, turn
                            )));
                        }
                    }

                    // Validate response_id exists.
                    let expected_tool_call_ids =
                        response_tool_calls.get(&tr.response_id).ok_or_else(|| {
                            crate::error::BytodeError::Session(format!(
                                "ToolResults response_id {:?} does not match any AssistantResponse",
                                tr.response_id
                            ))
                        })?;

                    // Validate each tool result corresponds to a tool call in that response.
                    for part in &tr.results {
                        if !expected_tool_call_ids.contains(&part.tool_call_id) {
                            return Err(crate::error::BytodeError::Session(format!(
                                "ToolResultPart tool_call_id {:?} does not belong to AssistantResponse {:?}",
                                part.tool_call_id, tr.response_id
                            )));
                        }

                        let content_str = match &part.content {
                            CanonicalContent::Inline(s) => s.clone(),
                            CanonicalContent::Artifact(a) => a.preview.clone(),
                        };

                        messages.push(llm::build_tool_result_message(
                            &content_str,
                            &part.tool_call_id.0,
                        ));
                    }
                }

                CanonicalRecordKind::TurnFinished(_)
                | CanonicalRecordKind::CompactReplacement(_)
                | CanonicalRecordKind::ToolResultCompacted(_) => {
                    // TurnFinished and CompactReplacement are not rendered as messages.
                }
            }
        }

        Ok(messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::conversation::build_canonical_context;
    use crate::session::conversation::model::{
        CanonicalAssistantResponse, CanonicalCompactReplacement, CanonicalContent,
        CanonicalMessageId, CanonicalMeta, CanonicalRecord, CanonicalSpan,
        CanonicalToolResultCompacted, CanonicalToolResultPart, CanonicalToolResults,
        CanonicalToolStatus, CanonicalTurnFinished, CanonicalTurnStarted,
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

    fn make_records(records: Vec<CanonicalRecordKind>) -> Vec<CanonicalRecord> {
        records
            .into_iter()
            .enumerate()
            .map(|(i, kind)| CanonicalRecord {
                meta: CanonicalMeta {
                    seq: i as u64,
                    created_at: now(),
                },
                kind,
            })
            .collect()
    }

    fn ctx(records: Vec<CanonicalRecordKind>) -> CanonicalContext {
        CanonicalContext {
            records: make_records(records),
        }
    }

    #[test]
    fn two_turns_with_tool_calls_do_not_merge() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task 1".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                message_id: CanonicalMessageId("m2".into()),
                parts: vec![CanonicalAssistantPart::ToolCall {
                    tool_call_id: ToolCallId("call_1".into()),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                }],
            }),
            CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                results: vec![CanonicalToolResultPart {
                    tool_call_id: ToolCallId("call_1".into()),
                    status: CanonicalToolStatus::Ok,
                    content: CanonicalContent::Inline("content a".into()),
                }],
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t1".into()),
            }),
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t2".into()),
                user_message_id: CanonicalMessageId("m3".into()),
                content: "task 2".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t2".into()),
                response_id: ResponseId("r2".into()),
                message_id: CanonicalMessageId("m4".into()),
                parts: vec![CanonicalAssistantPart::ToolCall {
                    tool_call_id: ToolCallId("call_2".into()),
                    name: "git_status".into(),
                    arguments: serde_json::json!({}),
                }],
            }),
            CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t2".into()),
                response_id: ResponseId("r2".into()),
                results: vec![CanonicalToolResultPart {
                    tool_call_id: ToolCallId("call_2".into()),
                    status: CanonicalToolStatus::Ok,
                    content: CanonicalContent::Inline("status".into()),
                }],
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t2".into()),
            }),
        ]);

        let messages = ProviderAdapter::adapt(&context).unwrap();

        // Messages order: user(t1), assistant(tool_calls r1), tool(call_1),
        //                 user(t2), assistant(tool_calls r2), tool(call_2)
        assert_eq!(messages.len(), 6);
        let roles: Vec<String> = messages
            .iter()
            .map(|m| {
                serde_json::to_value(m)
                    .unwrap()
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        assert_eq!(roles[0], "user");
        assert_eq!(roles[1], "assistant");
        assert_eq!(roles[2], "tool");
        assert_eq!(roles[3], "user");
        assert_eq!(roles[4], "assistant");
        assert_eq!(roles[5], "tool");
    }

    #[test]
    fn multi_tool_calls_in_one_response_are_single_assistant_message() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                message_id: CanonicalMessageId("m2".into()),
                parts: vec![
                    CanonicalAssistantPart::ToolCall {
                        tool_call_id: ToolCallId("call_1".into()),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "a.rs"}),
                    },
                    CanonicalAssistantPart::ToolCall {
                        tool_call_id: ToolCallId("call_2".into()),
                        name: "git_status".into(),
                        arguments: serde_json::json!({}),
                    },
                ],
            }),
            CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                results: vec![
                    CanonicalToolResultPart {
                        tool_call_id: ToolCallId("call_1".into()),
                        status: CanonicalToolStatus::Ok,
                        content: CanonicalContent::Inline("content a".into()),
                    },
                    CanonicalToolResultPart {
                        tool_call_id: ToolCallId("call_2".into()),
                        status: CanonicalToolStatus::Ok,
                        content: CanonicalContent::Inline("status".into()),
                    },
                ],
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t1".into()),
            }),
        ]);

        let messages = ProviderAdapter::adapt(&context).unwrap();

        // Messages order: user, assistant (with 2 tool_calls), tool(call_1), tool(call_2)
        assert_eq!(messages.len(), 4);
        let assistant = serde_json::to_value(&messages[1]).unwrap();
        let tool_calls = assistant
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .expect("assistant tool_calls");
        assert_eq!(tool_calls.len(), 2);
    }

    #[test]
    fn tool_results_with_missing_response_id_returns_error() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task".into(),
            }),
            // No AssistantResponse for r1 — ToolResults references nonexistent response.
            CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                results: vec![CanonicalToolResultPart {
                    tool_call_id: ToolCallId("call_1".into()),
                    status: CanonicalToolStatus::Ok,
                    content: CanonicalContent::Inline("content".into()),
                }],
            }),
        ]);

        let result = ProviderAdapter::adapt(&context);
        assert!(result.is_err());
    }

    #[test]
    fn tool_result_with_wrong_tool_call_id_returns_error() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                message_id: CanonicalMessageId("m2".into()),
                parts: vec![CanonicalAssistantPart::ToolCall {
                    tool_call_id: ToolCallId("call_1".into()),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                }],
            }),
            CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                results: vec![CanonicalToolResultPart {
                    tool_call_id: ToolCallId("call_NONEXISTENT".into()),
                    status: CanonicalToolStatus::Ok,
                    content: CanonicalContent::Inline("content".into()),
                }],
            }),
        ]);

        let result = ProviderAdapter::adapt(&context);
        assert!(result.is_err());
    }

    #[test]
    fn cross_turn_tool_results_returns_error() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task 1".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                message_id: CanonicalMessageId("m2".into()),
                parts: vec![CanonicalAssistantPart::ToolCall {
                    tool_call_id: ToolCallId("call_1".into()),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                }],
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t1".into()),
            }),
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t2".into()),
                user_message_id: CanonicalMessageId("m3".into()),
                content: "task 2".into(),
            }),
            // ToolResults references t1's response_id r1 but current turn is t2.
            CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                results: vec![CanonicalToolResultPart {
                    tool_call_id: ToolCallId("call_1".into()),
                    status: CanonicalToolStatus::Ok,
                    content: CanonicalContent::Inline("content".into()),
                }],
            }),
        ]);

        let result = ProviderAdapter::adapt(&context);
        assert!(result.is_err());
    }

    #[test]
    fn text_only_assistant_response_becomes_assistant_text_message() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "hello".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                message_id: CanonicalMessageId("m2".into()),
                parts: vec![CanonicalAssistantPart::Text {
                    content: "hi there".into(),
                }],
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t1".into()),
            }),
        ]);

        let messages = ProviderAdapter::adapt(&context).unwrap();
        assert_eq!(messages.len(), 2);
        let roles: Vec<String> = messages
            .iter()
            .map(|m| {
                serde_json::to_value(m)
                    .unwrap()
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        assert_eq!(roles[0], "user");
        assert_eq!(roles[1], "assistant");
    }

    #[test]
    fn compact_replacement_is_skipped_in_provider_messages() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task".into(),
            }),
            CanonicalRecordKind::CompactReplacement(CanonicalCompactReplacement {
                source: CanonicalSpan {
                    start_seq: 0,
                    end_seq_exclusive: 2,
                },
                content: "replaced".into(),
                evidence_pack_ref: artifact_ref(),
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t1".into()),
            }),
        ]);

        let messages = ProviderAdapter::adapt(&context).unwrap();
        // CompactReplacement should be ignored by the adapter — only the TurnStarted
        // produces a user message.
        assert_eq!(messages.len(), 1);
        let role = serde_json::to_value(&messages[0])
            .unwrap()
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        assert_eq!(role, "user");
    }

    #[test]
    fn compacted_tool_result_record_is_skipped_by_adapter() {
        let context = ctx(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task".into(),
            }),
            CanonicalRecordKind::ToolResultCompacted(CanonicalToolResultCompacted {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                tool_call_id: ToolCallId("call_1".into()),
                preview: "preview".into(),
                artifact_ref: artifact_ref(),
            }),
        ]);

        let messages = ProviderAdapter::adapt(&context).unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn canonical_context_micro_compact_shortens_provider_tool_message() {
        let records = make_records(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "task".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                message_id: CanonicalMessageId("m2".into()),
                parts: vec![CanonicalAssistantPart::ToolCall {
                    tool_call_id: ToolCallId("call_1".into()),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                }],
            }),
            CanonicalRecordKind::ToolResults(CanonicalToolResults {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                results: vec![CanonicalToolResultPart {
                    tool_call_id: ToolCallId("call_1".into()),
                    status: CanonicalToolStatus::Ok,
                    content: CanonicalContent::Inline("very long content that should disappear".into()),
                }],
            }),
            CanonicalRecordKind::ToolResultCompacted(CanonicalToolResultCompacted {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                tool_call_id: ToolCallId("call_1".into()),
                preview: "preview only".into(),
                artifact_ref: artifact_ref(),
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t1".into()),
            }),
        ]);

        let ctx = build_canonical_context(&records).unwrap();
        let messages = ProviderAdapter::adapt(&ctx).unwrap();
        let tool_msg = serde_json::to_value(&messages[2]).unwrap();
        assert_eq!(
            tool_msg.get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );
        assert_eq!(tool_msg.get("content").and_then(|v| v.as_str()), Some("preview only"));
    }

    #[test]
    fn canonical_context_interactive_compact_replaces_old_messages_for_provider() {
        let records = make_records(vec![
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t1".into()),
                user_message_id: CanonicalMessageId("m1".into()),
                content: "old task".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t1".into()),
                response_id: ResponseId("r1".into()),
                message_id: CanonicalMessageId("m2".into()),
                parts: vec![CanonicalAssistantPart::Text {
                    content: "old answer".into(),
                }],
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t1".into()),
            }),
            CanonicalRecordKind::CompactReplacement(CanonicalCompactReplacement {
                source: CanonicalSpan {
                    start_seq: 0,
                    end_seq_exclusive: 3,
                },
                content: "compressed summary".into(),
                evidence_pack_ref: artifact_ref(),
            }),
            CanonicalRecordKind::TurnStarted(CanonicalTurnStarted {
                turn_id: TurnId("t2".into()),
                user_message_id: CanonicalMessageId("m3".into()),
                content: "new task".into(),
            }),
            CanonicalRecordKind::AssistantResponse(CanonicalAssistantResponse {
                turn_id: TurnId("t2".into()),
                response_id: ResponseId("r2".into()),
                message_id: CanonicalMessageId("m4".into()),
                parts: vec![CanonicalAssistantPart::Text {
                    content: "new answer".into(),
                }],
            }),
            CanonicalRecordKind::TurnFinished(CanonicalTurnFinished {
                turn_id: TurnId("t2".into()),
            }),
        ]);

        let ctx = build_canonical_context(&records).unwrap();
        let messages = ProviderAdapter::adapt(&ctx).unwrap();
        let contents: Vec<String> = messages
            .iter()
            .map(|m| {
                serde_json::to_value(m)
                    .unwrap()
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        assert_eq!(contents, vec!["compressed summary", "new task", "new answer"]);
    }
}
