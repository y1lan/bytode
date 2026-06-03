pub(crate) mod context;
pub(crate) mod memory;

use crate::error::Result;
use crate::llm::{DeepSeekClient, LlmOutput, ToolCall};
use crate::project::ProjectProfile;
use crate::session::compact::interactive::{
    self, InteractiveCompactState, build_compact_system_msg, commit_to_main_store,
    create_compact_store, generate_evidence_pack, select_source_range,
};
use crate::session::compact::kind_for_tool;
use crate::session::{
    InteractiveCompactEntry, SessionId, SessionRuntime, ToolStatus,
};
use crate::tools::{ToolRegistry, ToolResult};
use crate::transcript::{
    AssistantMessage, AssistantPart, HistoryEntry, TextPart, ToolPart, ToolPresentation, ToolState,
    UserMessage,
};
use context::{ContextBuilder, format_tool_result};
use memory::{MemoryLayer, Turn};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Agent {
    llm: DeepSeekClient,
    registry: ToolRegistry,
    context: ContextBuilder,
    memory: MemoryLayer,
    runtime: SessionRuntime,
    cancelled: Arc<AtomicBool>,
    primary_language: String,
    detected_languages: HashSet<String>,
    base_enabled: HashSet<String>,
    base_disabled: HashSet<String>,
    is_exact: bool,
    mode: AgentMode,
    profile: ProjectProfile,
    compact: Option<InteractiveCompactState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Normal,
    Plan,
    InteractiveCompact,
}

impl Agent {
    pub fn new(
        llm: DeepSeekClient,
        mut registry: ToolRegistry,
        profile: &ProjectProfile,
        enabled: &HashSet<String>,
        disabled: &HashSet<String>,
        is_exact: bool,
        session_id: SessionId,
        session_root: PathBuf,
    ) -> Result<Self> {
        let primary_lang = profile.primary.to_str();
        let detected: HashSet<String> = profile
            .all_languages
            .iter()
            .map(|l| l.to_str().to_string())
            .collect();
        registry.activate_for(primary_lang, &detected, enabled, disabled, is_exact);

        let context = ContextBuilder::new(profile, &registry);
        let runtime = SessionRuntime::open(session_id, session_root)?;

        Ok(Agent {
            llm,
            registry,
            context,
            memory: MemoryLayer::new(),
            runtime,
            cancelled: Arc::new(AtomicBool::new(false)),
            primary_language: primary_lang.to_string(),
            detected_languages: detected,
            base_enabled: enabled.clone(),
            base_disabled: disabled.clone(),
            is_exact,
            mode: AgentMode::Normal,
            profile: profile.clone(),
            compact: None,
        })
    }

    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    pub fn reset_cancelled(&self) {
        self.cancelled.store(false, Ordering::Relaxed);
    }

    /// Run a turn with SSE streaming — `on_text` is called for each token chunk
    pub async fn run_turn_streaming(
        &mut self,
        user_input: &str,
        mut on_text: impl FnMut(&str),
    ) -> Result<AgentOutput> {
        // In InteractiveCompact mode, route to the compact session instead of
        // the main session. The compact session uses non-streaming chat, so
        // we deliver the full response as one chunk via `on_text`.
        if self.mode == AgentMode::InteractiveCompact {
            let output = self.run_interactive_compact_turn(user_input).await?;
            on_text(&output.content);
            return Ok(AgentOutput::Text(output.content));
        }

        // Record the user message in the session log (context source) and in
        // memory (UI transcript).
        self.runtime.record_user_message(user_input)?;
        self.memory.add_turn(Turn::new(user_input));

        // Per-user-turn auto micro compact: ColdResume / TurnInterval / BloatPressure.
        self.runtime.maybe_micro_compact_for_turn()?;

        let mut consecutive_errors: u32 = 0;
        const MAX_CONSECUTIVE_ERRORS: u32 = 5;

        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Ok(AgentOutput::Text("(cancelled)".into()));
            }

            let mut rendered = self.runtime.rendered_context_entries()?;
            let mut messages = self.context.build(&rendered);
            let ctx_used = self.context.ctx_used();
            let ctx_total = self.context.ctx_total();

