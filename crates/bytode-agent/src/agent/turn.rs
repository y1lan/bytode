use super::{
    Agent, AgentMode, AgentOutput, memory,
    tool_calls::{ToolCallContext, format_args},
};
use crate::agent::context::format_tool_result;
use crate::error::Result;
use crate::llm::LlmOutput;
use crate::session::ToolStatus;
use crate::session::compact::kind_for_tool;
use crate::session::conversation::{
    CanonicalAssistantPart, CanonicalAssistantResponse, CanonicalContent, CanonicalMessageId,
    CanonicalRecord, CanonicalRecordKind, CanonicalToolResultPart, CanonicalToolResults,
    CanonicalToolStatus as CcToolStatus, ResponseId, ToolCallId, TurnId,
};
use crate::tools::ToolResult;
use std::sync::atomic::Ordering;

impl Agent {
    /// Run a turn with SSE streaming — `on_text` is called for each token chunk
    pub async fn run_turn_streaming(
        &mut self,
        user_input: &str,
        mut on_text: impl FnMut(&str),
    ) -> Result<AgentOutput> {
        if self.mode == AgentMode::InteractiveCompact {
            let output = self.run_interactive_compact_turn(user_input).await?;
            on_text(&output.content);
            return Ok(AgentOutput::Text(output.content));
        }

        self.runtime.record_user_message(user_input)?;

        let turn_id = {
            let meta = self.runtime.cc_next_meta()?;
            let tid = TurnId(format!("t-{}", meta.seq));
            self.runtime.cc_append(CanonicalRecord {
                meta,
                kind: CanonicalRecordKind::TurnStarted(
                    crate::session::conversation::CanonicalTurnStarted {
                        turn_id: tid.clone(),
                        user_message_id: CanonicalMessageId(format!("m-{}", meta.seq)),
                        content: user_input.to_string(),
                    },
                ),
            })?;
            tid
        };

        self.memory.add_turn(memory::Turn::new(user_input));
        self.runtime.maybe_micro_compact_for_turn()?;

        let mut consecutive_errors: u32 = 0;
        const MAX_CONSECUTIVE_ERRORS: u32 = 5;

        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                self.runtime
                    .record_system_note(crate::session::SystemNoteKind::Cancelled, "(cancelled)")?;
                return Ok(AgentOutput::Text("(cancelled)".into()));
            }

            let sop_needed = self.context.last_was_failure();
            let mut messages = self.build_provider_messages(sop_needed)?;

            let ctx_used = self.context.estimate_tokens(&messages);
            let ctx_total = self.context.ctx_total();

            if self
                .runtime
                .maybe_micro_compact_for_request(ctx_used, ctx_total)?
                .is_some()
            {
                messages = self.build_provider_messages(sop_needed)?;
            }
            if sop_needed {
                self.context.clear_last_failure();
            }

            let tools = self.registry.to_openai_format();
            let response = self.llm.chat_stream(messages, tools, &mut on_text).await?;
            self.runtime.update_last_model_request_at()?;

