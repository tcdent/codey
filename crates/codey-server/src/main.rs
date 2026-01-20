//! Codey WebSocket Server
//!
//! A WebSocket server that exposes the Codey agent for remote access.
//! Uses the same config file format as the CLI (~/.config/codey/config.toml).

mod protocol;
mod server;
mod session;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use codey::{AgentRuntimeConfig, Config, ToolFilters};
use server::{Server, ServerConfig};

/// Codey WebSocket Server
#[derive(Parser, Debug)]
#[command(name = "codey-server")]
#[command(author, version, about = "WebSocket server for the Codey AI coding assistant")]
struct Args {
    /// Address to listen on
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    listen: SocketAddr,

    /// Path to config file (defaults to ~/.config/codey/config.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Path to system prompt file (overrides config)
    #[arg(short, long)]
    system_prompt: Option<PathBuf>,

    /// Model to use (overrides config)
    #[arg(short, long)]
    model: Option<String>,

    /// Log file path
    #[arg(long, default_value = "/tmp/codey-server.log")]
    log_file: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Set up file-based logging
    let log_file = std::fs::File::create(&args.log_file)?;
    tracing_subscriber::registry()
        .with(EnvFilter::new("info,codey=debug"))
        .with(fmt::layer().with_writer(log_file).with_ansi(false))
        .init();

    // Load .env files
    let _ = dotenvy::from_filename(".env");
    if let Some(home) = dirs::home_dir() {
        let _ = dotenvy::from_path(home.join(".env"));
    }

    // Load configuration
    let config = if let Some(path) = &args.config {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?
    } else {
        Config::load()?
    };

    // Compile tool filters from config
    let filters = ToolFilters::compile(&config.tools.filters())
        .context("Failed to compile tool filters")?;

    // Count configured filters for logging
    let filter_count = config.tools.filters()
        .values()
        .filter(|f| !f.allow.is_empty() || !f.deny.is_empty())
        .count();

    // Load system prompt (CLI arg overrides config)
    let system_prompt = match args.system_prompt {
        Some(path) => std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read system prompt: {}", path.display()))?,
        None => "You are a helpful AI coding assistant. Help the user with their programming tasks.".to_string(),
    };

    // Configure agent (CLI arg overrides config)
    let model = args.model.unwrap_or_else(|| config.agents.foreground.model.clone());
    let agent_config = AgentRuntimeConfig {
        model: model.clone(),
        max_tokens: config.agents.foreground.max_tokens,
        thinking_budget: config.agents.foreground.thinking_budget,
        max_retries: config.general.max_retries,
        compaction_thinking_budget: config.general.compaction_thinking_budget,
    };

    let server_config = ServerConfig {
        system_prompt,
        agent_config,
        filters: Arc::new(filters),
    };

    // Print startup message
    eprintln!("Codey WebSocket Server");
    eprintln!("Listening on: ws://{}", args.listen);
    eprintln!("Model: {}", model);
    eprintln!("Tool filters: {} configured", filter_count);
    eprintln!("Log file: {}", args.log_file.display());
    eprintln!();
    eprintln!("Press Ctrl+C to stop");

    // Run server
    let server = Server::new(args.listen, server_config);
    server.run().await
}
