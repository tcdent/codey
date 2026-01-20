//! Codey WebSocket Server
//!
//! A WebSocket server that exposes the Codey agent for remote access.

mod protocol;
mod server;
mod session;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use codey::AgentRuntimeConfig;
use server::{Server, ServerConfig};

/// Codey WebSocket Server
#[derive(Parser, Debug)]
#[command(name = "codey-server")]
#[command(author, version, about = "WebSocket server for the Codey AI coding assistant")]
struct Args {
    /// Address to listen on
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    listen: SocketAddr,

    /// Path to system prompt file (optional)
    #[arg(short, long)]
    system_prompt: Option<PathBuf>,

    /// Model to use
    #[arg(short, long, default_value = "claude-sonnet-4-20250514")]
    model: String,

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

    // Load system prompt
    let system_prompt = match args.system_prompt {
        Some(path) => std::fs::read_to_string(&path)?,
        None => "You are a helpful AI coding assistant. Help the user with their programming tasks.".to_string(),
    };

    // Configure agent
    let agent_config = AgentRuntimeConfig {
        model: args.model,
        ..Default::default()
    };

    let config = ServerConfig {
        system_prompt,
        agent_config,
    };

    // Print startup message
    eprintln!("Codey WebSocket Server");
    eprintln!("Listening on: ws://{}", args.listen);
    eprintln!("Log file: {}", args.log_file.display());
    eprintln!();
    eprintln!("Press Ctrl+C to stop");

    // Run server
    let server = Server::new(args.listen, config);
    server.run().await
}