            match response {
                LlmOutput::Text(text) => {
                    {
                        let meta = self.runtime.cc_next_meta()?;
                        let msg_id = CanonicalMessageId(format!("m-{}", meta.seq));
                        self.runtime.cc_append(CanonicalRecord {
                            meta,
                            kind: CanonicalRecordKind::AssistantResponse(
                                CanonicalAssistantResponse {
                                    turn_id: turn_id.clone(),
                                    response_id: ResponseId(format!("r-{}", meta.seq)),
                                    message_id: msg_id,
                                    parts: vec![CanonicalAssistantPart::Text {
                                        content: text.clone(),
                                    }],
                                },
                            ),
                        })?;
                    }

                    {
                        let meta = self.runtime.cc_next_meta()?;
                        self.runtime.cc_append(CanonicalRecord {
                            meta,
                            kind: CanonicalRecordKind::TurnFinished(
                                crate::session::conversation::CanonicalTurnFinished {
                                    turn_id: turn_id.clone(),
                                },
                            ),
                        })?;
                    }

                    self.runtime.record_assistant_message(&text)?;
                    if let Some(t) = self.memory.last_turn_mut() {
                        t.assistant_text = Some(text.clone());
                    }
                    return Ok(AgentOutput::Text(text));
                }
                LlmOutput::ToolCalls(calls) => {
                    let (response_id, cc_parts) = {
                        let meta = self.runtime.cc_next_meta()?;
                        let rid = ResponseId(format!("r-{}", meta.seq));
                        let msg_id = CanonicalMessageId(format!("m-{}", meta.seq));
                        let parts: Vec<CanonicalAssistantPart> = calls
                            .iter()
                            .enumerate()
                            .map(|(i, call)| {
                                let tc_id = if call.id.is_empty() {
                                    ToolCallId(format!("{}-{}-{}", turn_id.0, rid.0, i))
                                } else {
                                    ToolCallId(call.id.clone())
                                };
                                CanonicalAssistantPart::ToolCall {
                                    tool_call_id: tc_id,
                                    name: call.name.clone(),
                                    arguments: call.arguments.clone(),
                                }
                            })
                            .collect();

                        self.runtime.cc_append(CanonicalRecord {
                            meta,
                            kind: CanonicalRecordKind::AssistantResponse(
                                CanonicalAssistantResponse {
                                    turn_id: turn_id.clone(),
                                    response_id: rid.clone(),
                                    message_id: msg_id,
                                    parts: parts.clone(),
                                },
                            ),
                        })?;

                        (rid, parts)
                    };

                    let mut cc_results: Vec<CanonicalToolResultPart> = Vec::new();

                    for (i, call) in calls.iter().enumerate() {
                        let tool_name = call.name.clone();
                        let tool_args = call.arguments.clone();
                        on_text(&format!(
                            "\n  ⟳ {}\n",
                            streamed_tool_summary(&tool_name, &tool_args)
                        ));

                        let call_entry_id = self.runtime.record_tool_call(
                            &tool_name,
                            Some(call.id.clone()),
                            None,
                            tool_args.clone(),
                            None,
                        )?;
                        let outcome = {
                            let engine_context = ToolCallContext {
                                registry: &self.registry,
                                mode: self.mode,
                                profile: &self.profile,
                                forbidden_write_patterns: &self.forbidden_write_patterns,
                                approval_channel: self.approval_channel.clone(),
                            };

                            self.tool_call_engine
                                .execute(&engine_context, &turn_id.0, i, call)
                                .await
                        };
                        on_text(&format!("{}\n", indent_tool_display(&outcome.display)));

                        if outcome.counts_as_error {
                            consecutive_errors += 1;
                            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                                self.record_tool_result(
                                    call_entry_id,
                                    &tool_name,
                                    &outcome.result,
                                    ToolStatus::Error,
                                )?;
                                return Ok(AgentOutput::Text(
                                    "Too many consecutive tool errors. Stopping.".into(),
                                ));
                            }
                        }

                        self.record_tool_result(
                            call_entry_id,
                            &tool_name,
                            &outcome.result,
                            outcome.status,
                        )?;

                        let tc_id = if let Some(part) = cc_parts.get(i) {
                            match part {
                                CanonicalAssistantPart::ToolCall { tool_call_id, .. } => {
                                    tool_call_id.clone()
                                }
                                _ => ToolCallId(format!("fallback-{}", i)),
                            }
                        } else {
                            ToolCallId(format!("fallback-{}", i))
                        };

                        let cc_status = match outcome.status {
                            ToolStatus::Ok => CcToolStatus::Ok,
                            ToolStatus::Error => CcToolStatus::Error,
                            ToolStatus::Cancelled => CcToolStatus::Cancelled,
                        };

                        let cc_content =
                            CanonicalContent::Inline(format_tool_result(&outcome.result));

                        cc_results.push(CanonicalToolResultPart {
                            tool_call_id: tc_id,
                            status: cc_status,
                            content: cc_content,
                        });

                        if let Some(t) = self.memory.last_turn_mut() {
                            t.tool_calls.push(memory::ToolCallRecord {
                                id: call.id.clone(),
                                name: tool_name,
                                arguments: tool_args,
                                result: outcome.result.clone(),
                            });
                        }

                        self.context.update_last_result(&outcome.result);
                    }

                    {
                        let meta = self.runtime.cc_next_meta()?;
                        self.runtime.cc_append(CanonicalRecord {
                            meta,
                            kind: CanonicalRecordKind::ToolResults(CanonicalToolResults {
                                turn_id: turn_id.clone(),
                                response_id,
                                results: cc_results,
                            }),
                        })?;
                    }
                }
            }
        }
    }

    /// Non-streaming fallback
    pub async fn run_turn(&mut self, user_input: &str) -> Result<AgentOutput> {
        self.run_turn_streaming(user_input, |_| {}).await
    }

    /// Record a tool result in the session log, classifying its artifact kind by
    /// tool name (consistent with micro compact classification).
    fn record_tool_result(
        &mut self,
        call_entry_id: crate::session::EntryId,
        tool_name: &str,
        result: &ToolResult,
        status: ToolStatus,
    ) -> Result<()> {
        let content = format_tool_result(result);
        let kind = kind_for_tool(tool_name);
        self.runtime
            .record_tool_result(call_entry_id, None, status, kind, &content)?;
        Ok(())
    }
}

fn streamed_tool_summary(tool_name: &str, tool_args: &serde_json::Value) -> String {
    let args = format_args(tool_name, tool_args);
    if args.trim().is_empty() {
        format!("{tool_name}()")
    } else {
        format!("{tool_name}({args})")
    }
}

fn indent_tool_display(display: &str) -> String {
    display
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
