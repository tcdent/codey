//! Provider-specific request customization.
//!
//! The [`RequestCustomizer`] trait defines how each LLM provider builds
//! headers, body params, and system prompt blocks. The agent loop calls
//! these methods to assemble the final request without knowing
//! provider-specific details.
//!
//! Each provider lives in its own submodule:
//! - [`anthropic`] — Anthropic API (key, OAuth, fast mode)
//! - [`openrouter`] — OpenRouter (OpenAI-compatible)

mod anthropic;
mod openrouter;

pub use anthropic::{AnthropicFastMode, AnthropicKey, AnthropicOAuth};
pub use openrouter::OpenRouter;

use genai::chat::{ChatOptions, ChatRequest};

/// Provider-specific request customization.
///
/// Implementors modify [`ChatOptions`] and [`ChatRequest`] before they're
/// sent to the LLM. The agent loop constructs base options with common
/// settings (max_tokens, capture flags) and passes them here for
/// provider-specific adjustments (headers, extra body, reasoning config,
/// body serializer, system prompt injection, etc.)
///
/// # Adding a new provider
///
/// 1. Create a new submodule under `provider/`
/// 2. Define a struct with any config it needs
/// 3. Implement `RequestCustomizer`
/// 4. Add construction logic to `Agent::resolve_provider()` in `agent.rs`
pub trait RequestCustomizer: Send + Sync {
    /// Label for logging (e.g. `"anthropic-fast"`, `"openrouter"`).
    fn label(&self) -> &str;

    /// Apply provider-specific settings to chat options.
    ///
    /// Called with base options already configured with `max_tokens`,
    /// `capture_usage`, and `capture_tool_calls`. The provider adds
    /// headers, extra_body, body_serializer, reasoning config, etc.
    ///
    /// `thinking_budget` is the token budget from the agent's mode config,
    /// used by providers that support extended thinking.
    fn apply_chat_options(&self, options: ChatOptions, thinking_budget: u32) -> ChatOptions;

    /// Apply provider-specific modifications to the chat request.
    ///
    /// Called after the request is built with messages and tools.
    /// Override to inject system prompt blocks, modify messages, etc.
    /// Default is identity (no modification).
    fn apply_chat_request(&self, request: ChatRequest) -> ChatRequest {
        request
    }
}
