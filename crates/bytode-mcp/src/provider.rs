use crate::client::{McpClient, McpServerSpec};
use crate::schema::map_tool;
use async_trait::async_trait;
use bytode_common::config::McpConfig;
use bytode_common::{BytodeError, Result};
use bytode_tools::{
    Tool, ToolAvailability, ToolEntry, ToolProvider, ToolProviderId, ToolResult,
};
use rmcp::model::{CallToolResult, RawContent, ResourceContents};
use serde_json::{Value, json};

pub struct McpToolProvider {
    provider_id: &'static str,
    tools: Vec<ToolEntry>,
}

impl McpToolProvider {
    pub async fn from_config(config: &McpConfig) -> Result<Vec<Box<dyn ToolProvider>>> {
        let mut providers: Vec<Box<dyn ToolProvider>> = Vec::new();

        for (server_name, server_config) in &config.servers {
            if !server_config.enabled {
                continue;
            }

            let spec = McpServerSpec {
                name: server_name.clone(),
                config: server_config.clone(),
            };
            let client = McpClient::new(spec.clone());
            let remote_tools = client.list_tools().await?;
            providers.push(Box::new(Self::new(spec, remote_tools)));
        }

        Ok(providers)
    }

    fn new(spec: McpServerSpec, remote_tools: Vec<rmcp::model::Tool>) -> Self {
        let tools = remote_tools
            .into_iter()
            .map(|remote| {
                let mapped = map_tool(&spec.name, &spec.config, remote);
                ToolEntry::new(
                    Box::new(McpTool {
                        client: McpClient::new(spec.clone()),
                        descriptor: mapped.descriptor,
                        schema: mapped.schema,
                        remote_tool_name: mapped.remote_tool_name,
                        timeout_ms: spec.config.timeout_secs * 1_000,
                    }),
                    ToolAvailability::Always,
                )
            })
            .collect::<Vec<_>>();

        let provider_id = tools
            .first()
            .map(|entry| entry.descriptor.provider_id)
            .unwrap_or("mcp:unknown");

        Self { provider_id, tools }
    }
}

impl ToolProvider for McpToolProvider {
    fn provider_id(&self) -> ToolProviderId {
        ToolProviderId(self.provider_id)
    }

    fn list_tools(&self) -> Vec<&ToolEntry> {
        self.tools.iter().collect()
    }

    fn into_tools(self: Box<Self>) -> Vec<ToolEntry> {
        self.tools
    }
}

struct McpTool {
    client: McpClient,
    descriptor: bytode_tools::ToolDescriptor,
    schema: Value,
    remote_tool_name: String,
    timeout_ms: u64,
}

#[async_trait]
impl Tool for McpTool {
    fn descriptor(&self) -> bytode_tools::ToolDescriptor {
        self.descriptor.clone()
    }

    fn parameters_schema(&self) -> Value {
        self.schema.clone()
    }

    fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let result = self.client.call_tool(&self.remote_tool_name, args).await?;
        normalize_mcp_result(self.descriptor.name, result)
    }
}

fn normalize_mcp_result(tool_name: &str, result: CallToolResult) -> Result<ToolResult> {
    if result.is_error == Some(true) {
        return Err(BytodeError::Tool {
            tool: tool_name.into(),
            message: result_error_summary(&result),
        });
    }

    if let Some(text) = text_content(&result) {
        return Ok(ToolResult::Text {
            source: tool_name.into(),
            content: text,
            truncated: false,
        });
    }

    let value = serde_json::to_value(&result)?;
    Ok(ToolResult::Json {
        tool: tool_name.into(),
        filter: None,
        count: 1,
        data: vec![value],
    })
}

fn text_content(result: &CallToolResult) -> Option<String> {
    let parts = result
        .content
        .iter()
        .filter_map(|item| match &item.raw {
            RawContent::Text(text) => Some(text.text.as_str()),
            RawContent::Resource(resource) => match &resource.resource {
                ResourceContents::TextResourceContents { text, .. } => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn result_error_summary(result: &CallToolResult) -> String {
    if let Some(text) = text_content(result)
        && !text.trim().is_empty()
    {
        return text;
    }

    if let Some(structured) = result.structured_content.as_ref() {
        return structured.to_string();
    }

    json!({
        "isError": result.is_error,
        "contentBlocks": result.content.len(),
    })
    .to_string()
}
