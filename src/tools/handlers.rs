//! Effect handlers
//!
//! Each handler is a struct that knows how to execute a specific effect.
//! Handlers are stateless - they receive data and produce a Step result.

use crate::tools::io;
use crate::tools::pipeline::{Effect, EffectHandler, Step};

// =============================================================================
// Control flow handlers
// =============================================================================

/// Set pipeline output
pub struct Output {
    pub content: String,
}

#[async_trait::async_trait]
impl EffectHandler for Output {
    async fn call(self: Box<Self>) -> Step {
        Step::Output(self.content)
    }
}

/// Emit streaming delta
pub struct Delta {
    pub content: String,
}

#[async_trait::async_trait]
impl EffectHandler for Delta {
    async fn call(self: Box<Self>) -> Step {
        Step::Delta(self.content)
    }
}

/// Fail the pipeline
pub struct Error {
    pub message: String,
}

#[async_trait::async_trait]
impl EffectHandler for Error {
    async fn call(self: Box<Self>) -> Step {
        Step::Error(self.message)
    }
}

// =============================================================================
// Shell handler
// =============================================================================

/// Execute a shell command
pub struct Shell {
    pub command: String,
    pub working_dir: Option<String>,
    pub timeout_secs: u64,
}

#[async_trait::async_trait]
impl EffectHandler for Shell {
    async fn call(self: Box<Self>) -> Step {
        match io::execute_shell(&self.command, self.working_dir.as_deref(), self.timeout_secs).await
        {
            Ok(result) if result.success => Step::Output(result.output),
            Ok(result) => Step::Output(result.output), // Still output, but includes exit code
            Err(e) => Step::Error(e),
        }
    }
}

// =============================================================================
// Network handlers
// =============================================================================

/// Fetch content from a URL
pub struct FetchUrl {
    pub url: String,
    pub max_length: Option<usize>,
}

#[async_trait::async_trait]
impl EffectHandler for FetchUrl {
    async fn call(self: Box<Self>) -> Step {
        match io::fetch_url(&self.url, self.max_length).await {
            Ok(result) => {
                let header = format!(
                    "[URL: {}]\n[Content-Type: {}]\n[Size: {} bytes]\n\n",
                    self.url, result.content_type, result.size
                );
                Step::Output(header + &result.content)
            }
            Err(e) => Step::Error(e),
        }
    }
}

/// Search the web
pub struct WebSearch {
    pub query: String,
    pub count: u32,
}

#[async_trait::async_trait]
impl EffectHandler for WebSearch {
    async fn call(self: Box<Self>) -> Step {
        match io::web_search(&self.query, self.count).await {
            Ok(results) => {
                if results.is_empty() {
                    Step::Output("No results found.".to_string())
                } else {
                    let output = results
                        .iter()
                        .enumerate()
                        .map(|(i, r)| format!("{}. [{}]({})", i + 1, r.title, r.url))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Step::Output(output)
                }
            }
            Err(e) => Step::Error(e),
        }
    }
}


// =============================================================================
// Agent management handlers
// =============================================================================

/// List all spawned sub-agents
pub struct ListAgents;

#[async_trait::async_trait]
impl EffectHandler for ListAgents {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::ListAgents)
    }
}

