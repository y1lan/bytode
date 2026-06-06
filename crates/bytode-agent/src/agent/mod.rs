pub(crate) mod audit;
mod compact;
pub(crate) mod context;
mod history;
pub(crate) mod memory;
pub(crate) mod provider_adapter;
mod tool_calls;
pub(crate) mod tool_runtime;
mod turn;

pub use tool_calls::{ApprovalEvent, InteractiveApprovalChannel};

use crate::error::Result;
use crate::llm::DeepSeekClient;
use crate::project::ProjectProfile;
use crate::session::compact::interactive::InteractiveCompactState;
use crate::session::{SessionId, SessionRuntime};
use crate::tools::ToolRegistry;
use context::ContextBuilder;
use memory::MemoryLayer;
use provider_adapter::ProviderAdapter;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tool_calls::{ApprovalChannel, ToolCallEngine};

pub struct Agent {
    llm: DeepSeekClient,
    registry: ToolRegistry,
    context: ContextBuilder,
    memory: MemoryLayer,
    runtime: SessionRuntime,
    tool_call_engine: ToolCallEngine,
    approval_channel: Option<Arc<dyn ApprovalChannel>>,
    cancelled: Arc<AtomicBool>,
    forbidden_write_patterns: Vec<String>,
    primary_language: String,
    detected_languages: HashSet<String>,
    base_enabled: HashSet<String>,
    base_disabled: HashSet<String>,
    is_exact: bool,
    mode: AgentMode,
    profile: ProjectProfile,
    compact: Option<InteractiveCompactState>,
}

pub struct AgentInit<'a> {
    pub enabled: &'a HashSet<String>,
    pub disabled: &'a HashSet<String>,
    pub is_exact: bool,
    pub session_id: SessionId,
    pub session_root: PathBuf,
    pub forbidden_write_patterns: Vec<String>,
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
        init: AgentInit<'_>,
    ) -> Result<Self> {
        let primary_lang = profile.primary.to_str();
        let detected: HashSet<String> = profile
            .all_languages
            .iter()
            .map(|l| l.to_str().to_string())
            .collect();
        registry.activate_for(
            primary_lang,
            &detected,
            init.enabled,
            init.disabled,
            init.is_exact,
        );

        let context = ContextBuilder::new(profile, &registry);
        let tool_call_engine =
            ToolCallEngine::new(init.session_id.as_str(), init.session_root.clone())?;
        let runtime = SessionRuntime::open(init.session_id, init.session_root)?;

        Ok(Agent {
            llm,
            registry,
            context,
            memory: MemoryLayer::new(),
            runtime,
            tool_call_engine,
            approval_channel: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            forbidden_write_patterns: init.forbidden_write_patterns,
            primary_language: primary_lang.to_string(),
            detected_languages: detected,
            base_enabled: init.enabled.clone(),
            base_disabled: init.disabled.clone(),
            is_exact: init.is_exact,
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

    pub fn set_approval_channel(&mut self, channel: Arc<dyn ApprovalChannel>) {
        self.approval_channel = Some(channel);
    }

    /// Persist an out-of-band interaction note (slash command, error) so it is
    /// restored in the UI history after a restart.
    pub fn record_system_note(
        &mut self,
        kind: crate::session::SystemNoteKind,
        content: &str,
    ) -> Result<()> {
        self.runtime.record_system_note(kind, content)?;
        Ok(())
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

    fn build_provider_messages(
        &self,
        sop_needed: bool,
    ) -> Result<Vec<async_openai::types::ChatCompletionRequestMessage>> {
        let cc = self.runtime.canonical_context()?;
        let mut messages = ProviderAdapter::adapt(&cc)?;
        messages.insert(
            0,
            crate::llm::build_system_message(self.context.core_prompt()),
        );
        if sop_needed {
            messages.push(crate::llm::build_system_message(self.context.sop_prompt()));
        }
        Ok(messages)
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
}

#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
}
