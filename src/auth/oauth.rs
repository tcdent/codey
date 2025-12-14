//! Anthropic OAuth authentication (Device Authorization Grant with PKCE)

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// OAuth configuration
pub struct OAuthConfig {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub device_code_url: String,
    pub scopes: Vec<String>,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            // Using OpenCode's client ID as reference
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string(),
            auth_url: "https://console.anthropic.com/v1/oauth/authorize".to_string(),
            token_url: "https://console.anthropic.com/v1/oauth/token".to_string(),
            device_code_url: "https://console.anthropic.com/v1/oauth/device/code".to_string(),
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "offline_access".to_string(),
            ],
        }
    }
}

/// OAuth tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64, // Unix timestamp in milliseconds
    pub token_type: String,
}

impl OAuthTokens {
    /// Check if the access token is expired
    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        now >= self.expires_at
    }

    /// Check if the token will expire within the given duration
    pub fn expires_within(&self, seconds: i64) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        now + (seconds * 1000) >= self.expires_at
    }
}

/// Device code response from OAuth server
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u32,
    pub interval: u32,
}

/// Token response from OAuth server
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    token_type: String,
}

/// OAuth error response
#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
}

/// PKCE code verifier and challenge
struct PkceChallenge {
    verifier: String,
    challenge: String,
}

impl PkceChallenge {
    fn generate() -> Self {
        // Generate a random 32-byte verifier
        let mut rng = rand::thread_rng();
        let verifier_bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
        let verifier = URL_SAFE_NO_PAD.encode(&verifier_bytes);

        // Create SHA256 hash for challenge
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(&hash);

        Self { verifier, challenge }
    }
}

/// OAuth client for Anthropic authentication
pub struct OAuthClient {
    config: OAuthConfig,
    http_client: reqwest::Client,
    pkce: PkceChallenge,
}

impl OAuthClient {
    /// Create a new OAuth client
    pub fn new() -> Self {
        Self {
            config: OAuthConfig::default(),
            http_client: reqwest::Client::new(),
            pkce: PkceChallenge::generate(),
        }
    }

    /// Start the device authorization flow
    pub async fn start_device_flow(&self) -> Result<DeviceCodeResponse> {
        let response = self
            .http_client
            .post(&self.config.device_code_url)
            .json(&serde_json::json!({
                "client_id": self.config.client_id,
                "scope": self.config.scopes.join(" "),
                "code_challenge": self.pkce.challenge,
                "code_challenge_method": "S256"
            }))
            .send()
            .await
            .context("Failed to start device flow")?;

        if !response.status().is_success() {
            let error: OAuthError = response.json().await.unwrap_or(OAuthError {
                error: "unknown".to_string(),
                error_description: Some("Unknown error occurred".to_string()),
            });
            anyhow::bail!(
                "Device flow failed: {} - {}",
                error.error,
                error.error_description.unwrap_or_default()
            );
        }

        response
            .json()
            .await
            .context("Failed to parse device code response")
    }

    /// Poll for token after user authorization
    pub async fn poll_for_token(&self, device_code: &str) -> Result<OAuthTokens> {
        let response = self
            .http_client
            .post(&self.config.token_url)
            .json(&serde_json::json!({
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                "device_code": device_code,
                "client_id": self.config.client_id,
                "code_verifier": self.pkce.verifier
            }))
            .send()
            .await
            .context("Failed to poll for token")?;

        if !response.status().is_success() {
            let error: OAuthError = response.json().await.unwrap_or(OAuthError {
                error: "unknown".to_string(),
                error_description: None,
            });

            // Check for specific OAuth errors
            match error.error.as_str() {
                "authorization_pending" => {
                    anyhow::bail!("authorization_pending");
                }
                "slow_down" => {
                    anyhow::bail!("slow_down");
                }
                "expired_token" => {
                    anyhow::bail!("Device code expired. Please restart the authentication flow.");
                }
                "access_denied" => {
                    anyhow::bail!("Access denied by user.");
                }
                _ => {
                    anyhow::bail!(
                        "Token exchange failed: {} - {}",
                        error.error,
                        error.error_description.unwrap_or_default()
                    );
                }
            }
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .context("Failed to parse token response")?;

        let expires_at =
            chrono::Utc::now().timestamp_millis() + (token_response.expires_in as i64 * 1000);

        Ok(OAuthTokens {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at,
            token_type: token_response.token_type,
        })
    }

    /// Refresh an expired access token
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<OAuthTokens> {
        let response = self
            .http_client
            .post(&self.config.token_url)
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": self.config.client_id
            }))
            .send()
            .await
            .context("Failed to refresh token")?;

        if !response.status().is_success() {
            let error: OAuthError = response.json().await.unwrap_or(OAuthError {
                error: "unknown".to_string(),
                error_description: None,
            });
            anyhow::bail!(
                "Token refresh failed: {} - {}",
                error.error,
                error.error_description.unwrap_or_default()
            );
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .context("Failed to parse token response")?;

        let expires_at =
            chrono::Utc::now().timestamp_millis() + (token_response.expires_in as i64 * 1000);

        Ok(OAuthTokens {
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at,
            token_type: token_response.token_type,
        })
    }
}

impl Default for OAuthClient {
    fn default() -> Self {
        Self::new()
    }
}
