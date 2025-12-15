mod app;
mod config;
mod llm;
mod permission;
mod tools;
mod transcript;
mod ui;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
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
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up file-based logging
    let log_file = std::fs::File::create("/tmp/codey.log")?;
    tracing_subscriber::registry()
        .with(EnvFilter::new("debug"))
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

    // Set working directory
    if let Some(ref working_dir) = config.general.working_dir {
        std::env::set_current_dir(working_dir)?;
    }

    // Run the application
    let mut app = app::App::new(config, args.r#continue).await?;
    app.run().await
}
