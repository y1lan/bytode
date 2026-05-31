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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tools::{
    check::CheckTool,
    file::{ReadFileTool, WriteFileTool},
    lsp_diag::DiagnosticsTool,
    search::SearchTool,
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
    tracing_subscriber::fmt()
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
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                })
                .await?;
            println!();
        }
        None => {
            run_repl(&mut agent, &cli.model, &profile, git_branch, git_dirty, &detection_source).await?;
        }
    };

    lsp_client.shutdown().await;
    Ok(())
}

async fn run_repl(
    agent: &mut Agent,
    model: &str,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
) -> Result<()> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    let terminal = Arc::new(Mutex::new(Terminal::init()?));
    let mut user_input = String::new();
    let mut sidebar_visible = false;

    // Chat history: each String is one message block
    let chat_history: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let scroll_offset: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

    loop {
        // Build visible text from chat_history + current streaming + scroll offset
        let history = chat_history.lock().unwrap();
        let full_text = history.join("\n");
        let total_lines = full_text.lines().count();
        let offset = *scroll_offset.lock().unwrap();

        // Visible portion: show from (total_lines - offset - visible_lines) to (total_lines - offset)
        // offset 0 = show bottom (newest), offset > 0 = scrolled up
        let visible_lines = 40usize; // approximate visible lines in output area
        let visible_start = total_lines.saturating_sub(offset + visible_lines);
        let visible_text: String = full_text
            .lines()
            .skip(visible_start)
            .take(visible_lines + offset)
            .collect::<Vec<_>>()
            .join("\n");

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
                model: model.to_string(),
                ctx_used: agent.context_used(),
                ctx_total: agent.context_total(),
                tool_count: agent.tool_names().len(),
                session_cost: agent.session_cost(),
                session_calls: agent.session_call_count(),
            },
            tool_result: None,
            llm_output: if visible_text.is_empty() { None } else { Some(visible_text) },
            user_input: user_input.clone(),
            approval: None,
            primary_language: format!("{} [{}]", profile.primary, profile.build_system),
            detection_source: detection_source.to_string(),
        };

        drop(history);

        {
            let mut t = terminal.lock().unwrap();
            t.draw(|frame| ui::layout::render_ui(frame, &state))?;
        }

        if let Ok(event) = crossterm::event::read() {
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Enter => {
                        let input = std::mem::take(&mut user_input);
                        if input.is_empty() {
                            continue;
                        }

                        // Add user message to history
                        {
                            let mut h = chat_history.lock().unwrap();
                            h.push(format!("▸ {}", input));
                        }
                        // Reset scroll to bottom
                        *scroll_offset.lock().unwrap() = 0;

                        // Start collecting streaming response
                        let streaming_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

                        // Capture state for the streaming callback
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
                        let model_s = model.to_string();
                        let dir_s = std::env::current_dir()
                            .map(|d| d.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let gb = git_branch.clone();

                        let result = agent
                            .run_turn_streaming(&input, move |chunk| {
                                streaming_clone.lock().unwrap().push_str(chunk);
                                // Keep scroll at bottom during active streaming
                                *scroll_clone.lock().unwrap() = 0;
                                if let Ok(mut t) = terminal_clone.lock() {
                                    let h = history_clone.lock().unwrap();
                                    let full = h.join("\n");
                                    let streamed = streaming_clone.lock().unwrap().clone();
                                    let display = if streamed.is_empty() {
                                        full
                                    } else {
                                        format!("{}\n{}", full, streamed)
                                    };
                                    let _ = t.draw(|frame| {
                                        let s = UiState {
                                            sidebar_visible,
                                            tool_names: tool_names.clone(),
                                            status: StatusBarState {
                                                dir: dir_s.clone(),
                                                git_branch: gb.clone(),
                                                git_dirty,
                                                task: "thinking...".into(),
                                                model: model_s.clone(),
                                                ctx_used,
                                                ctx_total,
                                                tool_count: tool_names.len(),
                                                session_cost,
                                                session_calls,
                                            },
                                            tool_result: None,
                                            llm_output: Some(display),
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

                        // Add assistant response to history, clear streaming buffer
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
                    // Scroll up/down through history
                    KeyCode::Up | KeyCode::PageUp => {
                        let mut off = scroll_offset.lock().unwrap();
                        let step = if key.code == KeyCode::PageUp { 20 } else { 3 };
                        *off = (*off + step).min(10_000); // cap, will be clamped by render
                    }
                    KeyCode::Down | KeyCode::PageDown => {
                        let mut off = scroll_offset.lock().unwrap();
                        let step = if key.code == KeyCode::PageDown { 20 } else { 3 };
                        *off = off.saturating_sub(step);
                    }
                    KeyCode::Char(c) => {
                        user_input.push(c);
                        if user_input == "/exit" || user_input == "/quit" {
                            break;
                        }
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
