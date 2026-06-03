pub(crate) mod context;
pub(crate) mod memory;

use crate::error::Result;
use crate::llm::{DeepSeekClient, LlmOutput, ToolCall};
use crate::project::ProjectProfile;
use crate::session::compact::{kind_for_tool, preserve_cutoff};
use crate::session::{
    ArtifactKind, ArtifactRef, EntrySpan, InteractiveCompactEntry, InteractiveCompactOutcome,
    InteractiveCompactResult, SessionEntry, SessionEntryKind, SessionId, SessionRuntime,
    ToolStatus,
};
use crate::tools::{ToolRegistry, ToolResult};
use crate::transcript::{
    AssistantMessage, AssistantPart, HistoryEntry, TextPart, ToolPart, ToolPresentation, ToolState,
    UserMessage,
};
use context::{ContextBuilder, format_tool_result};
use memory::{MemoryLayer, Turn};
use serde_json::Value;
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

/// In-memory state for an active interactive compact session. Not persisted;
/// only the committed `InteractiveCompactEntry` in the main session log is.
struct InteractiveCompactState {
    source_range: EntrySpan,
    evidence_pack_ref: ArtifactRef,
    /// Messages exchanged in the compact session (system prompt + user turns +
    /// assistant responses). Never written to the main session log.
    messages: Vec<async_openai::types::ChatCompletionRequestMessage>,
    /// Latest assistant response — the candidate for `/commit`.
    current_content: String,
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
                LlmOutput::ToolCall(call) => {
                    let tool_name = call.name.clone();
                    let tool_args = call.arguments.clone();

                    let args_summary = format_args(&call.name, &call.arguments);
                    on_text(&format!("\n  ⟳ {}({})\n", tool_name, args_summary));

                    let call_entry_id =
                        self.runtime
                            .record_tool_call(&tool_name, tool_args.clone(), None)?;
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

                    self.record_tool_result(call_entry_id, &tool_name, &result, status)?;

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
            .record_tool_result(call_entry_id, status, kind, &content)?;
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
    /// `InteractiveCompact` mode. Returns a diagnostic error when there is no
    /// eligible range to compact.
    pub fn start_interactive_compact(&mut self) -> Result<()> {
        let entries = self.runtime.replay_entries()?;
        let source_range =
            select_source_range(&entries, self.runtime.policy()).ok_or_else(|| {
                crate::error::BytodeError::Session(
                    "no eligible source range to compact — all old entries are already covered"
                        .into(),
                )
            })?;

        let evidence_pack_ref = self.generate_evidence_pack(&entries, &source_range)?;
        let system_msg = build_compact_system_msg(&entries, &source_range);
        let start_seq = source_range.start_seq;
        let end_seq = source_range.end_seq_exclusive;

        self.compact = Some(InteractiveCompactState {
            source_range,
            evidence_pack_ref,
            messages: vec![system_msg],
            current_content: String::new(),
        });
        self.mode = AgentMode::InteractiveCompact;

        tracing::info!(start_seq, end_seq, "interactive compact started");
        Ok(())
    }

    /// Send user input to the compact session as a modification request.
    /// Returns the LLM's updated compressed content.
    pub async fn run_interactive_compact_turn(
        &mut self,
        input: &str,
    ) -> Result<InteractiveCompactOutput> {
        let state = self.compact.as_mut().ok_or_else(|| {
            crate::error::BytodeError::Session("no active interactive compact session".into())
        })?;

        // The compact session has no project tools — only text responses.
        use crate::llm;
        state.messages.push(llm::build_user_message(&format!(
            "Modification request:\n{input}\n\nRespond with the updated compressed content."
        )));

        let response = self.llm.chat(state.messages.clone(), vec![]).await?;
        let content = match response {
            LlmOutput::Text(t) => t,
            LlmOutput::ToolCall(call) => {
                return Err(crate::error::BytodeError::Session(format!(
                    "compact session produced unexpected tool call: {}",
                    call.name
                )));
            }
        };

        state
            .messages
            .push(llm::build_assistant_text_message(&content));
        state.current_content = content.clone();

        Ok(InteractiveCompactOutput { content })
    }

    /// Commit the latest compact session content to the main session log and
    /// return to `Normal` mode.
    pub fn commit_interactive_compact(&mut self) -> Result<InteractiveCompactEntry> {
        let state = self.compact.take().ok_or_else(|| {
            crate::error::BytodeError::Session("no active interactive compact session".into())
        })?;

        if state.current_content.is_empty() {
            return Err(crate::error::BytodeError::Session(
                "cannot commit: compact session has no content yet".into(),
            ));
        }

        let start_seq = state.source_range.start_seq;
        let end_seq = state.source_range.end_seq_exclusive;
        let ic_entry = InteractiveCompactEntry {
            source_range: state.source_range,
            compact_session_id: self.runtime.session_id().clone(),
            outcome: InteractiveCompactOutcome::Committed,
            result: Some(InteractiveCompactResult {
                content: state.current_content,
                evidence_pack_ref: state.evidence_pack_ref,
            }),
            operation_digest: format!(
                "InteractiveCompact committed: replaced seq {start_seq}-{end_seq}"
            ),
        };

        let meta = self.runtime.store_next_meta();
        let entry = SessionEntry {
            meta,
            kind: SessionEntryKind::InteractiveCompact(ic_entry.clone()),
        };
        self.runtime.commit_entry(entry)?;

        self.mode = AgentMode::Normal;

        let start_seq = ic_entry.source_range.start_seq;
        let end_seq = ic_entry.source_range.end_seq_exclusive;
        tracing::info!(start_seq, end_seq, "interactive compact committed");
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

    /// Generate an evidence pack artifact for the given source range.
    fn generate_evidence_pack(
        &mut self,
        entries: &[SessionEntry],
        source_range: &EntrySpan,
    ) -> Result<ArtifactRef> {
        let mut excerpts: Vec<Value> = Vec::new();
        for entry in entries {
            if entry.meta.seq < source_range.start_seq
                || entry.meta.seq >= source_range.end_seq_exclusive
            {
                continue;
            }
            let (kind_str, excerpt) = entry_excerpt(entry);
            excerpts.push(serde_json::json!({
                "seq": entry.meta.seq,
                "kind": kind_str,
                "excerpt": excerpt,
            }));
        }

        let pack = serde_json::json!({
            "source_session_id": self.runtime.session_id().as_str(),
            "source_range": {
                "start_seq": source_range.start_seq,
                "end_seq_exclusive": source_range.end_seq_exclusive,
            },
            "entry_count": excerpts.len(),
            "entries": excerpts,
        });

        let pack_str = serde_json::to_string_pretty(&pack).unwrap_or_default();
        let meta = self.runtime.store_next_meta();
        self.runtime
            .write_artifact(&meta, ArtifactKind::CompactArchive, &pack_str)
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
}

#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
}

/// Output from one turn of an interactive compact session.
#[derive(Debug, Clone)]
pub struct InteractiveCompactOutput {
    pub content: String,
}

// Interactive compact helpers ------------------------------------------------

/// Select a contiguous source range for interactive compact. Returns `None`
/// when all old entries are already covered by committed compacts or when the
/// only remaining entries are within the recent-turn window.
fn select_source_range(
    entries: &[SessionEntry],
    policy: &crate::session::MicroCompactPolicy,
) -> Option<EntrySpan> {
    if entries.is_empty() {
        return None;
    }

    let mut covered_until: u64 = entries.first().map(|e| e.meta.seq).unwrap_or(0);
    for entry in entries {
        if let SessionEntryKind::InteractiveCompact(ic) = &entry.kind {
            if matches!(ic.outcome, InteractiveCompactOutcome::Committed) {
                covered_until = covered_until.max(ic.source_range.end_seq_exclusive);
            }
        }
    }

    let cutoff_idx = preserve_cutoff(entries, policy);
    let cutoff_seq = if cutoff_idx < entries.len() {
        entries[cutoff_idx].meta.seq
    } else {
        entries
            .last()
            .map(|e| e.meta.seq.saturating_add(1))
            .unwrap_or(0)
    };

    if covered_until >= cutoff_seq {
        return None;
    }

    Some(EntrySpan {
        start_seq: covered_until,
        end_seq_exclusive: cutoff_seq,
    })
}

fn entry_excerpt(entry: &SessionEntry) -> (&'static str, String) {
    match &entry.kind {
        SessionEntryKind::UserMessage(u) => ("UserMessage", truncate_str(&u.content, 400)),
        SessionEntryKind::AssistantMessage(a) => {
            ("AssistantMessage", truncate_str(&a.content, 400))
        }
        SessionEntryKind::ToolCall(c) => (
            "ToolCall",
            format!(
                "{} {}",
                c.tool_name,
                truncate_str(
                    &c.inline_args
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    200
                )
            ),
        ),
        SessionEntryKind::ToolResult(r) => (
            "ToolResult",
            r.preview
                .clone()
                .unwrap_or_else(|| truncate_str(r.inline_content.as_deref().unwrap_or(""), 300)),
        ),
        SessionEntryKind::MicroCompact(mc) => ("MicroCompact", mc.operation_digest.clone()),
        SessionEntryKind::InteractiveCompact(ic) => {
            ("InteractiveCompact", ic.operation_digest.clone())
        }
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push_str("...");
        out
    }
}

fn build_compact_system_msg(
    entries: &[SessionEntry],
    source_range: &EntrySpan,
) -> async_openai::types::ChatCompletionRequestMessage {
    let mut evidence = String::new();
    for entry in entries {
        if entry.meta.seq < source_range.start_seq
            || entry.meta.seq >= source_range.end_seq_exclusive
        {
            continue;
        }
        let (kind, excerpt) = entry_excerpt(entry);
        evidence.push_str(&format!(
            "  [seq {}] {}: {}\n",
            entry.meta.seq, kind, excerpt
        ));
    }

    let prompt = format!(
        r#"You are compressing a range of conversation history. Produce a concise
replacement that preserves all essential facts, decisions, constraints, and
unfinished items from the source range. Rules:

1. Do NOT fabricate facts not present in the evidence.
2. Do NOT judge task completion or declare work finished.
3. Retain: user requests, tool results, file modifications, diagnostics,
   error messages, and unfinished items.
4. Omit: redundant tool output, repeated search results, verbose diffs
   when the outcome is already stated.
5. Output ONLY the compressed content — no preamble, no commentary.

Source range: seq {}-{}

Evidence:
{}"#,
        source_range.start_seq,
        source_range.end_seq_exclusive.saturating_sub(1),
        evidence,
    );

    crate::llm::build_system_message(&prompt)
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
