#![allow(dead_code)]

mod agent;
mod config;
mod error;
mod llm;
mod lsp;
mod project;
mod repl;
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
    git::{GitCommitTool, GitDiffTool, GitLogTool, GitPushTool, GitStatusTool},
    lsp_diag::DiagnosticsTool,
    search::SearchTool,
    web::SearchWebTool,
    ToolAvailability, ToolCategory, ToolEntry, ToolRegistry,
};

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
            tool: Box::new(SearchWebTool {
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
        ToolEntry {
            tool: Box::new(GitCommitTool { project_root: project_root.clone() }),
            category: ToolCategory::Modification,
            availability: ToolAvailability::Always,
        },
        ToolEntry {
            tool: Box::new(GitPushTool { project_root: project_root.clone() }),
            category: ToolCategory::Modification,
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
    let mut session_file = session_path(&project_root, &session_dir);

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
                    // New session: use timestamped filename to avoid overwriting
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let hash = session_path(&project_root, &session_dir)
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    session_file = session_dir.join(format!("{hash}_{:x}.json", ts));
                } else if let Some(s) = sessions.get(choice - 1) {
                    if agent.load_session(&s.path).is_ok() {
                        session_file = s.path.clone();
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
            agent = repl::run(
                agent,
                &profile,
                git_branch,
                git_dirty,
                &detection_source,
                chat_history_init,
            )
            .await;
        }
    };

    let _ = agent.save_session(&session_file, &project_root.to_string_lossy());
    lsp_client.shutdown().await;
    Ok(())
}

fn get_git_branch(root: &Path) -> Option<String> {
    let repo = git2::Repository::open(root).ok()?;
    repo.head().ok()?.shorthand().map(String::from)
}

fn is_git_dirty(root: &Path) -> bool {
    if let Ok(repo) = git2::Repository::open(root)
        && let Ok(statuses) = repo.statuses(None) {
            return !statuses.is_empty();
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
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or_else(|| {
            serde_json::json!({
                "rustc": { "source": "discover" },
                "diagnostics": { "enable": true }
            })
        })
}

fn session_path(project_root: &Path, session_dir: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_root.to_string_lossy().hash(&mut hasher);
    let hash = hasher.finish();
    session_dir.join(format!("{:016x}.json", hash))
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
            let hash = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let name = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| {
                    v.get("project_path")
                        .and_then(|p| p.as_str())
                        .map(|p| {
                            let home = std::env::var("HOME").unwrap_or_default();
                            p.replace(&home, "~")
                        })
                })
                .unwrap_or_else(|| hash.clone());
            let turns = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("turns").and_then(|t| t.as_array().map(|a| a.len())))
                .unwrap_or(0);
            entries.push(SessionEntry {
                name,
                path,
                turns,
            });
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