            // Per-request auto micro compact: ContextPressure only.
            if self
                .runtime
                .maybe_micro_compact_for_request(ctx_used, ctx_total)?
                .is_some()
            {
                // Rebuild context to reflect the compaction.
                rendered = self.runtime.rendered_context_entries()?;
                messages = self.context.build(&rendered);
            }

            let tools = self.registry.to_openai_format();
            let response = self.llm.chat_stream(messages, tools, &mut on_text).await?;
            self.runtime.update_last_model_request_at()?;

            match response {
                LlmOutput::Text(text) => {
                    self.runtime.record_assistant_message(&text)?;
                    if let Some(t) = self.memory.last_turn_mut() {
                        t.assistant_text = Some(text.clone());
                    }
                    return Ok(AgentOutput::Text(text));
                }
                LlmOutput::ToolCalls(calls) => {
                    let tool_call_group_id = Some(format!("tcg-{}", uuid_like_group_id(&calls)));
                    for call in calls {
                        let tool_name = call.name.clone();
                        let tool_args = call.arguments.clone();

                        let args_summary = format_args(&call.name, &call.arguments);
                        on_text(&format!("\n  ⟳ {}({})\n", tool_name, args_summary));

                        let call_entry_id =
                            self.runtime
                                .record_tool_call(
                                    &tool_name,
                                    Some(call.id.clone()),
                                    tool_call_group_id.clone(),
                                    tool_args.clone(),
                                    None,
                                )?;
                        let tool_ref = self.registry.find(&call.name);

                        let (result, status) = match self.execute_tool(&call).await {
                            Ok(r) => {
                                if let Some(tool) = tool_ref {
                                    if let Some(display) = tool.format_result_for_display(&r) {
                                        on_text(&format!("{}\n", display));
                                    } else {
                                        on_text("  ok\n");
                                    }
                                }
                                (r, ToolStatus::Ok)
                            }
                            Err(e) => {
                                let msg = format!("Error: {}", e);
                                tracing::error!(tool = %call.name, args = %call.arguments, error = %e, "Tool call failed");
                                on_text(&format!("{}\n", msg));
                                consecutive_errors += 1;
                                let result = ToolResult::Text {
                                    source: "tool_error".into(),
                                    content: msg,
                                    truncated: false,
                                };
                                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                                    self.record_tool_result(
                                        call_entry_id,
                                        tool_call_group_id.clone(),
                                        &tool_name,
                                        &result,
                                        ToolStatus::Error,
                                    )?;
                                    return Ok(AgentOutput::Text(
                                        "Too many consecutive tool errors. Stopping.".into(),
                                    ));
                                }
                                (result, ToolStatus::Error)
                            }
                        };

                        self.record_tool_result(
                            call_entry_id,
                            tool_call_group_id.clone(),
                            &tool_name,
                            &result,
                            status,
                        )?;

                        if let Some(t) = self.memory.last_turn_mut() {
                            t.tool_calls.push(memory::ToolCallRecord {
                                id: call.id.clone(),
                                name: tool_name,
                                arguments: tool_args,
                                result: result.clone(),
                            });
                        }

                        self.context.update_last_result(&result);
                    }
                }
            }
        }
    }

    /// Record a tool result in the session log, classifying its artifact kind by
    /// tool name (consistent with micro compact classification).
    fn record_tool_result(
        &mut self,
        call_entry_id: crate::session::EntryId,
        tool_call_group_id: Option<String>,
        tool_name: &str,
        result: &ToolResult,
        status: ToolStatus,
    ) -> Result<()> {
        let content = format_tool_result(result);
        let kind = kind_for_tool(tool_name);
        self.runtime
            .record_tool_result(call_entry_id, tool_call_group_id, status, kind, &content)?;
        Ok(())
    }

    /// Non-streaming fallback
    pub async fn run_turn(&mut self, user_input: &str) -> Result<AgentOutput> {
        self.run_turn_streaming(user_input, |_| {}).await
    }

    // ------------------------------------------------------------------
    // Interactive compact
    // ------------------------------------------------------------------

    /// Select a source range, generate an evidence pack, and enter
    /// compact session, and enter `InteractiveCompact` mode.
    pub fn start_interactive_compact(&mut self) -> Result<()> {
        let entries = self.runtime.replay_entries()?;
        let overlay = crate::session::compact::CompactOverlay::from_entries(&entries);

        let source_range =
            select_source_range(&entries, self.runtime.policy()).ok_or_else(|| {
                crate::error::BytodeError::Session(
                    "no eligible source range to compact".into(),
                )
            })?;

        let evidence_pack_ref = generate_evidence_pack(
            &entries,
            &source_range,
            &overlay,
            self.runtime.artifact_store(),
            self.runtime.session_id(),
        )?;

        let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let compact_store = create_compact_store(&project_root)?;
        let system_msg = build_compact_system_msg(&entries, &source_range, &overlay);
        let start_seq = source_range.start_seq;
        let end_seq = source_range.end_seq_exclusive;

        self.compact = Some(InteractiveCompactState {
            source_range,
            evidence_pack_ref,
            compact_store,
            messages: vec![system_msg],
            current_content: String::new(),
        });
        self.mode = AgentMode::InteractiveCompact;

        tracing::info!(start_seq, end_seq, "interactive compact started");
        Ok(())
    }

    /// Send user input to the compact session as a modification request.
    /// Both the user input and the assistant response are appended to the
    /// compact session's own `log.jsonl`.
    pub async fn run_interactive_compact_turn(
        &mut self,
        input: &str,
    ) -> Result<interactive::InteractiveCompactOutput> {
        let state = self.compact.as_mut().ok_or_else(|| {
            crate::error::BytodeError::Session("no active interactive compact session".into())
        })?;

        // Append user message to the compact session log.
        {
            let meta = state.compact_store.next_meta();
            state.compact_store.commit(crate::session::SessionEntry {
                meta,
                kind: crate::session::SessionEntryKind::UserMessage(
                    crate::session::UserMessageEntry {
                        content: input.to_string(),
                    },
                ),
            })?;
        }

        use crate::llm;
        state.messages.push(llm::build_user_message(&format!(
            "Modification request:\n{input}\n\nRespond with the updated compressed content."
        )));

        let response = self.llm.chat(state.messages.clone(), vec![]).await?;
        let content = match response {
            LlmOutput::Text(t) => t,
            LlmOutput::ToolCalls(calls) => {
                let names = calls
                    .into_iter()
                    .map(|call| call.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(crate::error::BytodeError::Session(format!(
                    "compact session produced unexpected tool call(s): {}",
                    names
                )));
            }
        };

        state
            .messages
            .push(llm::build_assistant_text_message(&content));
        state.current_content = content.clone();

        // Append assistant response to the compact session log.
        {
            let meta = state.compact_store.next_meta();
            state.compact_store.commit(crate::session::SessionEntry {
                meta,
                kind: crate::session::SessionEntryKind::AssistantMessage(
                    crate::session::AssistantMessageEntry {
                        content: content.clone(),
                    },
                ),
            })?;
        }

        Ok(interactive::InteractiveCompactOutput { content })
    }

    /// Commit the latest compact session content to the main session log.
    pub fn commit_interactive_compact(&mut self) -> Result<InteractiveCompactEntry> {
        let state = self.compact.take().ok_or_else(|| {
            crate::error::BytodeError::Session("no active interactive compact session".into())
        })?;

        if state.current_content.is_empty() {
            return Err(crate::error::BytodeError::Session(
                "cannot commit: compact session has no content yet".into(),
            ));
        }

        let ic_entry = commit_to_main_store(&mut self.runtime.store_mut(), &state)?;

        self.mode = AgentMode::Normal;
        tracing::info!(
            start_seq = ic_entry.source_range.start_seq,
            end_seq = ic_entry.source_range.end_seq_exclusive,
            "interactive compact committed"
        );
        Ok(ic_entry)
    }

    /// Abort the interactive compact session without changing the main session.
    pub fn abort_interactive_compact(&mut self) -> Result<()> {
        if self.compact.is_none() {
            return Err(crate::error::BytodeError::Session(
                "no active interactive compact session".into(),
            ));
        }
        self.compact = None;
        self.mode = AgentMode::Normal;
        tracing::info!("interactive compact aborted");
        Ok(())
    }

    /// Whether the agent is currently in an interactive compact session.
    pub fn is_in_interactive_compact(&self) -> bool {
        self.mode == AgentMode::InteractiveCompact
    }

    async fn execute_tool(&self, call: &ToolCall) -> Result<ToolResult> {
        let tool =
            self.registry
                .find(&call.name)
                .ok_or_else(|| crate::error::BytodeError::Tool {
                    tool: call.name.clone(),
                    message: "unknown tool".into(),
                })?;

        let timeout = tokio::time::Duration::from_millis(tool.timeout_ms());
        let result = tokio::time::timeout(timeout, tool.execute(call.arguments.clone()))
            .await
            .map_err(|_| crate::error::BytodeError::Tool {
                tool: call.name.clone(),
                message: "timeout".into(),
            })??;

        Ok(result)
    }

    pub fn context_used(&self) -> u64 {
        self.context.ctx_used()
    }
    pub fn context_total(&self) -> u64 {
        self.context.ctx_total()
    }
    pub fn tool_names(&self) -> Vec<String> {
        self.registry.active_names()
    }
    pub fn session_input_tokens(&self) -> u64 {
        self.llm.session_input_tokens()
    }
    pub fn session_output_tokens(&self) -> u64 {
        self.llm.session_output_tokens()
    }
    pub fn session_cost(&self) -> f64 {
        self.llm.session_cost()
    }
    pub fn session_call_count(&self) -> u64 {
        self.llm.session_call_count()
    }

    pub fn model_name(&self) -> &str {
        self.llm.model_name()
    }

    pub fn set_model(&mut self, model: String) {
        self.llm.set_model(model);
    }

    pub fn mode(&self) -> AgentMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: AgentMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;

        let mut plan_disabled: HashSet<String> = self.base_disabled.clone();
        if mode == AgentMode::Plan {
            plan_disabled.insert("write_file".into());
            plan_disabled.insert("git_status".into());
            plan_disabled.insert("git_diff".into());
            plan_disabled.insert("git_log".into());
            plan_disabled.insert("git_commit".into());
            plan_disabled.insert("git_push".into());
        }

        self.registry.activate_for(
            &self.primary_language,
            &self.detected_languages,
            &self.base_enabled,
            &plan_disabled,
            self.is_exact,
        );

        self.context
            .rebuild_core_prompt(&self.profile, &self.registry);
    }

    pub fn chat_history_entries(&self) -> Vec<HistoryEntry> {
        let mut entries = Vec::new();

        for turn in self.memory.recent_turns() {
            if let Some(input) = &turn.user_input {
                entries.push(HistoryEntry::User(UserMessage {
                    body: input.clone(),
                    meta: None,
                }));
            }

            let mut parts = turn
                .tool_calls
                .iter()
                .map(history_tool_part)
                .map(AssistantPart::Tool)
                .collect::<Vec<_>>();

            if let Some(text) = &turn.assistant_text
                && !text.trim().is_empty()
            {
                parts.push(AssistantPart::Text(TextPart {
                    content: text.clone(),
                }));
            }

            if !parts.is_empty() {
                entries.push(HistoryEntry::Assistant(AssistantMessage { parts }));
            }
        }

        entries
    }

    /// Replay the session log and convert it into `HistoryEntry` items for UI
    /// display. Called on startup to restore the conversation view. Audit
    /// entries (MicroCompact, InteractiveCompact) are skipped.
    pub fn replay_chat_history(&self) -> Result<Vec<HistoryEntry>> {
        let entries = self.runtime.replay_entries()?;
        let mut history = Vec::new();
        let mut pending_tool_results: Vec<String> = Vec::new();

        for entry in &entries {
            match &entry.kind {
                crate::session::SessionEntryKind::UserMessage(u) => {
                    // Flush any pending tool results before the next user message.
                    flush_tool_results(&mut history, &mut pending_tool_results);
                    history.push(HistoryEntry::User(UserMessage {
                        body: u.content.clone(),
                        meta: None,
                    }));
                }
                crate::session::SessionEntryKind::AssistantMessage(a) => {
                    flush_tool_results(&mut history, &mut pending_tool_results);
                    history.push(HistoryEntry::Assistant(AssistantMessage::from_text(
                        a.content.clone(),
                    )));
                }
                crate::session::SessionEntryKind::ToolCall(tc) => {
                    let summary = format!("{} (args archived)", tc.tool_name);
                    let body = tc
                        .inline_args
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "(archived)".into());
                    pending_tool_results.push(format!("{summary}\n{body}"));
                }
                crate::session::SessionEntryKind::ToolResult(tr) => {
                    let preview = tr
                        .preview
                        .clone()
                        .or_else(|| tr.inline_content.clone())
                        .unwrap_or_else(|| "(result archived)".into());
                    // Attach to the most recent pending tool call.
                    if let Some(last) = pending_tool_results.last_mut() {
                        last.push_str(&format!("\n→ {preview}"));
                    } else {
                        pending_tool_results.push(format!("tool result: {preview}"));
                    }
                }
                // Audit-only entries — not shown in conversation history.
                crate::session::SessionEntryKind::MicroCompact(_)
                | crate::session::SessionEntryKind::InteractiveCompact(_) => {}
            }
        }
        flush_tool_results(&mut history, &mut pending_tool_results);

        Ok(history)
    }
}

