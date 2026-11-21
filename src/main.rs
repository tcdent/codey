mod app;
mod commands;
mod config;
mod effect;
mod llm;
mod notifications;
#[cfg(feature = "profiling")]
mod profiler;
mod prompts;
mod tools;
mod transcript;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Codey - A terminal-based AI coding assistant
#[derive(Parser, Debug)]
#[command(name = "codey")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Working directory
    #[arg(short, long)]
    working_dir: Option<PathBuf>,

    /// Continue from previous session
    #[arg(short, long)]
    r#continue: bool,

    /// Override the model (e.g., "claude-opus-4-7")
    #[arg(short, long)]
    model: Option<String>,

    /// Enable performance profiling (requires --features profiling)
    /// Exports profile data to the specified path on exit
    #[cfg(feature = "profiling")]
    #[arg(long, value_name = "OUTPUT_PATH")]
    profile: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up file-based logging
    let log_file = std::fs::File::create("/tmp/codey.log")?;
    tracing_subscriber::registry()
        .with(EnvFilter::new("debug,reqwest=off,hyper_util=off,nvim_rs=off,h2=off"))
        .with(fmt::layer().with_writer(log_file).with_ansi(false))
        .init();

    // Load .env files (local first, then home directory)
    // Errors are ignored - files are optional
    let _ = dotenvy::from_filename(".env");
    if let Some(home) = dirs::home_dir() {
        let _ = dotenvy::from_path(home.join(".env"));
    }

    let args = Args::parse();

    // Load configuration
    let mut config = config::Config::load()?;

    // Apply CLI overrides
    if let Some(working_dir) = args.working_dir {
        config.general.working_dir = Some(working_dir);
    }
    if let Some(model) = args.model {
        config.agents.foreground.model = model;
    }

    // Set working directory
    if let Some(ref working_dir) = config.general.working_dir {
        std::env::set_current_dir(working_dir)?;
    }

    // Initialize profiling if enabled
    #[cfg(feature = "profiling")]
    let profile_output = args.profile.clone();
    #[cfg(feature = "profiling")]
    if profile_output.is_some() {
        let terminal_size = crossterm::terminal::size().unwrap_or((80, 24));
        profiler::init(terminal_size);
        tracing::info!("Performance profiling enabled");
    }

    // Run the application
    let mut app = app::App::new(config, args.r#continue).await?;
    let result = app.run().await;

    // Export profiling data if enabled
    #[cfg(feature = "profiling")]
    if let Some(path) = profile_output {
        profiler::stop();
        if let Err(e) = profiler::export_json(&path) {
            tracing::error!("Failed to export profile data: {}", e);
            eprintln!("Failed to export profile data to {}: {}", path.display(), e);
        } else {
            eprintln!("Profile data exported to: {}", path.display());
        }
    }

    result
}
