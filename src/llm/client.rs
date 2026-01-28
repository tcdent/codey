//! Client builder with OpenRouter support
//!
//! This module provides a unified client builder that routes requests to different
//! providers based on model name prefixes:
//!
//! - `openrouter::anthropic/claude-3.5-sonnet` → OpenRouter API
//! - `claude-sonnet-4-20250514` → Native Anthropic API (default)
//!
//! # Environment Variables
//!
//! - `OPENROUTER_API_KEY` - Required for OpenRouter models
//! - `ANTHROPIC_API_KEY` - Required for native Anthropic models

use genai::adapter::AdapterKind;
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden, ServiceTarget};

/// OpenRouter API endpoint
const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1/";

/// Prefix for OpenRouter models in config
pub const OPENROUTER_PREFIX: &str = "openrouter::";

/// Build a genai Client with OpenRouter routing support.
///
/// Models prefixed with `openrouter::` are routed to the OpenRouter API.
/// All other models use genai's default provider resolution.
///
/// # Example
///
/// ```ignore
/// // In config.toml:
/// // model = "openrouter::anthropic/claude-3.5-sonnet"
///
/// let client = build_client();
/// // Requests to this model go to OpenRouter
/// ```
pub fn build_client() -> Client {
    let target_resolver = ServiceTargetResolver::from_resolver_fn(
        |service_target: ServiceTarget| -> Result<ServiceTarget, genai::resolver::Error> {
            let ServiceTarget { ref model, .. } = service_target;
            
            // Check for openrouter:: prefix
            if let Some(actual_model) = model.model_name.strip_prefix(OPENROUTER_PREFIX) {
                let endpoint = Endpoint::from_static(OPENROUTER_ENDPOINT);
                let auth = AuthData::from_env("OPENROUTER_API_KEY");
                // OpenRouter uses OpenAI-compatible API
                let model = ModelIden::new(AdapterKind::OpenAI, actual_model);
                return Ok(ServiceTarget { endpoint, auth, model });
            }
            
            // Return unchanged for default provider resolution
            Ok(service_target)
        },
    );

    Client::builder()
        .with_service_target_resolver(target_resolver)
        .build()
}

/// Check if a model name is an OpenRouter model
pub fn is_openrouter_model(model: &str) -> bool {
    model.starts_with(OPENROUTER_PREFIX)
}

/// Strip the openrouter:: prefix from a model name, if present
pub fn strip_openrouter_prefix(model: &str) -> &str {
    model.strip_prefix(OPENROUTER_PREFIX).unwrap_or(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_openrouter_model() {
        assert!(is_openrouter_model("openrouter::anthropic/claude-3.5-sonnet"));
        assert!(is_openrouter_model("openrouter::openai/gpt-4o"));
        assert!(!is_openrouter_model("claude-sonnet-4-20250514"));
        assert!(!is_openrouter_model("gpt-4o"));
    }

    #[test]
    fn test_strip_openrouter_prefix() {
        assert_eq!(
            strip_openrouter_prefix("openrouter::anthropic/claude-3.5-sonnet"),
            "anthropic/claude-3.5-sonnet"
        );
        assert_eq!(
            strip_openrouter_prefix("claude-sonnet-4-20250514"),
            "claude-sonnet-4-20250514"
        );
    }
}
