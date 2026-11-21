//! Shared agent types: steps, usage, and request modes.
//!
//! The backends live in [`super::sdk_agent`] (driving the `claude` binary via
//! claude-agent-sdk-rs) and [`super::rockem_agent`] (Codey's own loop over a rockem
//! service), behind [`super::backend::Agent`]. This module holds the types shared
//! between them and the rest of Codey (App, transcript, tools).
#![allow(dead_code)]

use crate::tools::ToolCall;

/// Token usage tracking
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    /// Cumulative output tokens across the session
    pub output_tokens: u32,
    /// Current context window size (total input tokens for last request)
    pub context_tokens: u32,
    /// Cache creation tokens in last request
    pub cache_creation_tokens: u32,
    /// Cache read tokens in last request
    pub cache_read_tokens: u32,
    /// Generation rate over the last turn, tokens per second; 0 when the backend
    /// doesn't report one.
    pub tokens_per_second: u32,
}

impl Usage {
    /// Format usage information for logging
    pub fn format_log(&self) -> String {
        let mut details = format!("Context: {} tokens", self.context_tokens);

        if self.cache_read_tokens > 0 || self.cache_creation_tokens > 0 {
            details.push_str(&format!(
                " (cached: {}, new: {})",
                self.cache_read_tokens, self.cache_creation_tokens
            ));
        }

        details.push_str(&format!(", output: {}", self.output_tokens));

        details
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.output_tokens += other.output_tokens;
        // These represent current state, not cumulative
        self.context_tokens = other.context_tokens;
        self.cache_creation_tokens = other.cache_creation_tokens;
        self.cache_read_tokens = other.cache_read_tokens;
        self.tokens_per_second = other.tokens_per_second;
    }
}

/// What a running turn is doing, for the activity readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting in line for the model (rockem: before a node takes the request).
    Queued,
    /// Reading the prompt, until the first token.
    Prefill,
    /// Generating reasoning.
    Thinking,
    /// Generating the reply.
    Writing,
    /// Waiting on tool results.
    Tools,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Prefill => "prefill",
            Phase::Thinking => "thinking",
            Phase::Writing => "writing",
            Phase::Tools => "tools",
        }
    }
}

/// A running turn: since when, what it is doing, and the tokens generated so far
/// (`None` when the backend doesn't count them as they arrive).
#[derive(Debug, Clone, Copy)]
pub struct Activity {
    pub started: std::time::Instant,
    pub phase: Phase,
    pub tokens: Option<u32>,
}

/// Steps yielded by the agent during processing
pub enum AgentStep {
    /// Streaming text chunk
    TextDelta(String),
    /// Streaming thinking/reasoning chunk (extended thinking)
    ThinkingDelta(String),
    /// Agent wants to execute tools, needs approval
    ToolRequest(Vec<ToolCall>),
    /// Retrying after error
    Retrying { attempt: u32, error: String },
    /// Agent finished processing this message
    Finished { usage: Usage },
    /// Error occurred
    Error(String),
}

/// Request mode controlling agent behavior for a single request.
///
/// Retained for the public `send_request` signature; only `Normal` is used today.
#[derive(Debug, Clone, Copy, Default)]
pub enum RequestMode {
    /// Normal conversation mode with tool access
    #[default]
    Normal,
}

/// A function that builds a dynamic system prompt.
/// Called before each LLM request to allow prompt content to change.
pub type SystemPromptBuilder = Box<dyn Fn() -> String + Send + Sync>;
