//! API key authentication

use anyhow::{Context, Result};

/// API key authentication handler
pub struct ApiKeyAuth {
    api_key: String,
}

impl ApiKeyAuth {
    /// Create from an API key string
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// Load API key from environment variable
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY environment variable not set")?;
        Ok(Self::new(api_key))
    }

    /// Get the API key
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Validate the API key format
    pub fn validate(&self) -> Result<()> {
        if self.api_key.is_empty() {
            anyhow::bail!("API key is empty");
        }
        if !self.api_key.starts_with("sk-ant-") {
            anyhow::bail!("Invalid API key format (should start with 'sk-ant-')");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_validation() {
        let valid = ApiKeyAuth::new("sk-ant-test123".to_string());
        assert!(valid.validate().is_ok());

        let invalid = ApiKeyAuth::new("invalid-key".to_string());
        assert!(invalid.validate().is_err());

        let empty = ApiKeyAuth::new("".to_string());
        assert!(empty.validate().is_err());
    }
}
