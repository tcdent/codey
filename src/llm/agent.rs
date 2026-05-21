//! Shared agent types: steps, usage, and request modes.
//!
//! The agent backend itself lives in [`super::sdk_agent`] (driving the `claude`
//! binary via claude-agent-sdk-rs). This module holds the types shared between
//! that backend and the rest of Codey (App, transcript, tools).
#![allow(dead_code)]

use crate::config::AgentRuntimeConfig;
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
    }
}

/// Steps yielded by the agent during processing
pub enum AgentStep {
    /// Streaming text chunk
    TextDelta(String),
    /// Streaming thinking/reasoning chunk (extended thinking)
    ThinkingDelta(String),
    /// Streaming compaction summary chunk
    CompactionDelta(String),
    /// Agent wants to execute tools, needs approval
    ToolRequest(Vec<ToolCall>),
    /// Retrying after error
    Retrying { attempt: u32, error: String },
    /// Agent finished processing this message
    Finished { usage: Usage },
    /// Error occurred
    Error(String),
}

/// Request mode controlling agent behavior for a single request
#[derive(Debug, Clone, Copy, Default)]
pub enum RequestMode {
    /// Normal conversation mode with tool access
    #[default]
    Normal,
    /// Compaction mode: no tools, focused on summarization
    Compaction,
}

/// Options derived from a RequestMode
pub struct RequestOptions {
    pub tools_enabled: bool,
    pub thinking_budget: u32,
    pub capture_tool_calls: bool,
}

impl RequestMode {
    pub fn options(&self, config: &AgentRuntimeConfig) -> RequestOptions {
        match self {
            Self::Normal => RequestOptions {
                tools_enabled: true,
                thinking_budget: config.thinking_budget,
                capture_tool_calls: true,
            },
            Self::Compaction => RequestOptions {
                tools_enabled: false,
                thinking_budget: config.compaction_thinking_budget,
                capture_tool_calls: false,
            },
        }
    }
}

/// A function that builds a dynamic system prompt.
/// Called before each LLM request to allow prompt content to change.
pub type SystemPromptBuilder = Box<dyn Fn() -> String + Send + Sync>;
