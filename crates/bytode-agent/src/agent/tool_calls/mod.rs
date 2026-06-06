mod approval;
mod context;
mod engine;
mod outcome;
mod policy;
mod request;

pub use approval::{ApprovalChannel, ApprovalEvent, InteractiveApprovalChannel};
pub use context::ToolCallContext;
pub use engine::ToolCallEngine;
pub use outcome::ToolCallOutcome;
pub use request::ToolCallRequest;

use serde_json::Value;

pub fn format_args(tool_name: &str, args: &Value) -> String {
    match tool_name {
        "read_file" => {
            let path = args["path"].as_str().unwrap_or("?");
            let offset = args["offset"]
                .as_u64()
                .map(|o| format!(", offset={o}"))
                .unwrap_or_default();
            let limit = args["limit"]
                .as_u64()
                .map(|l| format!(", limit={l}"))
                .unwrap_or_default();
            format!("{path}{offset}{limit}")
        }
        "write_file" => args["path"].as_str().unwrap_or("?").to_string(),
        "search_code" => {
            let pat = args["pattern"].as_str().unwrap_or("?");
            if let Some(p) = args["path"].as_str() {
                format!("pattern=\"{pat}\", path={p}")
            } else {
                format!("pattern=\"{pat}\"")
            }
        }
        "get_diagnostics" => {
            let mut parts = Vec::new();
            if let Some(p) = args["path"].as_str() {
                parts.push(format!("path={p}"));
            }
            if let Some(f) = args["filter"].as_str() {
                parts.push(format!("filter={f}"));
            }
            if parts.is_empty() {
                "?".into()
            } else {
                parts.join(", ")
            }
        }
        "cargo" => {
            let cmd = args["cmd"].as_str().unwrap_or("?");
            if let Some(extra) = args["args"].as_array() {
                let ex: Vec<&str> = extra.iter().filter_map(|v| v.as_str()).collect();
                if ex.is_empty() {
                    cmd.to_string()
                } else {
                    format!("{cmd} {}", ex.join(" "))
                }
            } else {
                cmd.to_string()
            }
        }
        "cargo_check" => {
            if let Some(e) = args["extra_args"].as_array() {
                let ex: Vec<&str> = e.iter().filter_map(|v| v.as_str()).collect();
                if ex.is_empty() {
                    "?".into()
                } else {
                    ex.join(" ")
                }
            } else {
                "?".into()
            }
        }
        "git_status" => args["path"].as_str().unwrap_or("").to_string(),
        "git_diff" => {
            let mut parts = Vec::new();
            if args["staged"].as_bool().unwrap_or(false) {
                parts.push("staged");
            }
            if let Some(p) = args["path"].as_str() {
                parts.push(p);
            }
            parts.join(", ")
        }
        "git_log" => {
            let count = args["count"].as_u64().unwrap_or(10);
            if let Some(p) = args["path"].as_str() {
                format!("count={count}, path={p}")
            } else {
                format!("count={count}")
            }
        }
        "web_search" => {
            let q = args["query"].as_str().unwrap_or("?");
            if q.len() > 60 {
                format!("\"{}...\"", &q[..57])
            } else {
                format!("\"{q}\"")
            }
        }
        "git_commit" => {
            let msg = args["message"].as_str().unwrap_or("?");
            if msg.len() > 50 {
                format!("\"{}...\"", &msg[..47])
            } else {
                format!("\"{msg}\"")
            }
        }
        "git_push" => {
            let mut parts = Vec::new();
            if let Some(r) = args["remote"].as_str().filter(|r| *r != "origin") {
                parts.push(format!("remote={r}"));
            }
            if let Some(b) = args["branch"].as_str() {
                parts.push(format!("branch={b}"));
            }
            if args["force"].as_bool().unwrap_or(false) {
                parts.push("force".into());
            }
            if parts.is_empty() {
                "origin".into()
            } else {
                parts.join(", ")
            }
        }
        _ => "?".into(),
    }
}
