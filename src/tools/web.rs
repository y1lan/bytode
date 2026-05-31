use crate::error::{BytodeError, Result};
use crate::tools::{Tool, ToolCategory, ToolEntry, ToolResult, ToolAvailability};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

pub struct WebSearchTool {
    pub timeout_secs: u64,
    pub proxy: Option<String>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        r#"Search the web via DuckDuckGo HTML (no API key required). Returns plain-text results.

WHEN TO USE: For looking up crate docs, error messages, API references, or debugging information
not available in the local codebase.

WHEN NOT TO USE: For local code questions — use search_code or read_file.
For compiler errors — use get_diagnostics or run_cargo(cmd="check").

EXAMPLES:
  web_search(query="tokio::sync::Mutex example")     # search for tokio usage
  web_search(query="rust async trait Send bound")    # search for rust concepts
  web_search(query="reqwest 0.12 breaking changes")  # search for library docs

RETURNS: Title, URL, and snippet for each result (up to 10)."#
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query string. Be specific (include crate names, error text, etc)."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn timeout_ms(&self) -> u64 {
        self.timeout_secs * 1000
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let query = args["query"].as_str().ok_or_else(|| BytodeError::Tool {
            tool: "web_search".into(),
            message: "missing 'query' argument".into(),
        })?;

        let query_trimmed = query.trim();
        if query_trimmed.is_empty() {
            return Err(BytodeError::Tool {
                tool: "web_search".into(),
                message: "query is empty".into(),
            });
        }

        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding(query_trimmed)
        );

        let mut client_builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .user_agent("bytode/0.1.0 (terminal coding agent)");

        if let Some(ref proxy_url) = self.proxy {
            client_builder = client_builder.proxy(reqwest::Proxy::all(proxy_url)?);
        }

        let client = client_builder.build().map_err(|e| BytodeError::Tool {
            tool: "web_search".into(),
            message: format!("failed to build HTTP client: {}", e),
        })?;

        let response = client.get(&url).send().await.map_err(|e| BytodeError::Tool {
            tool: "web_search".into(),
            message: format!("request failed: {}", e),
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(BytodeError::Tool {
                tool: "web_search".into(),
                message: format!("DuckDuckGo returned HTTP {}", status),
            });
        }

        let body = response.text().await.map_err(|e| BytodeError::Tool {
            tool: "web_search".into(),
            message: format!("failed to read response body: {}", e),
        })?;

        let results = parse_ddg_html(&body);

        if results.is_empty() {
            return Ok(ToolResult::Text {
                source: "web_search".into(),
                content: "no results found".into(),
                truncated: false,
            });
        }

        let content = results.join("\n\n");

        Ok(ToolResult::Text {
            source: "web_search".into(),
            content,
            truncated: false,
        })
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "+".to_string(),
            other => {
                let bytes = other.to_string().into_bytes();
                bytes
                    .iter()
                    .map(|b| format!("%{:02X}", b))
                    .collect::<Vec<_>>()
                    .join("")
            }
        })
        .collect()
}

fn parse_ddg_html(html: &str) -> Vec<String> {
    let mut results = Vec::new();

    let class = "result__body";
    let mut pos = 0;

    while pos < html.len() && results.len() < 10 {
        let body_start = match html[pos..].find(&format!("class=\"{}\"", class)) {
            Some(i) => pos + i,
            None => break,
        };

        let section_end = html[body_start..].find("</div>").map(|i| body_start + i);

        let title = extract_tag_content(&html, body_start, "result__title");
        let snippet = extract_tag_content(&html, body_start, "result__snippet");
        let link = extract_link(&html, body_start);

        if !title.is_empty() || !snippet.is_empty() {
            let mut result = String::new();
            if !title.is_empty() {
                result.push_str(&format!("Title: {}", title));
            }
            if !link.is_empty() {
                result.push_str(&format!("\nURL: {}", link));
            }
            if !snippet.is_empty() {
                result.push_str(&format!("\nSnippet: {}", snippet));
            }
            results.push(result);
        }

        pos = section_end.unwrap_or(html.len());
    }

    results
}

fn extract_tag_content(html: &str, start: usize, class_name: &str) -> String {
    let class_pattern = format!("class=\"{}\"", class_name);
    let tag_start = match html[start..].find(&class_pattern) {
        Some(i) => start + i,
        None => return String::new(),
    };

    let content_start = match html[tag_start..].find('>') {
        Some(i) => tag_start + i + 1,
        None => return String::new(),
    };

    let content_end = html[content_start..]
        .find("</a>")
        .or_else(|| html[content_start..].find("</span>"))
        .or_else(|| html[content_start..].find("</td>"))
        .unwrap_or(500);

    let raw = &html[content_start..content_start + content_end.min(html.len() - content_start)];
    strip_html(raw)
}

fn extract_link(html: &str, start: usize) -> String {
    let link_start = match html[start..].find("class=\"result__url\"") {
        Some(i) => start + i,
        None => return String::new(),
    };

    let trimmed = &html[link_start..];
    let href_pos = match trimmed.find("href=\"") {
        Some(i) => i + 6,
        None => {
            let href_start = match trimmed.find("href='") {
                Some(i) => i + 6,
                None => return String::new(),
            };
            let end = trimmed[href_start..].find('\'').unwrap_or(200);
            return trimmed[href_start..href_start + end].to_string();
        }
    };

    let end = trimmed[href_pos..].find('"').unwrap_or(200);
    trimmed[href_pos..href_pos + end].to_string()
}

fn strip_html(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    let trimmed = result.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#x27;", "'").replace("&nbsp;", " ");
    let collapsed: String = trimmed
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    collapsed
}

impl WebSearchTool {
    pub fn entry(timeout_secs: u64, proxy: Option<String>) -> ToolEntry {
        ToolEntry {
            tool: Box::new(WebSearchTool { timeout_secs, proxy }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::Always,
        }
    }
}
