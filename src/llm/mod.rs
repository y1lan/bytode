use crate::error::{BytodeError, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    },
    Client,
};
use futures::StreamExt;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

/// Token usage from a single API call
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

pub struct DeepSeekClient {
    client: Client<OpenAIConfig>,
    model: String,
    max_tokens: u32,
    // Cumulative session counters
    session_input: AtomicU64,
    session_output: AtomicU64,
    session_calls: AtomicU64,
}

#[derive(Debug, Clone)]
pub enum LlmOutput {
    Text(String),
    ToolCall(ToolCall),
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn info(&self) -> String {
        format!("{}()", self.name)
    }
}

impl DeepSeekClient {
    pub fn new(api_key: String, model: String, base_url: Option<String>) -> Result<Self> {
        let config = OpenAIConfig::default()
            .with_api_key(api_key)
            .with_api_base(
                base_url.unwrap_or_else(|| "https://api.deepseek.com/v1".into()),
            );

        Ok(DeepSeekClient {
            client: Client::with_config(config),
            model,
            max_tokens: 8192,
            session_input: AtomicU64::new(0),
            session_output: AtomicU64::new(0),
            session_calls: AtomicU64::new(0),
        })
    }

    /// Streaming chat with token usage tracking.
    pub async fn chat_stream(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<Value>,
        mut on_text: impl FnMut(&str),
    ) -> Result<LlmOutput> {
        let tools_serialized = serde_json::to_value(&tools).unwrap_or_default();
        let openai_tools: Vec<async_openai::types::ChatCompletionTool> =
            serde_json::from_value(tools_serialized).unwrap_or_default();

        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .messages(messages)
            .tools(openai_tools)
            .max_tokens(self.max_tokens)
            .temperature(0.0_f32)
            .build()
            .map_err(|e| BytodeError::Llm(format!("build request: {}", e)))?;

        let mut stream = self
            .client
            .chat()
            .create_stream(request)
            .await
            .map_err(|e| BytodeError::Llm(format!("stream error: {}", e)))?;

        let mut full_text = String::new();
        let mut tool_call_id = String::new();
        let mut tool_call_name = String::new();
        let mut tool_call_args = String::new();
        let mut usage = TokenUsage::default();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| BytodeError::Llm(format!("chunk error: {}", e)))?;

            // Capture usage from the last chunk (DeepSeek includes it)
            if let Some(ref u) = chunk.usage {
                usage.input_tokens = u.prompt_tokens as u64;
                usage.output_tokens = u.completion_tokens as u64;
                usage.total_tokens = u.total_tokens as u64;
            }

