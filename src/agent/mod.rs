pub mod context;
pub mod memory;

use crate::error::Result;
use crate::llm::{DeepSeekClient, LlmOutput, ToolCall};
use crate::project::ProjectProfile;
use crate::tools::{ToolRegistry, ToolResult};
use context::ContextBuilder;
use memory::{MemoryLayer, SessionData, Turn};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct Agent {
    llm: DeepSeekClient,
    registry: ToolRegistry,
    context: ContextBuilder,
    cancelled: Arc<AtomicBool>,
    primary_language: String,
    detected_languages: HashSet<String>,
    base_enabled: HashSet<String>,
    base_disabled: HashSet<String>,
    is_exact: bool,
    mode: AgentMode,
    profile: ProjectProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Normal,
    Plan,
}

impl Agent {
    pub fn new(
        llm: DeepSeekClient,
        mut registry: ToolRegistry,
        profile: &ProjectProfile,
        enabled: &HashSet<String>,
        disabled: &HashSet<String>,
        is_exact: bool,
    ) -> Self {
        let primary_lang = profile.primary.to_str();
        let detected: HashSet<String> = profile
            .all_languages
            .iter()
            .map(|l| l.to_str().to_string())
            .collect();
        registry.activate_for(primary_lang, &detected, enabled, disabled, is_exact);

        let context = ContextBuilder::new(profile, &registry);

        Agent {
            llm,
            registry,
            context,
            cancelled: Arc::new(AtomicBool::new(false)),
            primary_language: primary_lang.to_string(),
            detected_languages: detected,
            base_enabled: enabled.clone(),
            base_disabled: disabled.clone(),
            is_exact,
            mode: AgentMode::Normal,
            profile: profile.clone(),
        }
    }

    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    /// Run a turn with SSE streaming — `on_text` is called for each token chunk
    pub async fn run_turn_streaming(
        &mut self,
        user_input: &str,
        mut on_text: impl FnMut(&str),
    ) -> Result<AgentOutput> {
        let turn = Turn::new(user_input);
        self.context.memory.add_turn(turn.clone());
        let mut consecutive_errors: u32 = 0;
        const MAX_CONSECUTIVE_ERRORS: u32 = 5;

        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Ok(AgentOutput::Text("(cancelled)".into()));
            }

            let messages = self.context.build(&turn);
            let tools = self.registry.to_openai_format();

            let response = self
                .llm
                .chat_stream(messages, tools, &mut on_text)
                .await?;