fn flush_tool_results(history: &mut Vec<HistoryEntry>, pending: &mut Vec<String>) {
    if pending.is_empty() {
        return;
    }
    let parts: Vec<AssistantPart> = std::mem::take(pending)
        .into_iter()
        .map(|body| {
            AssistantPart::Tool(ToolPart {
                name: String::new(),
                summary: String::new(),
                body: Some(body),
                state: crate::transcript::ToolState::Completed,
                presentation: crate::transcript::ToolPresentation::Block,
                collapsed: true,
            })
        })
        .collect();
    history.push(HistoryEntry::Assistant(AssistantMessage { parts }));
}

#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
}

fn format_args(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "read_file" => {
            let path = args["path"].as_str().unwrap_or("?");
            let offset = args["offset"]
                .as_u64()
                .map(|o| format!(", offset={o}"))
                .unwrap_or_default();
            let limit = args["limit"]
                .as_u64()
                .map(|l| format!(", limit={l}"))
                .unwrap_or_default();
            format!("{path}{offset}{limit}")
        }
        "write_file" => args["path"].as_str().unwrap_or("?").to_string(),
        "search_code" => {
            let pat = args["pattern"].as_str().unwrap_or("?");
            if let Some(p) = args["path"].as_str() {
                format!("pattern=\"{pat}\", path={p}")
            } else {
                format!("pattern=\"{pat}\"")
            }
        }
        "get_diagnostics" => {
            let mut parts = Vec::new();
            if let Some(p) = args["path"].as_str() {
                parts.push(format!("path={p}"));
            }
            if let Some(f) = args["filter"].as_str() {
                parts.push(format!("filter={f}"));
            }
            if parts.is_empty() {
                "?".into()
            } else {
                parts.join(", ")
            }
        }
        "cargo" => {
            let cmd = args["cmd"].as_str().unwrap_or("?");
            if let Some(extra) = args["args"].as_array() {
                let ex: Vec<&str> = extra.iter().filter_map(|v| v.as_str()).collect();
                if ex.is_empty() {
                    cmd.to_string()
                } else {
                    format!("{cmd} {}", ex.join(" "))
                }
            } else {
                cmd.to_string()
            }
        }
        "cargo_check" => {
            if let Some(e) = args["extra_args"].as_array() {
                let ex: Vec<&str> = e.iter().filter_map(|v| v.as_str()).collect();
                if ex.is_empty() {
                    "?".into()
                } else {
                    ex.join(" ")
                }
            } else {
                "?".into()
            }
        }
        "git_status" => args["path"].as_str().unwrap_or("").to_string(),
        "git_diff" => {
            let mut parts = Vec::new();
            if args["staged"].as_bool().unwrap_or(false) {
                parts.push("staged");
            }
            if let Some(p) = args["path"].as_str() {
                parts.push(p);
            }
            parts.join(", ")
        }
        "git_log" => {
            let count = args["count"].as_u64().unwrap_or(10);
            if let Some(p) = args["path"].as_str() {
                format!("count={count}, path={p}")
            } else {
                format!("count={count}")
            }
        }
        "web_search" => {
            let q = args["query"].as_str().unwrap_or("?");
            if q.len() > 60 {
                format!("\"{}...\"", &q[..57])
            } else {
                format!("\"{q}\"")
            }
        }
        "git_commit" => {
            let msg = args["message"].as_str().unwrap_or("?");
            if msg.len() > 50 {
                format!("\"{}...\"", &msg[..47])
            } else {
                format!("\"{msg}\"")
            }
        }
        "git_push" => {
            let mut parts = Vec::new();
            if let Some(r) = args["remote"].as_str().filter(|r| *r != "origin") {
                parts.push(format!("remote={r}"));
            }
            if let Some(b) = args["branch"].as_str() {
                parts.push(format!("branch={b}"));
            }
            if args["force"].as_bool().unwrap_or(false) {
                parts.push("force".into());
            }
            if parts.is_empty() {
                "origin".into()
            } else {
                parts.join(", ")
            }
        }
        _ => "?".into(),
    }
}

