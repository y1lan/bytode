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
use std::sync::Arc;
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
    /// One-shot task (runs and exits; omit for REPL)
    task: Option<String>,

    /// Project root directory
    #[arg(short, long, default_value = ".")]
    project: PathBuf,

    /// DeepSeek API key (or DEEPSEEK_API_KEY env var)
    #[arg(long)]
    api_key: Option<String>,

    /// Model name
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
    // Resolve API key: CLI arg > env var
    let api_key = cli.api_key.take()
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
        .unwrap_or_else(|| {
            eprintln!("Error: DEEPSEEK_API_KEY is required. Set via --api-key or DEEPSEEK_API_KEY env var.");
            std::process::exit(1);
        });
    let project_root = std::fs::canonicalize(&cli.project)?;

    // 1. Load config
    let config = Config::load(&project_root)?;

    // 2. Detect project
    let profile = ProjectProfile::detect(&project_root, &config)?;
    tracing::info!("project: {} ({})", profile.primary, profile.build_system);

    // 3. Load .rust-analyzer.toml for LSP options
    let lsp_options = load_rust_analyzer_config(&project_root);

    // 4. Create LSP client
    let lsp_config = LspConfig {
        command: "rust-analyzer".into(),
        project_root: project_root.clone(),
        options: lsp_options,
    };
    let lsp_client = Arc::new(LspClient::new(lsp_config));

    // 5. Build tool registry
    let tools: Vec<ToolEntry> = vec![
        ToolEntry {
            tool: Box::new(ReadFileTool {
                project_root: project_root.clone(),
            }),
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
            tool: Box::new(CheckTool {
                extra_args: config.build.extra_check_flags.clone(),
            }),
            category: ToolCategory::Build,
            availability: ToolAvailability::PrimaryLanguage {
                requires: &["rust"],
            },
        },
        ToolEntry {
            tool: Box::new(DiagnosticsTool {
                lsp: lsp_client.clone(),
            }),
            category: ToolCategory::ReadOnly,
            availability: ToolAvailability::DetectedLanguage {
                languages: &["rust"],
            },
        },
    ];

    let registry = ToolRegistry::new(tools);

    // 6. Extract tool selection from config
    let (enabled_set, disabled_set, is_exact) = match &config.tools {
        config::ToolSelection::Exact { enabled } => {
            (enabled.iter().cloned().collect(), HashSet::new(), true)
        }
        config::ToolSelection::Additive { enable, disable } => {
            (enable.iter().cloned().collect(), disable.iter().cloned().collect(), false)
        }
        config::ToolSelection::None => {
            (HashSet::new(), HashSet::new(), false)
        }
    };

    // 7. Create LLM client
    let llm = DeepSeekClient::new(api_key, cli.model.clone(), None)?;

    // 8. Create agent
    let mut agent = Agent::new(llm, registry, &profile, &enabled_set, &disabled_set, is_exact);

    // 9. Get git info for status bar
    let git_branch = get_git_branch(&project_root);
    let git_dirty = is_git_dirty(&project_root);

    let detection_source: String = if matches!(config.tools, config::ToolSelection::None) {
        "auto-detect".into()
    } else {
        ".bytode.toml".into()
    };

    // 10. Run
    let result = match cli.task {
        Some(task) => {
            println!("⏳ {}", task);
            let output = agent.run_turn(&task).await?;
            println!("{}", match output {
                agent::AgentOutput::Text(t) => t,
            });
            Ok(())
        }
        None => {
            run_repl(&mut agent, &cli.model, &profile, git_branch, git_dirty, &detection_source).await
        }
    };

    // 11. Shutdown
    lsp_client.shutdown().await;
    result
}

async fn run_repl(
    agent: &mut Agent,
    model: &str,
    profile: &ProjectProfile,
    git_branch: Option<String>,
    git_dirty: bool,
    detection_source: &str,
) -> Result<()> {
    use crossterm::event::{Event, KeyCode, KeyEventKind};

    let mut terminal = Terminal::init()?;
    let mut user_input = String::new();
    let mut sidebar_visible = false;
    let mut last_output: Option<String> = None;
    let mut last_tool_result: Option<tools::ToolResult> = None;

    loop {
        let state = UiState {
            sidebar_visible,
            tool_names: agent.tool_names(),
            status: StatusBarState {
                dir: std::env::current_dir()
                    .map(|d| d.to_string_lossy().to_string())
                    .unwrap_or_default(),
                git_branch: git_branch.clone(),
                git_dirty,
                task: if last_output.is_some() { "⏳ idle" } else { "⏳ idle" }.into(),
                model: model.to_string(),
                ctx_used: agent.context_used(),
                ctx_total: agent.context_total(),
                tool_count: agent.tool_names().len(),
            },
            tool_result: last_tool_result.clone(),
            llm_output: last_output.clone(),
            user_input: user_input.clone(),
            approval: None,
            primary_language: format!("{} [{}]", profile.primary, profile.build_system),
            detection_source: detection_source.to_string(),
        };

        terminal.draw(|frame| ui::layout::render_ui(frame, &state))?;

        if let Ok(event) = crossterm::event::read() {
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Enter => {
                        let input = std::mem::take(&mut user_input);
                        if input.is_empty() {
                            continue;
                        }

                        let mut status_state = state.status.clone();
                        status_state.task = "🔍 thinking...".into();

                        terminal.draw(|frame| {
                            let s = UiState {
                                status: status_state,
                                ..state.clone()
                            };
                            ui::layout::render_ui(frame, &s);
                        })?;

                        match agent.run_turn(&input).await {
                            Ok(agent::AgentOutput::Text(t)) => {
                                last_output = Some(t);
                                last_tool_result = None;
                            }
                            Err(e) => {
                                last_output = Some(format!("Error: {}", e));
                                last_tool_result = None;
                            }
                        }
                    }
                    KeyCode::Char('t') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                        sidebar_visible = !sidebar_visible;
                    }
                    KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                        break;
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
