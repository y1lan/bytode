use bytode_common::{BytodeError, Result};
use http::{HeaderName, HeaderValue};
use rmcp::model::ClientInfo;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::ServiceExt;
use std::collections::{BTreeMap, HashMap};

pub async fn connect(
    client_info: &ClientInfo,
    url: &str,
    headers: &BTreeMap<String, String>,
    _timeout_secs: u64,
) -> Result<crate::transport::McpSession> {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .custom_headers(parse_headers(headers)?)
        .reinit_on_expired_session(true);
    let transport = StreamableHttpClientTransport::from_config(config);

    client_info
        .clone()
        .serve(transport)
        .await
        .map_err(|error| BytodeError::Tool {
            tool: "mcp".into(),
            message: format!("MCP HTTP(SSE) initialization failed: {error}"),
        })
}

fn parse_headers(headers: &BTreeMap<String, String>) -> Result<HashMap<HeaderName, HeaderValue>> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = HeaderName::try_from(name.as_str()).map_err(|error| BytodeError::Tool {
                tool: "mcp".into(),
                message: format!("invalid MCP HTTP header name {name}: {error}"),
            })?;
            let value = HeaderValue::from_str(value).map_err(|error| BytodeError::Tool {
                tool: "mcp".into(),
                message: format!("invalid MCP HTTP header value for {name}: {error}"),
            })?;
            Ok((name, value))
        })
        .collect()
}