            for choice in &chunk.choices {
                if let Some(ref content) = choice.delta.content {
                    on_text(content);
                    full_text.push_str(content);
                }

                if let Some(ref tc_deltas) = choice.delta.tool_calls {
                    for tc in tc_deltas {
                        if let Some(ref id) = tc.id {
                            tool_call_id = id.clone();
                        }
                        if let Some(ref func) = tc.function {
                            if let Some(ref name) = func.name {
                                tool_call_name = name.clone();
                            }
                            if let Some(ref args) = func.arguments {
                                tracing::debug!("tool_call_args chunk: {:?}", args);
                                tool_call_args.push_str(args);
                            } else {
                                tracing::error!("tool_call no arguments field in chunk");
                            }
                        }
                    }
                }
            }
        }

        // Update session counters
        self.session_calls.fetch_add(1, Ordering::Relaxed);
        self.session_input
            .fetch_add(usage.input_tokens, Ordering::Relaxed);
        self.session_output
            .fetch_add(usage.output_tokens, Ordering::Relaxed);

        // Log this call's usage
        let cost_estimate = estimate_cost(usage.input_tokens, usage.output_tokens);
        tracing::info!(
            "tokens: in={} out={} total={} cost≈${:.4} | session: in={} out={} calls={} total≈${:.4}",
            usage.input_tokens,
            usage.output_tokens,
            usage.total_tokens,
            cost_estimate,
            self.session_input.load(Ordering::Relaxed),
            self.session_output.load(Ordering::Relaxed),
            self.session_calls.load(Ordering::Relaxed),
            self.session_cost(),
        );

        if !tool_call_name.is_empty() {
            let arguments: Value = if tool_call_args.is_empty() {
                tracing::error!("tool_call '{}' has empty arguments", tool_call_name);
                Value::Null
            } else {
                match serde_json::from_str(&tool_call_args) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(
                            "tool_call '{}' args parse error: {} raw={:?}",
                            tool_call_name, e, tool_call_args
                        );
                        // Try to extract all JSON objects — LLM sometimes concatenates multiple calls
                        // Pick the one with the most keys (most complete)
                        if e.to_string().contains("trailing characters") {
                            let mut objs: Vec<Value> = Vec::new();
                            let mut pos = 0;
                            let bytes = tool_call_args.as_bytes();
                            while pos < bytes.len() {
                                // Skip non-{ chars
                                while pos < bytes.len() && bytes[pos] != b'{' {
                                    pos += 1;
                                }
                                if pos >= bytes.len() {
                                    break;
                                }
                                let mut depth = 0u32;
                                let start = pos;
                                while pos < bytes.len() {
                                    match bytes[pos] {
                                        b'{' => depth += 1,
                                        b'}' => {
                                            depth -= 1;
                                            if depth == 0 {
                                                let slice = &tool_call_args[start..=pos];
                                                if let Ok(v) = serde_json::from_str::<Value>(slice) {
                                                    objs.push(v);
                                                }
                                                pos += 1;
                                                break;
                                            }
                                        }
                                        _ => {}
                                    }
                                    pos += 1;
                                }
                            }
                            if !objs.is_empty() {
                                // Pick the object with the most keys (most complete)
                                objs.sort_by(|a, b| {
                                    let ka = a.as_object().map(|o| o.len()).unwrap_or(0);
                                    let kb = b.as_object().map(|o| o.len()).unwrap_or(0);
                                    kb.cmp(&ka)
                                });
                                let best = objs.into_iter().next().unwrap();
                                tracing::info!(
                                    "tool_call '{}' extracted best args from concatenated JSON: {:?}",
                                    tool_call_name, best
                                );
                                return Ok(LlmOutput::ToolCall(ToolCall {
                                    id: tool_call_id,
                                    name: tool_call_name,
                                    arguments: best,
                                }));
                            }
                        }
                        Value::Null
                    }
                }
            };

            tracing::info!(
                "tool_call: name={} args={:?}",
                tool_call_name, arguments
            );

            return Ok(LlmOutput::ToolCall(ToolCall {
                id: tool_call_id,
                name: tool_call_name,
                arguments,
            }));
        }

        Ok(LlmOutput::Text(full_text))
    }

    /// Non-streaming fallback
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<Value>,
    ) -> Result<LlmOutput> {
        self.chat_stream(messages, tools, |_| {}).await
    }

    /// Cumulative session input tokens
    pub fn session_input_tokens(&self) -> u64 {
        self.session_input.load(Ordering::Relaxed)
    }

    /// Cumulative session output tokens
    pub fn session_output_tokens(&self) -> u64 {
        self.session_output.load(Ordering::Relaxed)
    }

    /// Cumulative session API call count
    pub fn session_call_count(&self) -> u64 {
        self.session_calls.load(Ordering::Relaxed)
    }

    /// Current model name
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// Switch model at runtime
    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    /// Estimated cumulative session cost in USD
    pub fn session_cost(&self) -> f64 {
        let input = self.session_input.load(Ordering::Relaxed);
        let output = self.session_output.load(Ordering::Relaxed);
        estimate_cost(input, output)
    }
}

/// Estimate cost in USD based on DeepSeek pricing
///   Input:  ~$0.27 / 1M tokens (cache miss)
///   Output: ~$1.10 / 1M tokens
fn estimate_cost(input_tokens: u64, output_tokens: u64) -> f64 {
    let input_cost = input_tokens as f64 * 0.27 / 1_000_000.0;
    let output_cost = output_tokens as f64 * 1.10 / 1_000_000.0;
    input_cost + output_cost
}

pub fn build_system_message(content: &str) -> ChatCompletionRequestMessage {
    ChatCompletionRequestSystemMessageArgs::default()
        .content(content.to_string())
        .build()
        .unwrap()
        .into()
}

pub fn build_user_message(content: &str) -> ChatCompletionRequestMessage {
    ChatCompletionRequestUserMessageArgs::default()
        .content(content.to_string())
        .build()
        .unwrap()
        .into()
}

pub fn build_tool_result_message(
    content: &str,
    tool_call_id: &str,
) -> ChatCompletionRequestMessage {
    ChatCompletionRequestToolMessageArgs::default()
        .content(content.to_string())
        .tool_call_id(tool_call_id.to_string())
        .build()
        .unwrap()
        .into()
}

pub fn build_assistant_tool_call_message(tool_call: &ToolCall) -> ChatCompletionRequestMessage {
    let tc_json = serde_json::json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.arguments.to_string(),
        }
    });
    let tc: async_openai::types::ChatCompletionMessageToolCall =
        serde_json::from_value(tc_json).expect("valid tool call JSON");

    ChatCompletionRequestAssistantMessageArgs::default()
        .tool_calls(vec![tc])
        .build()
        .unwrap()
        .into()
}

pub fn build_assistant_text_message(content: &str) -> ChatCompletionRequestMessage {
    ChatCompletionRequestAssistantMessageArgs::default()
        .content(content.to_string())
        .build()
        .unwrap()
        .into()
}
