use bytode_common::config::McpServerConfig;
use bytode_tools::{
    ApprovalKind, McpToolMeta, ProviderMeta, RiskLevel, ToolCapability, ToolCategory,
    ToolDescriptor,
};
use rmcp::model::{Meta, Tool, ToolAnnotations};
use serde_json::Value;

use crate::config::{leak, provider_id, sanitized_tool_name, transport_name};

pub struct MappedMcpTool {
    pub descriptor: ToolDescriptor,
    pub schema: Value,
    pub remote_tool_name: String,
}

pub fn map_tool(server_name: &str, server_config: &McpServerConfig, tool: Tool) -> MappedMcpTool {
    let remote_tool_name = tool.name.to_string();
    let tool_name = leak(sanitized_tool_name(server_name, &remote_tool_name));
    let provider_id = leak(provider_id(server_name));
    let transport = leak(transport_name(server_config).to_string());
    let server_name = leak(server_name.to_string());
    let remote_tool_name_static = leak(remote_tool_name.clone());
    let description = leak(tool_description(server_name, &remote_tool_name, &tool));
    let schema = tool.schema_as_json_value();

    let profile = classify_tool(tool.annotations.as_ref(), tool.meta.as_ref(), &remote_tool_name);

    MappedMcpTool {
        descriptor: ToolDescriptor {
            name: tool_name,
            description,
            provider_id,
            provider_meta: Some(ProviderMeta::Mcp(McpToolMeta {
                server_name,
                remote_tool_name: remote_tool_name_static,
                transport,
            })),
            category: profile.category,
            capabilities: profile.capabilities,
            default_risk: profile.risk,
            approval: ApprovalKind::Always,
        },
        schema,
        remote_tool_name,
    }
}

struct ToolProfile {
    category: ToolCategory,
    capabilities: Vec<ToolCapability>,
    risk: RiskLevel,
}

fn tool_description(server_name: &str, remote_tool_name: &str, tool: &Tool) -> String {
    match tool.description.as_deref() {
        Some(description) if !description.trim().is_empty() => description.to_string(),
        _ => format!("MCP tool {remote_tool_name} from server {server_name}"),
    }
}

fn classify_tool(
    annotations: Option<&ToolAnnotations>,
    meta: Option<&Meta>,
    remote_tool_name: &str,
) -> ToolProfile {
    let declared = declared_capabilities(meta, remote_tool_name);
    if declared.command {
        return ToolProfile {
            category: ToolCategory::Modification,
            capabilities: vec![ToolCapability::UnknownExternal, ToolCapability::RunProjectCommand],
            risk: RiskLevel::Critical,
        };
    }

    if declared.write {
        return ToolProfile {
            category: ToolCategory::Modification,
            capabilities: vec![ToolCapability::UnknownExternal],
            risk: RiskLevel::High,
        };
    }

    if declared.read_only || annotations.and_then(|value| value.read_only_hint) == Some(true) {
        return ToolProfile {
            category: ToolCategory::ReadOnly,
            capabilities: vec![ToolCapability::UnknownExternal],
            risk: RiskLevel::Medium,
        };
    }

    ToolProfile {
        category: if annotations.and_then(|value| value.destructive_hint) == Some(false) {
            ToolCategory::ReadOnly
        } else {
            ToolCategory::Modification
        },
        capabilities: vec![ToolCapability::UnknownExternal],
        risk: RiskLevel::High,
    }
}

#[derive(Default)]
struct DeclaredCapabilities {
    read_only: bool,
    write: bool,
    command: bool,
}

fn declared_capabilities(meta: Option<&Meta>, remote_tool_name: &str) -> DeclaredCapabilities {
    let mut caps = DeclaredCapabilities::default();

    for token in capability_tokens(meta).into_iter().chain(name_tokens(remote_tool_name)) {
        match token.as_str() {
            "read" | "readonly" | "read_only" | "fetch" | "list" | "inspect" | "query" => {
                caps.read_only = true;
            }
            "write" | "create" | "update" | "delete" | "modify" | "mutate" | "patch" => {
                caps.write = true;
            }
            "exec" | "execute" | "command" | "shell" | "terminal" | "spawn" | "run" => {
                caps.command = true;
            }
            _ => {}
        }
    }

    caps
}

fn capability_tokens(meta: Option<&Meta>) -> Vec<String> {
    let Some(meta) = meta else {
        return Vec::new();
    };

    [
        meta.get("capabilities"),
        meta.get("bytode:capabilities"),
        meta.get("bytode_capabilities"),
        meta.get("x-capabilities"),
    ]
    .into_iter()
    .flatten()
    .flat_map(value_tokens)
    .collect()
}

fn name_tokens(name: &str) -> Vec<String> {
    name.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect()
}

fn value_tokens(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => name_tokens(text),
        Value::Array(items) => items.iter().flat_map(value_tokens).collect(),
        Value::Object(map) => map
            .iter()
            .flat_map(|(key, value)| {
                let mut tokens = name_tokens(key);
                tokens.extend(value_tokens(value));
                tokens
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytode_common::config::{McpServerConfig, McpTransportConfig};
    use rmcp::model::{JsonObject, ToolAnnotations};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn server_config() -> McpServerConfig {
        McpServerConfig {
            enabled: true,
            transport: McpTransportConfig::Sse {
                url: "http://127.0.0.1:8787/mcp".into(),
                headers: BTreeMap::new(),
            },
            timeout_secs: 15,
        }
    }

    fn base_tool(name: &str) -> Tool {
        Tool::new(
            name.to_string(),
            "tool",
            Arc::new(JsonObject::from_iter([("type".into(), json!("object"))])),
        )
    }

    #[test]
    fn read_only_annotations_map_to_medium_risk() {
        let tool = base_tool("fetch_docs").with_annotations(ToolAnnotations::new().read_only(true));
        let mapped = map_tool("docs", &server_config(), tool);
        assert_eq!(mapped.descriptor.default_risk, RiskLevel::Medium);
        assert_eq!(mapped.descriptor.category, ToolCategory::ReadOnly);
    }

    #[test]
    fn declared_write_maps_to_high_risk() {
        let mut tool = base_tool("workspace_apply");
        tool.meta = Some(rmcp::model::Meta(serde_json::Map::from_iter([(
            "capabilities".into(),
            json!(["write"]),
        )])));
        let mapped = map_tool("workspace", &server_config(), tool);
        assert_eq!(mapped.descriptor.default_risk, RiskLevel::High);
        assert_eq!(mapped.descriptor.approval, ApprovalKind::Always);
    }

    #[test]
    fn declared_command_maps_to_critical_risk() {
        let mut tool = base_tool("shell_run");
        tool.meta = Some(rmcp::model::Meta(serde_json::Map::from_iter([(
            "capabilities".into(),
            json!(["command"]),
        )])));
        let mapped = map_tool("ops", &server_config(), tool);
        assert_eq!(mapped.descriptor.default_risk, RiskLevel::Critical);
        assert!(mapped
            .descriptor
            .capabilities
            .contains(&ToolCapability::RunProjectCommand));
    }
}
