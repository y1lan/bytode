use crate::error::{BytodeError, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestAssistantMessageArgs,
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs,
        CreateChatCompletionRequestArgs,
    },
    Client,
};
use futures::StreamExt;
use serde_json::Value;

pub struct DeepSeekClient {
    client: Client<OpenAIConfig>,
    model: String,
    max_tokens: u32,
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
        })
    }

    /// Streaming chat: calls `on_text` for each text chunk as it arrives.
    /// Returns the final LlmOutput (either the full text or an accumulated ToolCall).
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

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| BytodeError::Llm(format!("chunk error: {}", e)))?;

            for choice in &chunk.choices {
                // Text delta
                if let Some(ref content) = choice.delta.content {
                    on_text(content);
                    full_text.push_str(content);
                }

                // Tool call delta (accumulate across chunks)
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
                                tool_call_args.push_str(args);
                            }
                        }
                    }
                }
            }
        }

        // If we accumulated a tool call, return it
        if !tool_call_name.is_empty() {
            let arguments: Value = if tool_call_args.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&tool_call_args).unwrap_or(Value::Null)
            };

            return Ok(LlmOutput::ToolCall(ToolCall {
                id: tool_call_id,
                name: tool_call_name,
                arguments,
            }));
        }

        Ok(LlmOutput::Text(full_text))
    }

    /// Non-streaming fallback (for when we don't need real-time output)
    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<Value>,
    ) -> Result<LlmOutput> {
        let mut text = String::new();
        self.chat_stream(messages, tools, |chunk| {
            text.push_str(chunk);
        })
        .await
    }
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
