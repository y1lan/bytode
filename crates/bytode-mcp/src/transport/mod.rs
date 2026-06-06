mod sse;
mod stdio;

use crate::client::McpServerSpec;
use bytode_common::config::McpTransportConfig;
use bytode_common::Result;
use rmcp::model::ClientInfo;
use rmcp::service::RunningService;
use rmcp::RoleClient;

pub type McpSession = RunningService<RoleClient, ClientInfo>;

pub async fn connect(spec: &McpServerSpec) -> Result<McpSession> {
    let client_info = client_info();
    match &spec.config.transport {
        McpTransportConfig::Stdio { command, args, env } => {
            stdio::connect(&client_info, command, args, env).await
        }
        McpTransportConfig::Sse { url, headers } => {
            sse::connect(&client_info, url, headers, spec.config.timeout_secs).await
        }
    }
}

fn client_info() -> ClientInfo {
    let mut info = ClientInfo::default();
    info.client_info.name = "bytode".into();
    info.client_info.version = env!("CARGO_PKG_VERSION").into();
    info
}
