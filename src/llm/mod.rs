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

    pub async fn chat(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: Vec<Value>,
    ) -> Result<LlmOutput> {
        // Convert Vec<Value> to Vec<ChatCompletionTool> via serde
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

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| BytodeError::Llm(format!("API error: {}", e)))?;

        let choice = response
            .choices
            .first()
            .ok_or_else(|| BytodeError::Llm("no choices in response".into()))?;

        let message = &choice.message;

        if let Some(tool_calls) = &message.tool_calls {
            if let Some(tc) = tool_calls.first() {
                let id = tc.id.clone();
                let name = tc.function.name.clone();
                let arguments: Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(Value::Null);

                return Ok(LlmOutput::ToolCall(ToolCall {
                    id,
                    name,
                    arguments,
                }));
            }
        }

        Ok(LlmOutput::Text(
            message.content.clone().unwrap_or_default(),
        ))
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