            match response {
                LlmOutput::Text(text) => {
                    if let Some(t) = self.context.memory.last_turn_mut() {
                        t.assistant_text = Some(text.clone());
                    }
                    return Ok(AgentOutput::Text(text));
                }
                LlmOutput::ToolCall(call) => {
                    let tool_name = call.name.clone();
                    let tool_args = call.arguments.clone();

                    let args_summary = format_args(&call.name, &call.arguments);
                    on_text(&format!("\n  ⟳ {}({})\n", tool_name, args_summary));

                    let tool_ref = self.registry.find(&call.name);

                    let result = match self.execute_tool(&call).await {
                        Ok(r) => {
                            if let Some(tool) = tool_ref {
                                if let Some(display) = tool.format_result_for_display(&r) {
                                    on_text(&format!("{}\n", display));
                                } else {
                                    on_text("  ok\n");
                                }
                            }
                            r
                        }
                        Err(e) => {
                            let msg = format!("Error: {}", e);
                            tracing::error!(tool = %call.name, args = %call.arguments, error = %e, "Tool call failed");
                            on_text(&format!("{}\n", msg));
                            consecutive_errors += 1;
                            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                                return Ok(AgentOutput::Text(
                                    "Too many consecutive tool errors. Stopping.".into(),
                                ));
                            }
                            ToolResult::Text {
                                source: "tool_error".into(),
                                content: msg,
                                truncated: false,
                            }
                        }
                    };

                    if let Some(t) = self.context.memory.last_turn_mut() {
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

    /// Non-streaming fallback
    pub async fn run_turn(&mut self, user_input: &str) -> Result<AgentOutput> {
        self.run_turn_streaming(user_input, |_| {}).await
    }

    async fn execute_tool(&self, call: &ToolCall) -> Result<ToolResult> {
        let tool = self.registry.find(&call.name).ok_or_else(|| {
            crate::error::BytodeError::Tool {
                tool: call.name.clone(),
                message: "unknown tool".into(),
            }
        })?;

        let timeout = tokio::time::Duration::from_millis(tool.timeout_ms());
        let result =
            tokio::time::timeout(timeout, tool.execute(call.arguments.clone()))
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
        }

        self.registry.activate_for(
            &self.primary_language,
            &self.detected_languages,
            &self.base_enabled,
            &plan_disabled,
            self.is_exact,
        );

        self.context.rebuild_core_prompt(&self.profile, &self.registry);
    }

    pub fn save_session(&self, path: &Path, project_path: &str) -> Result<()> {
        let mut data = self.context.memory.session_data();
        data.project_path = Some(project_path.to_string());
        let json = serde_json::to_string_pretty(&data).map_err(|e| {
            crate::error::BytodeError::Json(e)
        })?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load_session(&mut self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)?;
        let data: SessionData = serde_json::from_str(&content).map_err(|e| {
            crate::error::BytodeError::Json(e)
        })?;
        self.context.memory = MemoryLayer::from_session(data, 10);
        Ok(())
    }

    /// Reconstruct chat history text from loaded turns for UI display
    pub fn chat_history_text(&self) -> Vec<String> {
        self.context.memory.recent_turns().iter().filter_map(|t| {
            let mut entries = Vec::new();
            if let Some(ref input) = t.user_input {
                entries.push(format!("\u{25b8} {}", input));
            }
            if let Some(ref text) = t.assistant_text
                && !text.is_empty() {
                    entries.push(text.clone());
                }
            if entries.is_empty() { None } else { Some(entries.join("\n")) }
        }).collect()
    }
}

#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
}

fn format_args(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "read_file" => {
            let path = args["path"].as_str().unwrap_or("?");
            let offset = args["offset"].as_u64().map(|o| format!(", offset={o}")).unwrap_or_default();
            let limit = args["limit"].as_u64().map(|l| format!(", limit={l}")).unwrap_or_default();
            format!("{path}{offset}{limit}")
        }
        "write_file" => {
            args["path"].as_str().unwrap_or("?").to_string()
        }
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
            if let Some(p) = args["path"].as_str() { parts.push(format!("path={p}")); }
            if let Some(f) = args["filter"].as_str() { parts.push(format!("filter={f}")); }
            if parts.is_empty() { "?".into() } else { parts.join(", ") }
        }
        "run_cargo" => {
            let cmd = args["cmd"].as_str().unwrap_or("?");
            if let Some(extra) = args["args"].as_array() {
                let ex: Vec<&str> = extra.iter().filter_map(|v| v.as_str()).collect();
                if ex.is_empty() { cmd.to_string() } else { format!("{cmd} {}", ex.join(" ")) }
            } else {
                cmd.to_string()
            }
        }
        "run_check" => {
            if let Some(e) = args["extra_args"].as_array() {
                let ex: Vec<&str> = e.iter().filter_map(|v| v.as_str()).collect();
                if ex.is_empty() { "?".into() } else { ex.join(" ") }
            } else {
                "?".into()
            }
        }
        "git_status" => args["path"].as_str().unwrap_or("").to_string(),
        "git_diff" => {
            let mut parts = Vec::new();
            if args["staged"].as_bool().unwrap_or(false) { parts.push("staged"); }
            if let Some(p) = args["path"].as_str() { parts.push(p); }
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
            if q.len() > 60 { format!("\"{}...\"", &q[..57]) } else { format!("\"{q}\"") }
        }
        _ => "?".into(),
    }
}
