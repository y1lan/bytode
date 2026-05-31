mod agent;
mod config;
mod error;
mod llm;
mod lsp;
mod project;
mod tools;
mod ui;

use agent::Agent;
use clap::Parser;
use config::Config;
use error::Result;
use llm::DeepSeekClient;
use lsp::{LspClient, LspConfig};
use project::ProjectProfile;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tools::{
    cargo::CargoTool,
    check::CheckTool,
    file::{ReadFileTool, WriteFileTool},
    git::{GitDiffTool, GitLogTool, GitStatusTool},
    lsp_diag::DiagnosticsTool,
    search::SearchTool,
    web::WebSearchTool,
    ToolAvailability, ToolCategory, ToolEntry, ToolRegistry,
};
use ui::{layout::UiState, statusbar::StatusBarState, Terminal};

#[derive(Parser)]
#[command(name = "bytode", about = "Terminal coding agent")]
struct Cli {
    task: Option<String>,
    #[arg(short, long, default_value = ".")]
    project: PathBuf,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long, default_value = "deepseek-v4-pro")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_file = File::create("/tmp/bytode.log")
        .expect("failed to create /tmp/bytode.log");
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log_file))
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bytode=info".into()),
        )
        .init();

    let mut cli = Cli::parse();
    let api_key = cli
        .api_key
        .take()
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
        .unwrap_or_else(|| {
            eprintln!("Error: DEEPSEEK_API_KEY is required.");
            std::process::exit(1);
        });

    let project_root = std::fs::canonicalize(&cli.project)?;
    let config = Config::load(&project_root)?;
    let profile = ProjectProfile::detect(&project_root, &config)?;

    let lsp_options = load_rust_analyzer_config(&project_root);
    let lsp_client = Arc::new(LspClient::new(LspConfig {
        command: "rust-analyzer".into(),
        project_root: project_root.clone(),
        options: lsp_options,
    }));

    let tools: Vec<ToolEntry> = vec![
        ToolEntry {
            tool: Box::new(ReadFileTool { project_root: project_root.clone() }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::Always,
        },
        ToolEntry {
            tool: Box::new(WriteFileTool {
                project_root: project_root.clone(),
                confirm_before_write: config.agent.confirm_before_write,
                max_file_size: config.security.max_file_size,
                forbidden_patterns: config.security.forbidden_write_patterns.clone(),
                lsp: Some(lsp_client.clone()),
            }),
            category: ToolCategory::Modification,
            availability: ToolAvailability::Always,
        },
        ToolEntry {
            tool: Box::new(SearchTool {
                project_root: project_root.clone(),
                ignore_dirs: config.search.ignore_dirs.clone(),
                max_results: config.search.max_results,
            }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::Always,
        },
        ToolEntry {
            tool: Box::new(CheckTool { extra_args: config.build.extra_check_flags.clone() }),
            category: ToolCategory::Build,
            availability: ToolAvailability::PrimaryLanguage { requires: &["rust"] },
        },
        ToolEntry {
            tool: Box::new(DiagnosticsTool { lsp: lsp_client.clone() }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::DetectedLanguage { languages: &["rust"] },
        },
        ToolEntry {
            tool: Box::new(WebSearchTool {
                timeout_secs: config.web_search.timeout_secs,
                proxy: config.web_search.proxy.clone(),
            }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::Always,
        },
        ToolEntry {
            tool: Box::new(CargoTool { project_root: project_root.clone() }),
            category: ToolCategory::Build,
            availability: ToolAvailability::PrimaryLanguage { requires: &["rust"] },
        },
        ToolEntry {
            tool: Box::new(GitStatusTool { project_root: project_root.clone() }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::Always,
        },
        ToolEntry {
            tool: Box::new(GitDiffTool { project_root: project_root.clone() }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::Always,
        },
        ToolEntry {
            tool: Box::new(GitLogTool { project_root: project_root.clone() }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::Always,
        },
    ];

    let registry = ToolRegistry::new(tools);
    let (enabled_set, disabled_set, is_exact) = match &config.tools {
        config::ToolSelection::Exact { enabled } => {
            (enabled.iter().cloned().collect(), HashSet::new(), true)
        }
        config::ToolSelection::Additive { enable, disable } => {
            (enable.iter().cloned().collect(), disable.iter().cloned().collect(), false)
        }
        config::ToolSelection::None => (HashSet::new(), HashSet::new(), false),
    };

    let llm = DeepSeekClient::new(api_key, cli.model.clone(), None)?;
    let mut agent = Agent::new(llm, registry, &profile, &enabled_set, &disabled_set, is_exact);

    // Session persistence
    let session_dir = dirs::home_dir().unwrap().join(".bycode").join("sessions");
    std::fs::create_dir_all(&session_dir)?;
    let session_file = session_path(&project_root, &session_dir);

    // Session selection if in REPL mode
    let mut chat_history_init: Vec<String> = Vec::new();
    if cli.task.is_none() {
        let sessions = session_list(&session_dir);
        if !sessions.is_empty() {
            eprintln!("\nAvailable sessions:");
            for (i, s) in sessions.iter().enumerate() {
                eprintln!("  {}. {} ({} turns)", i + 1, s.name, s.turns);
            }
            eprintln!("  {}. Start new session\n", sessions.len() + 1);
            loop {
                let _ = std::io::stderr().write(b"Select session [1-");
                let _ = write!(std::io::stderr(), "{}", sessions.len() + 1);
                let _ = std::io::stderr().write(b"]: ");
                let _ = std::io::stderr().flush();
                let mut buf = String::new();
                if std::io::stdin().read_line(&mut buf).is_err() {
                    break;
                }
                let choice: usize = match buf.trim().parse() {
                    Ok(n) if n >= 1 && n <= sessions.len() + 1 => n,
                    _ => continue,
                };
                if choice == sessions.len() + 1 {
                    // Start new session
                } else if let Some(s) = sessions.get(choice - 1) {
                    if agent.load_session(&s.path).is_ok() {
                        chat_history_init = agent.chat_history_text();
                        eprintln!("Loaded session: {}\n", s.name);
                    } else {
                        eprintln!("Failed to load session\n");
                    }
                }
                break;
            }
        }
    } else if session_file.exists() {
        let _ = agent.load_session(&session_file);
    }

    // Ctrl+C cancellation
    let cancel = agent.cancel_token();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel.store(true, Ordering::Relaxed);
    });

    let git_branch = get_git_branch(&project_root);
    let git_dirty = is_git_dirty(&project_root);
    let detection_source: String = if matches!(config.tools, config::ToolSelection::None) {
        "auto-detect".into()
    } else {
        ".bytode.toml".into()
    };

    match cli.task {
        Some(task) => {
            eprintln!("→ {}", task);
            agent
                .run_turn_streaming(&task, |chunk| {
                    print!("{}", chunk);
                    let _ = std::io::stdout().flush();
                })
                .await?;
            println!();
        }
        None => {
            run_repl(
                &mut agent,
                &profile,
                git_branch,
                git_dirty,
                &detection_source,
                chat_history_init,
            )
            .await?;
        }
    };

    let _ = agent.save_session(&session_file);
    lsp_client.shutdown().await;
    Ok(())
}

async fn run_repl(
    agent: &mut Agent,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
    chat_history_init: Vec<String>,
) -> Result<()> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    let terminal = Arc::new(Mutex::new(Terminal::init()?));
    let mut user_input = String::new();
    let mut sidebar_visible = false;

    let chat_history: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(chat_history_init));
    let scroll_offset: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

    loop {
        let history = chat_history.lock().unwrap();
        let offset = *scroll_offset.lock().unwrap();
        let history_snapshot = history.clone();
        drop(history);

        let scroll_info = if offset > 0 {
            format!(" [↑ {}]", offset)
        } else {
            String::new()
        };

        let state = UiState {
            sidebar_visible,
            tool_names: agent.tool_names(),
            status: StatusBarState {
                dir: std::env::current_dir()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or_default(),
                git_branch: git_branch.clone(),
                git_dirty,
                task: format!("idle{}", scroll_info),
                model: agent.model_name().to_string(),
                ctx_used: agent.context_used(),
                ctx_total: agent.context_total(),
                tool_count: agent.tool_names().len(),
                session_cost: agent.session_cost(),
                session_calls: agent.session_call_count(),
                mode: mode_str(agent.mode()),
            },
            tool_result: None,
            history: history_snapshot,
            streaming: None,
            scroll_offset: offset,
            user_input: user_input.clone(),
            approval: None,
            primary_language: format!("{} [{}]", profile.primary, profile.build_system),
            detection_source: detection_source.to_string(),
        };

        {
            let mut t = terminal.lock().unwrap();
            t.draw(|frame| ui::layout::render_ui(frame, &state))?;
        }

        if let Ok(event) = crossterm::event::read() {
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Enter => {
                        let input = std::mem::take(&mut user_input);
                        if input == "/exit" || input == "/quit" {
                            break;
                        }
                        if input.starts_with("/model") {
                            let parts: Vec<&str> = input.split_whitespace().collect();
                            if parts.len() == 1 {
                                let mut h = chat_history.lock().unwrap();
                                h.push(format!("\u{25b8} {}", input));
                                h.push(format!("Current model: {}", agent.model_name()));
                                h.push(
                                    "Usage: /model pro | /model flash".into(),
                                );
                            } else {
                                let name = parts[1].to_lowercase();
                                let model = match name.as_str() {
                                    "pro" => "deepseek-v4-pro",
                                    "flash" => "deepseek-v4-flash",
                                    "deepseek-v4-pro" => "deepseek-v4-pro",
                                    "deepseek-v4-flash" => "deepseek-v4-flash",
                                    _ => {
                                        let mut h = chat_history.lock().unwrap();
                                        h.push(format!("\u{25b8} {}", input));
                                        h.push(format!(
                                            "Unknown model: {}. Use pro or flash.",
                                            name
                                        ));
                                        continue;
                                    }
                                };
                                agent.set_model(model.to_string());
                                let mut h = chat_history.lock().unwrap();
                                h.push(format!("\u{25b8} {}", input));
                                h.push(format!("Switched to model: {}", model));
                            }
                            continue;
                        }
                        if input.starts_with("/plan") {
                            let parts: Vec<&str> = input.split_whitespace().collect();
                            let toggle = parts.get(1).map(|s| s.to_lowercase());
                            let new_mode = match toggle.as_deref() {
                                Some("on") | Some("enable") => Some(agent::AgentMode::Plan),
                                Some("off") | Some("disable") => Some(agent::AgentMode::Normal),
                                None | Some("toggle") => {
                                    Some(if agent.mode() == agent::AgentMode::Plan {
                                        agent::AgentMode::Normal
                                    } else {
                                        agent::AgentMode::Plan
                                    })
                                }
                                _ => {
                                    let mut h = chat_history.lock().unwrap();
                                    h.push(format!("\u{25b8} {}", input));
                                    h.push("Usage: /plan [on|off|toggle]".into());
                                    continue;
                                }
                            };
                            if let Some(mode) = new_mode {
                                agent.set_mode(mode);
                                let label = match mode {
                                    agent::AgentMode::Plan => "plan (read-only)",
                                    agent::AgentMode::Normal => "build",
                                };
                                let mut h = chat_history.lock().unwrap();
                                h.push(format!("\u{25b8} {}", input));
                                h.push(format!(
                                    "Mode: {} | tools: {}",
                                    label,
                                    agent.tool_names().join(", ")
                                ));
                            }
                            continue;
                        }
                        if input.is_empty() {
                            continue;
                        }

                        // Add user message to history
                        {
                            let mut h = chat_history.lock().unwrap();
                            h.push(format!("\u{25b8} {}", input));
                        }
                        *scroll_offset.lock().unwrap() = 0;

                        // Show user message + thinking state immediately
                        {
                            let h = chat_history.lock().unwrap();
                            let snapshot = h.clone();
                            let mut t = terminal.lock().unwrap();
                            let _ = t.draw(|frame| {
                                let s = UiState {
                                    sidebar_visible,
                                    tool_names: agent.tool_names(),
                                    status: StatusBarState {
                                        dir: std::env::current_dir()
                                            .map(|d| d.to_string_lossy().to_string())
                                            .unwrap_or_default(),
                                        git_branch: git_branch.clone(),
                                        git_dirty,
                                        task: "thinking...".into(),
                                        model: agent.model_name().to_string(),
                                        ctx_used: agent.context_used(),
                                        ctx_total: agent.context_total(),
                                        tool_count: agent.tool_names().len(),
                                        session_cost: agent.session_cost(),
                                        session_calls: agent.session_call_count(),
                                        mode: mode_str(agent.mode()),
                                    },
                                    tool_result: None,
                                    history: snapshot,
                                    streaming: None,
                                    scroll_offset: 0,
                                    user_input: String::new(),
                                    approval: None,
                                    primary_language: format!(
                                        "{} [{}]",
                                        profile.primary, profile.build_system
                                    ),
                                    detection_source: detection_source.to_string(),
                                };
                                ui::layout::render_ui(frame, &s);
                            });
                        }

                        let streaming_buf: Arc<Mutex<String>> =
                            Arc::new(Mutex::new(String::new()));

                        let terminal_clone = Arc::clone(&terminal);
                        let history_clone = Arc::clone(&chat_history);
                        let scroll_clone = Arc::clone(&scroll_offset);
                        let streaming_clone = Arc::clone(&streaming_buf);
                        let tool_names = agent.tool_names();
                        let ctx_used = agent.context_used();
                        let ctx_total = agent.context_total();
                        let session_cost = agent.session_cost();
                        let session_calls = agent.session_call_count();
                        let primary_lang =
                            format!("{} [{}]", profile.primary, profile.build_system);
                        let ds = detection_source.to_string();
                        let model_s = agent.model_name().to_string();
                        let mode_s = mode_str(agent.mode());
                        let dir_s = std::env::current_dir()
                            .map(|d| d.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let gb = git_branch.clone();
                        let current_tool: Arc<Mutex<String>> =
                            Arc::new(Mutex::new(String::new()));
                        let last_draw: Arc<Mutex<std::time::Instant>> =
                            Arc::new(Mutex::new(std::time::Instant::now()));

                        let result = agent
                            .run_turn_streaming(&input, move |chunk| {
                                streaming_clone.lock().unwrap().push_str(chunk);
                                {
                                    let trimmed = chunk.trim();
                                    if trimmed.ends_with("(...)") {
                                        let tool_name =
                                            trimmed.trim_end_matches("(...)").trim();
                                        *current_tool.lock().unwrap() =
                                            tool_name.to_string();
                                    }
                                }
                                *scroll_clone.lock().unwrap() = 0;

                                let now = std::time::Instant::now();
                                let too_soon = now
                                    .duration_since(*last_draw.lock().unwrap())
                                    .as_millis()
                                    < 20;
                                if too_soon {
                                    return;
                                }
                                *last_draw.lock().unwrap() = now;

                                if let Ok(mut t) = terminal_clone.lock() {
                                    let h = history_clone.lock().unwrap();
                                    let start = h.len().saturating_sub(10);
                                    let snapshot: Vec<String> =
                                        h.iter().skip(start).cloned().collect();
                                    let streamed =
                                        streaming_clone.lock().unwrap().clone();
                                    let tool = current_tool.lock().unwrap().clone();
                                    let task = if tool.is_empty() {
                                        "thinking...".into()
                                    } else {
                                        format!("{}()...", tool)
                                    };
                                    let _ = t.draw(|frame| {
                                        let s = UiState {
                                            sidebar_visible,
                                            tool_names: tool_names.clone(),
                                            status: StatusBarState {
                                                dir: dir_s.clone(),
                                                git_branch: gb.clone(),
                                                git_dirty,
                                                task,
                                                model: model_s.clone(),
                                                ctx_used,
                                                ctx_total,
                                                tool_count: tool_names.len(),
                                                session_cost,
                                                session_calls,
                                                mode: mode_s.clone(),
                                            },
                                            tool_result: None,
                                            history: snapshot,
                                            streaming: if streamed.is_empty() {
                                                None
                                            } else {
                                                Some(streamed)
                                            },
                                            scroll_offset: 0,
                                            user_input: String::new(),
                                            approval: None,
                                            primary_language: primary_lang.clone(),
                                            detection_source: ds.clone(),
                                        };
                                        ui::layout::render_ui(frame, &s);
                                    });
                                }
                            })
                            .await;

                        let final_text = streaming_buf.lock().unwrap().clone();
                        let mut h = chat_history.lock().unwrap();
                        match result {
                            Ok(agent::AgentOutput::Text(_)) => {
                                if !final_text.is_empty() {
                                    h.push(final_text);
                                }
                            }
                            Err(e) => {
                                h.push(format!("Error: {}", e));
                            }
                        }
                        *scroll_offset.lock().unwrap() = 0;
                    }
                    KeyCode::Char('t')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        sidebar_visible = !sidebar_visible;
                    }
                    KeyCode::Char('c')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        break;
                    }
                    KeyCode::Up | KeyCode::PageUp => {
                        let mut off = scroll_offset.lock().unwrap();
                        let step = if key.code == KeyCode::PageUp { 20 } else { 3 };
                        *off = (*off + step).min(10_000);
                    }
                    KeyCode::Down | KeyCode::PageDown => {
                        let mut off = scroll_offset.lock().unwrap();
                        let step = if key.code == KeyCode::PageDown { 20 } else { 3 };
                        *off = off.saturating_sub(step);
                    }
                    KeyCode::Char(c) => {
                        user_input.push(c);
                    }
                    KeyCode::Backspace => {
                        user_input.pop();
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    Terminal::restore()?;
    Ok(())
}

fn mode_str(mode: agent::AgentMode) -> String {
    match mode {
        agent::AgentMode::Normal => "build".into(),
        agent::AgentMode::Plan => "plan".into(),
    }
}

fn get_git_branch(root: &Path) -> Option<String> {
    let repo = git2::Repository::open(root).ok()?;
    repo.head().ok()?.shorthand().map(String::from)
}

fn is_git_dirty(root: &Path) -> bool {
    if let Ok(repo) = git2::Repository::open(root) {
        if let Ok(statuses) = repo.statuses(None) {
            return !statuses.is_empty();
        }
    }
    false
}

fn load_rust_analyzer_config(root: &Path) -> serde_json::Value {
    let ra_toml = root.join(".rust-analyzer.toml");
    if !ra_toml.exists() {
        return serde_json::json!({
            "rustc": { "source": "discover" },
            "diagnostics": { "enable": true }
        });
    }

    std::fs::read_to_string(&ra_toml)
        .ok()
        .and_then(|content| toml::from_str::<toml::Value>(&content).ok())
        .map(|v| serde_json::to_value(v).ok())
        .flatten()
        .unwrap_or_else(|| {
            serde_json::json!({
                "rustc": { "source": "discover" },
                "diagnostics": { "enable": true }
            })
        })
}

fn session_path(project_root: &Path, session_dir: &Path) -> PathBuf {
    let safe: String = project_root
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    session_dir.join(format!("{}.json", safe))
}

struct SessionEntry {
    name: String,
    path: PathBuf,
    turns: usize,
}

fn session_list(session_dir: &Path) -> Vec<SessionEntry> {
    let mut entries = Vec::new();
    let Ok(dir) = std::fs::read_dir(session_dir) else {
        return entries;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let turns = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("turns").and_then(|t| t.as_array().map(|a| a.len())))
                .unwrap_or(0);
            entries.push(SessionEntry { name, path, turns });
        }
    }
    entries.sort_by(|a, b| {
        b.path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .cmp(&a.path.metadata().ok().and_then(|m| m.modified().ok()))
    });
    entries
}
