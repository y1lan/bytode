use bytode_common::{BytodeError, Result};
use rmcp::model::ClientInfo;
use rmcp::transport::TokioChildProcess;
use rmcp::{ServiceExt, transport::ConfigureCommandExt};
use std::collections::BTreeMap;
use std::process::Stdio;

pub async fn connect(
    client_info: &ClientInfo,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
) -> Result<crate::transport::McpSession> {
    let command_label = format!("{command} {}", args.join(" "));
    let command = tokio::process::Command::new(command).configure(|cmd| {
        cmd.args(args);
        cmd.envs(env);
    });
    let transport = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| BytodeError::Tool {
            tool: "mcp".into(),
            message: format!("failed to start MCP stdio server {command_label}: {error}"),
        })?
        .0;

    client_info
        .clone()
        .serve(transport)
        .await
        .map_err(|error| BytodeError::Tool {
            tool: "mcp".into(),
            message: format!("MCP stdio initialization failed: {error}"),
        })
}
