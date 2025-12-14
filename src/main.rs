//! Codepal - A terminal-based AI coding assistant

mod app;
mod config;

mod auth;
mod llm;
mod tools;
mod ui;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Codepal - A terminal-based AI coding assistant
#[derive(Parser, Debug)]
#[command(name = "codepal")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Model to use (overrides config)
    #[arg(short, long)]
    model: Option<String>,

    /// Working directory
    #[arg(short, long)]
    working_dir: Option<PathBuf>,

    /// API key (overrides config and environment)
    #[arg(long, env = "ANTHROPIC_API_KEY")]
    api_key: Option<String>,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let filter = if args.debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    // Load configuration
    let mut config = config::Config::load(args.config.as_deref())?;

    // Apply CLI overrides
    if let Some(model) = args.model {
        config.general.model = model;
    }
    if let Some(working_dir) = args.working_dir {
        config.general.working_dir = Some(working_dir);
    }
    if let Some(api_key) = args.api_key {
        config.auth.api_key = Some(api_key);
        config.auth.method = config::AuthMethod::ApiKey;
    }

    // Set working directory
    if let Some(ref working_dir) = config.general.working_dir {
        std::env::set_current_dir(working_dir)?;
    }

    // Run the application
    let mut app = app::App::new(config).await?;
    app.run().await
}
