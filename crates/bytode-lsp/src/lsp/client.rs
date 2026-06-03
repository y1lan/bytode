use crate::error::{BytodeError, Result};
use crate::lsp::types::Diagnostic;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub struct LspClient {
    process: Mutex<Option<LspProcess>>,
    next_id: AtomicU64,
    pub config: LspConfig,
    diagnostics_cache: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
}

#[derive(Debug, Clone)]
pub struct LspConfig {
    pub command: String,
    pub project_root: PathBuf,
    pub options: Value,
}

struct LspProcess {
    child: Child,
    stdin: tokio::io::BufWriter<tokio::process::ChildStdin>,
}

impl LspClient {
    pub fn new(config: LspConfig) -> Self {
        LspClient {
            process: Mutex::new(None),
            next_id: AtomicU64::new(1),
            config,
            diagnostics_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn ensure_started(&self) -> Result<()> {
        let mut guard = self.process.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let mut child = Command::new(&self.config.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BytodeError::Lsp(format!("launch rust-analyzer: {}", e)))?;

        let stdin = child.stdin.take().ok_or_else(|| BytodeError::Lsp("no stdin".into()))?;
        let stdout_opt = child.stdout.take();

        let mut proc = LspProcess {
            child,
            stdin: tokio::io::BufWriter::new(stdin),
        };

        let root_uri = format!("file://{}", self.config.project_root.display());

        let init_params = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "publishDiagnostics": {}
                    }
                },
                "initializationOptions": self.config.options,
            }
        });

        // Send initialize
        let init_data = build_lsp_message(&init_params);
        proc.stdin.write_all(&init_data).await?;
        proc.stdin.flush().await?;

        // Read initialize response
        if let Some(stdout) = stdout_opt {
            let mut reader = BufReader::new(stdout);
            let init_response = read_lsp_response(&mut reader, 1).await?;
            if init_response.get("error").is_some() {
                return Err(BytodeError::Lsp(format!(
                    "initialize failed: {:?}",
                    init_response["error"]
                )));
            }

            // Send initialized notification
            let initialized = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            });
            let init_data = build_lsp_message(&initialized);
            proc.stdin.write_all(&init_data).await?;
            proc.stdin.flush().await?;

            // Trigger diagnostics by opening Cargo.toml
            let cargo_toml = self.config.project_root.join("Cargo.toml");
            if cargo_toml.exists() {
                let uri = format!("file://{}", cargo_toml.display());
                let content = std::fs::read_to_string(&cargo_toml).unwrap_or_default();
                let did_open = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": "toml",
                            "version": 1,
                            "text": content
                        }
                    }
                });
                let data = build_lsp_message(&did_open);
                proc.stdin.write_all(&data).await?;
                proc.stdin.flush().await?;
            }

            // Spawn background reader for diagnostics
            tokio::spawn(read_diagnostics_loop(reader, Arc::clone(&self.diagnostics_cache)));
        }

        *guard = Some(proc);
        Ok(())
    }

    pub async fn get_cached_diagnostics(&self) -> Vec<Diagnostic> {
        let cache = self.diagnostics_cache.lock().await;
        cache.values().flat_map(|v| v.iter()).cloned().collect()
    }

    pub async fn get_cached_diagnostics_for(&self, file: &str) -> Vec<Diagnostic> {
        let cache = self.diagnostics_cache.lock().await;
        cache.get(file).cloned().unwrap_or_default()
    }

    pub async fn notify_did_change(&self, file_path: &std::path::Path, content: &str) {
        let uri = format!("file://{}", file_path.display());
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "version": 1
                },
                "contentChanges": [
                    { "text": content }
                ]
            }
        });

        let mut guard = self.process.lock().await;
        if let Some(ref mut proc) = *guard {
            let data = build_lsp_message(&notification);
            let _ = proc.stdin.write_all(&data).await;
            let _ = proc.stdin.flush().await;
        }
    }

    pub async fn shutdown(&self) {
        let mut guard = self.process.lock().await;
        if let Some(mut proc) = guard.take() {
            let shutdown = serde_json::json!({
                "jsonrpc": "2.0",
                "id": self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                "method": "shutdown",
                "params": null
            });
            let data = build_lsp_message(&shutdown);
            let _ = proc.stdin.write_all(&data).await;

            let exit = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "exit",
                "params": null
            });
            let data = build_lsp_message(&exit);
            let _ = proc.stdin.write_all(&data).await;
            let _ = proc.stdin.flush().await;

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let _ = proc.child.kill().await;
        }
    }
}

fn build_lsp_message(msg: &Value) -> Vec<u8> {
    let json_str = serde_json::to_string(msg).unwrap();
    let header = format!("Content-Length: {}\r\n\r\n", json_str.len());
    let mut data = header.into_bytes();
    data.extend_from_slice(json_str.as_bytes());
    data
}

async fn read_lsp_response(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    expected_id: u64,
) -> Result<Value> {
    loop {
        // Read Content-Length header
        let mut header_line = String::new();
        reader.read_line(&mut header_line).await?;

        let content_length: usize = header_line
            .strip_prefix("Content-Length: ")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);

        // Skip \r\n separator
        let mut empty_line = String::new();
        reader.read_line(&mut empty_line).await?;

        // Read body
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await?;

        let message: Value = serde_json::from_slice(&body)?;

        // Process notifications, return responses
        if message.get("id").is_none() {
            // It's a notification — skip for responses (handled in background task)
            continue;
        }

        if message.get("id").and_then(|id| id.as_u64()) == Some(expected_id) {
            return Ok(message);
        }
    }
}

async fn read_diagnostics_loop(
    mut reader: BufReader<tokio::process::ChildStdout>,
    cache: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
) {
    loop {
        let mut header_line = String::new();
        if reader.read_line(&mut header_line).await.is_err() {
            break;
        }

        let content_length: usize = header_line
            .strip_prefix("Content-Length: ")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);

        let mut empty_line = String::new();
        if reader.read_line(&mut empty_line).await.is_err() {
            break;
        }

        let mut body = vec![0u8; content_length];
        if reader.read_exact(&mut body).await.is_err() {
            break;
        }

        let message: Value = match serde_json::from_slice(&body) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if message.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics") {
            let params = &message["params"];
            let uri = params["uri"].as_str().unwrap_or("");

            let path = uri.strip_prefix("file://")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(uri));

            let diags: Vec<Diagnostic> = params["diagnostics"]
                .as_array()
                .map(|arr| {
                    arr.iter().map(|d| Diagnostic {
                        file: path.to_string_lossy().to_string(),
                        line: d["range"]["start"]["line"].as_u64().unwrap_or(0) as u32 + 1,
                        column: d["range"]["start"]["character"].as_u64().unwrap_or(0) as u32,
                        severity: match d["severity"].as_u64() {
                            Some(1) => "error",
                            Some(2) => "warning",
                            Some(3) => "info",
                            Some(4) => "hint",
                            _ => "unknown",
                        }.to_string(),
                        message: d["message"].as_str().unwrap_or("").to_string(),
                        code: d.get("code").and_then(|c| c.as_str()).map(String::from),
                    }).collect()
                })
                .unwrap_or_default();

            let mut cache = cache.lock().await;
            cache.insert(path.to_string_lossy().to_string(), diags);
        }
    }
}
