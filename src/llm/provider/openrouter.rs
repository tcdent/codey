//! OpenRouter provider.
//!
//! OpenRouter is an OpenAI-compatible proxy. Requests get referer/title
//! headers for attribution but no reasoning or Anthropic-specific features.

use genai::chat::ChatOptions;
use genai::Headers;

use super::RequestCustomizer;

/// OpenRouter provider (OpenAI-compatible proxy).
pub struct OpenRouter;

impl RequestCustomizer for OpenRouter {
    fn label(&self) -> &str {
        "openrouter"
    }

    fn apply_chat_options(&self, options: ChatOptions, _thinking_budget: u32) -> ChatOptions {
        options.with_extra_headers(Headers::from([
            (
                "HTTP-Referer".to_string(),
                "https://github.com/tcdent/codey".to_string(),
            ),
            ("X-Title".to_string(), "Codey".to_string()),
        ]))
        // No reasoning support — thinking_budget intentionally ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openrouter_headers() {
        let provider = OpenRouter;
        assert_eq!(provider.label(), "openrouter");

        let options = provider.apply_chat_options(ChatOptions::default(), 2000);
        // Verify headers are set (we can't easily inspect ChatOptions internals,
        // but we can verify it doesn't panic and returns)
        let _ = options;
    }
}
