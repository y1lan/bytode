use bytode_common::config::{McpServerConfig, McpTransportConfig};

pub fn provider_id(server_name: &str) -> String {
    format!("mcp:{server_name}")
}

pub fn transport_name(config: &McpServerConfig) -> &'static str {
    match config.transport {
        McpTransportConfig::Stdio { .. } => "stdio",
        McpTransportConfig::Sse { .. } => "http+sse",
    }
}

pub fn sanitized_tool_name(server_name: &str, remote_tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_name(server_name),
        sanitize_name(remote_tool_name)
    )
}

pub fn sanitize_name(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

pub fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}
