//! Codey - A terminal-based AI coding assistant
//!
//! This crate can be used as a library to create AI agents with custom system prompts.
//!
//! # Example
//!
//! ```no_run
//! use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, ToolRegistry};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create an agent with a custom system prompt
//!     let mut agent = Agent::new(
//!         AgentRuntimeConfig::default(),
//!         "You are a helpful assistant. Answer questions concisely.",
//!         None, // oauth credentials (uses ANTHROPIC_API_KEY env var)
//!         ToolRegistry::empty(),
//!     );
//!
//!     // Send a message
//!     agent.send_request("What is the capital of France?", RequestMode::Normal);
//!
//!     // Process streaming responses
//!     while let Some(step) = agent.next().await {
//!         match step {
//!             AgentStep::TextDelta(text) => print!("{}", text),
//!             AgentStep::ThinkingDelta(_) => { /* extended thinking */ }
//!             AgentStep::Finished { usage } => {
//!                 println!("\n\nTokens used: {}", usage.output_tokens);
//!                 break;
//!             }
//!             AgentStep::Error(e) => {
//!                 eprintln!("Error: {}", e);
//!                 break;
//!             }
//!             _ => {}
//!         }
//!     }
//! }
//! ```

// Core modules (always available)
mod auth;
mod config;
mod ide;
mod llm;
mod tools;
mod transcript;

// CLI-only modules
#[cfg(feature = "cli")]
mod compaction;
#[cfg(feature = "cli")]
mod prompts;
#[cfg(feature = "cli")]
mod tool_filter;

// Re-export the public API
pub use config::AgentRuntimeConfig;
pub use llm::{Agent, AgentStep, RequestMode, Usage};
pub use tools::{SimpleTool, ToolCall, ToolRegistry};
