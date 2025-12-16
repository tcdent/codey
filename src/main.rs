mod app;
mod auth;
mod commands;
mod compaction;
mod config;
mod llm;
mod permission;
mod tool_filter;
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

    /// OAuth login - without code prints auth URL, with code exchanges for token
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    login: Option<String>,
}

/// Handle the OAuth login flow
async fn handle_login(code: Option<String>) -> Result<()> {
    match code {
        Some(code) if !code.is_empty() => {
            // Exchange code for tokens
            // Load the verifier from the temp file
            let verifier_path = std::env::temp_dir().join("codey_pkce_verifier");
            let verifier = std::fs::read_to_string(&verifier_path)
                .map_err(|_| anyhow::anyhow!("No pending login. Run 'codey --login' first."))?;
            
            // Clean up verifier file
            let _ = std::fs::remove_file(&verifier_path);

            println!("Exchanging code for tokens...");
            let credentials = auth::exchange_code(&code, &verifier).await?;
            credentials.save()?;
            
            println!("Authenticated successfully!");
            println!("Token saved to: {}", auth::OAuthCredentials::path()?.display());
            Ok(())
        }
        _ => {
            // Generate and print auth URL
            let (url, verifier) = auth::generate_auth_url();
            
            // Save verifier for later exchange
            let verifier_path = std::env::temp_dir().join("codey_pkce_verifier");
            std::fs::write(&verifier_path, &verifier)?;

            println!("Visit this URL to authorize:");
            println!();
            println!("  {}", url);
            println!();
            println!("Then run: codey --login <code>");
            Ok(())
        }
    }
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

    // Handle OAuth login flow
    if let Some(login_arg) = args.login {
        return handle_login(Some(login_arg).filter(|s| !s.is_empty())).await;
    }

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
