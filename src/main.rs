mod config;
mod error;
mod project;
mod tools;

use clap::Parser;
use error::Result;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "bytode", about = "Terminal coding agent")]
struct Cli {
    /// One-shot task (no REPL)
    task: Option<String>,

    /// Project root directory
    #[arg(short, long, default_value = ".")]
    project: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "bytode=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let project_root = std::fs::canonicalize(&cli.project)?;
    let config = config::Config::load(&project_root)?;

    let profile = project::ProjectProfile::detect(&project_root, &config)?;
    tracing::info!(
        "detected: {:?} ({:?})",
        profile.primary,
        profile.build_system
    );
    tracing::info!("snapshot:\n{}", profile.snapshot());

    tracing::info!("bytode v0.1.0 — {:?}", cli.task);
    tracing::info!("project root: {}", project_root.display());
    tracing::info!("tool selection: {:?}", config.tools);

    Ok(())
}
