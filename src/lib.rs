//! Codey - A terminal-based AI coding assistant
//!
//! This crate can be used as a library to create custom AI agents with tool capabilities.
//!
//! # Example
//!
//! ```no_run
//! use codey::{Agent, AgentRuntimeConfig, ToolRegistry};
//! use codey::tools::{ReadFileTool, WriteFileTool, ShellTool};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Build a custom tool registry
//!     let mut tools = ToolRegistry::empty();
//!     tools.register(std::sync::Arc::new(ReadFileTool));
//!     tools.register(std::sync::Arc::new(WriteFileTool));
//!     tools.register(std::sync::Arc::new(ShellTool::new()));
//!
//!     // Create an agent with custom system prompt
//!     let mut agent = Agent::new(
//!         AgentRuntimeConfig::default(),
//!         "You are a helpful coding assistant.",
//!         None, // oauth credentials
//!         tools,
//!     );
//!
//!     // Send a message
//!     agent.send_request("Help me refactor this code", codey::RequestMode::Normal);
//!
//!     // Process responses
//!     while let Some(step) = agent.next().await {
//!         match step {
//!             codey::AgentStep::TextDelta(text) => print!("{}", text),
//!             codey::AgentStep::ToolRequest(calls) => {
//!                 // Handle tool approval and execution
//!             }
//!             codey::AgentStep::Finished { usage } => {
//!                 println!("\nDone! Used {} tokens", usage.output_tokens);
//!                 break;
//!             }
//!             _ => {}
//!         }
//!     }
//! }
//! ```

// Core modules - public for library usage
pub mod config;
pub mod llm;
pub mod tools;

// Supporting modules - exposed for tools that need IDE types
pub mod ide;

// Internal modules - not exposed in library API
mod auth;
mod compaction;
mod tool_filter;
mod transcript;

// Re-export commonly used types at crate root for convenience
pub use config::AgentRuntimeConfig;
pub use llm::{Agent, AgentStep, RequestMode};
pub use tools::{Tool, ToolCall, ToolDecision, ToolRegistry};
