use crate::transport;
use bytode_common::config::McpServerConfig;
use bytode_common::{BytodeError, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use serde_json::{Map, Value};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct McpServerSpec {
    pub name: String,
    pub config: McpServerConfig,
}

#[derive(Debug, Clone)]
pub struct McpClient {
    spec: McpServerSpec,
}

impl McpClient {
    pub fn new(spec: McpServerSpec) -> Self {
        Self { spec }
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let session = self.connect().await?;
        tokio::time::timeout(self.timeout(), session.peer().list_all_tools())
            .await
            .map_err(|_| timeout_error(&self.spec.name, "tools/list"))?
            .map_err(|error| service_error(&self.spec.name, "tools/list", error))
    }

    pub async fn call_tool(&self, remote_tool_name: &str, arguments: Value) -> Result<CallToolResult> {
        let session = self.connect().await?;
        let request = match arguments_to_object(arguments)? {
            Some(arguments) => CallToolRequestParams::new(remote_tool_name.to_string())
                .with_arguments(arguments),
            None => CallToolRequestParams::new(remote_tool_name.to_string()),
        };

        tokio::time::timeout(self.timeout(), session.peer().call_tool(request))
            .await
            .map_err(|_| timeout_error(&self.spec.name, "tools/call"))?
            .map_err(|error| service_error(&self.spec.name, "tools/call", error))
    }

    async fn connect(&self) -> Result<transport::McpSession> {
        tokio::time::timeout(self.timeout(), transport::connect(&self.spec))
            .await
            .map_err(|_| timeout_error(&self.spec.name, "initialize"))?
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.spec.config.timeout_secs)
    }
}

fn arguments_to_object(arguments: Value) -> Result<Option<Map<String, Value>>> {
    match arguments {
        Value::Null => Ok(None),
        Value::Object(map) => Ok(Some(map)),
        other => Err(BytodeError::Tool {
            tool: "mcp".into(),
            message: format!("MCP tool arguments must be a JSON object, got {other}"),
        }),
    }
}

fn timeout_error(server_name: &str, method: &str) -> BytodeError {
    BytodeError::Tool {
        tool: "mcp".into(),
        message: format!("{server_name} {method} timed out"),
    }
}

fn service_error(
    server_name: &str,
    method: &str,
    error: impl std::fmt::Display,
) -> BytodeError {
    BytodeError::Tool {
        tool: "mcp".into(),
        message: format!("{server_name} {method} failed: {error}"),
    }
}
