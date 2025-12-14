//! Authentication module for Codepal

mod api_key;
mod oauth;
mod storage;

pub use api_key::ApiKeyAuth;
pub use oauth::{DeviceCodeResponse, OAuthClient, OAuthTokens};
pub use storage::AuthStorage;

use anyhow::Result;

/// Authentication credentials
#[derive(Debug, Clone)]
pub enum Credentials {
    ApiKey(String),
    OAuth(OAuthTokens),
}

impl Credentials {
    /// Get the authorization header value
    pub fn auth_header(&self) -> String {
        match self {
            Credentials::ApiKey(key) => key.clone(),
            Credentials::OAuth(tokens) => format!("Bearer {}", tokens.access_token),
        }
    }

    /// Check if using API key authentication
    pub fn is_api_key(&self) -> bool {
        matches!(self, Credentials::ApiKey(_))
    }
}
