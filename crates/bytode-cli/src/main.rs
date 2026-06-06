#![allow(dead_code)]

mod repl;
mod ui;

pub use bytode_agent as agent;
pub use bytode_common::{config, error, project};
pub use bytode_llm as llm;
pub use bytode_lsp as lsp;
pub use bytode_tools as tools;

use agent::{Agent, AgentInit, InteractiveApprovalChannel};
use clap::Parser;
use config::Config;
use error::Result;
use llm::DeepSeekClient;
use lsp::{LspClient, LspConfig};
use project::ProjectProfile;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tools::{BuiltinToolConfig, BuiltinToolProvider, ToolProvider, ToolRegistry};
use ui::panels::HistoryEntry;

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

    let builtin_config = BuiltinToolConfig {
        confirm_before_write: config.agent.confirm_before_write,
        max_file_size: config.security.max_file_size,
        forbidden_write_patterns: config.security.forbidden_write_patterns.clone(),
        ignore_dirs: config.search.ignore_dirs.clone(),
        max_results: config.search.max_results,
        extra_check_flags: config.build.extra_check_flags.clone(),
        web_timeout_secs: config.web_search.timeout_secs,
        web_proxy: config.web_search.proxy.clone(),
    };
    let providers: Vec<Box<dyn ToolProvider>> = vec![Box::new(BuiltinToolProvider::new(
        project_root.clone(),
        builtin_config,
        Some(lsp_client.clone()),
    ))];
    let registry = ToolRegistry::from_providers(providers);
    let (enabled_set, disabled_set, is_exact) = match &config.tools {
        config::ToolSelection::Exact { enabled } => {
            (enabled.iter().cloned().collect(), HashSet::new(), true)
        }
        config::ToolSelection::Additive { enable, disable } => (
            enable.iter().cloned().collect(),
            disable.iter().cloned().collect(),
            false,
        ),
        config::ToolSelection::None => (HashSet::new(), HashSet::new(), false),
    };

    let llm = DeepSeekClient::new(api_key, cli.model.clone(), None)?;
    // Interactive mode lets the user pick or create a session at startup;
    // one-shot task mode always uses the default per-project session.
    let session_id = if cli.task.is_none() {
        pick_session(&project_root)?
    } else {
        agent::session::default_session_id(&project_root)
    };
    let session_root = agent::session::session_root(&session_id)?;

    // Route tracing logs to a file inside the session directory. This is a TUI
    // program — nothing diagnostic may reach the terminal. The worker guard
    // must outlive the program, so it is held until the end of `main`.
    let _log_guard = init_session_logging(&session_root)?;

    let mut agent = Agent::new(
        llm,
        registry,
        &profile,
        AgentInit {
            enabled: &enabled_set,
            disabled: &disabled_set,
            is_exact,
            session_id,
            session_root,
            forbidden_write_patterns: config.security.forbidden_write_patterns.clone(),
        },
    )?;

    // Restore conversation history from the session log.
    let chat_history_init: Vec<HistoryEntry> = agent.replay_chat_history().unwrap_or_else(|e| {
        eprintln!("Note: could not replay session history: {e}");
        Vec::new()
    });

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
            let (approval_channel, approval_rx) = InteractiveApprovalChannel::new();
            agent.set_approval_channel(approval_channel.clone());
            repl::run(
                agent,
                approval_channel,
                approval_rx,
                &profile,
                git_branch,
                git_dirty,
                &detection_source,
                chat_history_init,
            )
            .await;
        }
    };

    lsp_client.shutdown().await;
    Ok(())
}

/// Initialize tracing to write to `<session_root>/bytode.log`. Returns the
/// non-blocking worker guard, which must be kept alive for logs to flush.
fn init_session_logging(
    session_root: &Path,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(session_root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(session_root.join("bytode.log"))?;
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bytode=info".into()),
        )
        .init();
    Ok(guard)
}

/// Present the startup session picker on the terminal. Lists existing sessions
/// for this project (most recent first) plus a "new session" option. Empty
/// input selects the most recent session; an empty list creates a new one.
fn pick_session(project_root: &Path) -> Result<agent::session::SessionId> {
    use std::io::BufRead;

    let sessions = agent::session::list_project_sessions(project_root)?;
    if sessions.is_empty() {
        return Ok(agent::session::default_session_id(project_root));
    }

    println!("Select a session:");
    println!("  [0] New session");
    for (i, s) in sessions.iter().enumerate() {
        let when = s
            .last_activity
            .map(format_timestamp)
            .unwrap_or_else(|| "unknown".into());
        let msg = s.first_user_message.as_deref().unwrap_or("(empty)");
        println!("  [{}] {}  {}  {}", i + 1, s.session_id.as_str(), when, msg);
    }
    print!("Choice [Enter = most recent]: ");
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let choice = line.trim();

    if choice.is_empty() {
        // Most recent — the list is sorted most-recent-first.
        return Ok(sessions[0].session_id.clone());
    }

    let n: usize = choice
        .parse()
        .map_err(|_| error::BytodeError::Session(format!("invalid session choice: {choice}")))?;
    if n == 0 {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        return Ok(agent::session::new_session_id(project_root, secs));
    }
    sessions
        .get(n - 1)
        .map(|s| s.session_id.clone())
        .ok_or_else(|| error::BytodeError::Session(format!("session {n} out of range")))
}

/// Format a unix-seconds timestamp as a `YYYY-MM-DD HH:MM` string (UTC).
fn format_timestamp(secs: i64) -> String {
    // Civil date from days since epoch (Howard Hinnant's algorithm).
    let total_secs = secs.max(0);
    let days = total_secs / 86_400;
    let rem = total_secs % 86_400;
    let (hour, minute) = (rem / 3600, (rem % 3600) / 60);

    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        year, month, day, hour, minute
    )
}

fn get_git_branch(root: &Path) -> Option<String> {
    let repo = git2::Repository::open(root).ok()?;
    repo.head().ok()?.shorthand().map(String::from)
}

fn is_git_dirty(root: &Path) -> bool {
    if let Ok(repo) = git2::Repository::open(root)
        && let Ok(statuses) = repo.statuses(None)
    {
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