fn uuid_like_group_id(calls: &[crate::llm::ToolCall]) -> String {
    calls.first()
        .map(|call| call.id.replace("call_", ""))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "group".into())
}

fn history_tool_part(record: &memory::ToolCallRecord) -> ToolPart {
    let summary = format!(
        "{} {}",
        record.name,
        format_args(&record.name, &record.arguments)
    )
    .trim()
    .to_string();
    let state = if record.is_error() {
        ToolState::Failed
    } else {
        ToolState::Completed
    };
    let body = history_tool_body(&record.result);
    let presentation = history_tool_presentation(&record.name, body.as_deref());
    let collapsed = body
        .as_deref()
        .map(|content| content.lines().count() > 6)
        .unwrap_or(false);

    ToolPart {
        name: record.name.clone(),
        summary,
        body,
        state,
        presentation,
        collapsed,
    }
}

fn history_tool_body(result: &ToolResult) -> Option<String> {
    match result {
        ToolResult::FileContent {
            path,
            content,
            line_count,
            ..
        } => Some(format!("{}\n{} lines\n{}", path, line_count, content)),
        ToolResult::Diagnostics {
            total,
            errors,
            warnings,
            list,
            ..
        } => {
            let mut lines = vec![format!(
                "diagnostics: total={} errors={} warnings={}",
                total, errors, warnings
            )];
            lines.extend(list.iter().take(8).map(|item| {
                format!(
                    "{}:{} {} {}",
                    item.file, item.line, item.severity, item.message
                )
            }));
            Some(lines.join("\n"))
        }
        ToolResult::Json {
            tool, count, data, ..
        } => {
            let preview = data
                .iter()
                .take(6)
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            Some(format!("{tool}: {count} item(s)\n{preview}"))
        }
        ToolResult::Matches {
            pattern,
            count,
            items,
            truncated,
        } => {
            let mut lines = vec![format!("pattern={pattern} count={count}")];
            lines.extend(
                items
                    .iter()
                    .take(8)
                    .map(|item| format!("{}:{} {}", item.file, item.line, item.text)),
            );
            if *truncated {
                lines.push("...".into());
            }
            Some(lines.join("\n"))
        }
        ToolResult::WriteConfirmation {
            path,
            bytes_written,
            lines,
            diff,
        } => Some(format!(
            "{}\n{} bytes, {} lines\n{}",
            path, bytes_written, lines, diff
        )),
        ToolResult::Text { content, .. } => Some(content.clone()),
    }
}

fn history_tool_presentation(name: &str, body: Option<&str>) -> ToolPresentation {
    if matches!(
        name,
        "write_file" | "cargo" | "cargo_check" | "get_diagnostics"
    ) {
        return ToolPresentation::Block;
    }

    let Some(body) = body else {
        return ToolPresentation::Inline;
    };

    if body.contains("```")
        || body.lines().count() > 4
        || body
            .lines()
            .any(|line| line.starts_with('+') || line.starts_with('-') || line.contains(" | "))
        || body.len() > 160
    {
        ToolPresentation::Block
    } else {
        ToolPresentation::Inline
    }
}
