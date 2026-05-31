pub mod context;
pub mod memory;

use crate::error::Result;
use crate::llm::{DeepSeekClient, LlmOutput, ToolCall};
use crate::project::ProjectProfile;
use crate::tools::{ToolRegistry, ToolResult};
use context::ContextBuilder;
use memory::Turn;
use std::collections::HashSet;

pub struct Agent {
    llm: DeepSeekClient,
    registry: ToolRegistry,
    context: ContextBuilder,
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
        }
    }

    pub async fn run_turn(&mut self, user_input: &str) -> Result<AgentOutput> {
        let turn = Turn::new(user_input);
        self.context.memory.add_turn(turn.clone());

        loop {
            let messages = self.context.build(&turn);
            let tools = self.registry.to_openai_format();

            let response = self.llm.chat(messages, tools).await?;

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

                    let result = self.execute_tool(&call).await?;

                    if let Some(t) = self.context.memory.last_turn_mut() {
                        t.tool_calls.push(memory::ToolCallRecord {
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
}

#[derive(Debug, Clone)]
pub enum AgentOutput {
    Text(String),
}
