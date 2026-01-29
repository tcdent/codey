//! OAuth authentication for Anthropic API (Claude Max)
#![allow(dead_code)]

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference";

/// Stored OAuth credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub refresh_token: String,
    pub access_token: String,
    pub expires_at: u64,
}

impl OAuthCredentials {
    /// Check if the access token is expired (with 60s buffer)
    pub fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.expires_at < now + 60
    }

    /// Get the auth file path
    pub fn path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("Could not find config directory"))?
            .join("codey");
        std::fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("auth.json"))
    }

    /// Load credentials from disk
    pub fn load() -> Result<Option<Self>> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&path)?;
        let creds: Self = serde_json::from_str(&contents)?;
        Ok(Some(creds))
    }

    /// Save credentials to disk
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }
}

/// PKCE verifier and challenge pair
pub struct PKCE {
    pub verifier: String,
    pub challenge: String,
}

impl PKCE {
    /// Generate a new PKCE pair
    pub fn generate() -> Self {
        // Generate 32 random bytes for verifier
        let random_bytes: [u8; 32] = rand::random();
        let verifier = URL_SAFE_NO_PAD.encode(random_bytes);

        // SHA256 hash of verifier, base64url encoded
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        Self { verifier, challenge }
    }
}

/// Generate the authorization URL for Claude Max OAuth
pub fn generate_auth_url() -> (String, String) {
    let pkce = PKCE::generate();

    let url = format!(
        "https://claude.ai/oauth/authorize?\
        code=true&\
        client_id={}&\
        response_type=code&\
        redirect_uri={}&\
        scope={}&\
        code_challenge={}&\
        code_challenge_method=S256&\
        state={}",
        CLIENT_ID,
        urlencoding::encode(REDIRECT_URI),
        urlencoding::encode(SCOPES),
        pkce.challenge,
        pkce.verifier,
    );

    (url, pkce.verifier)
}

/// Exchange authorization code for tokens
pub async fn exchange_code(code: &str, verifier: &str) -> Result<OAuthCredentials> {
    // Code format is "code#state"
    let parts: Vec<&str> = code.split('#').collect();
    let auth_code = parts[0];
    let state = parts.get(1).copied().unwrap_or("");

    let client = reqwest::Client::new();
    let response = client
        .post("https://console.anthropic.com/v1/oauth/token")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "code": auth_code,
            "state": state,
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": verifier,
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Token exchange failed ({}): {}", status, body));
    }

    let json: serde_json::Value = response.json().await?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let expires_in = json["expires_in"].as_u64().unwrap_or(3600);

    Ok(OAuthCredentials {
        refresh_token: json["refresh_token"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing refresh_token"))?
            .to_string(),
        access_token: json["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing access_token"))?
            .to_string(),
        expires_at: now + expires_in,
    })
}

/// Refresh an expired access token
pub async fn refresh_token(credentials: &OAuthCredentials) -> Result<OAuthCredentials> {
    let client = reqwest::Client::new();
    let response = client
        .post("https://console.anthropic.com/v1/oauth/token")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": credentials.refresh_token,
            "client_id": CLIENT_ID,
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Token refresh failed ({}): {}", status, body));
    }

    let json: serde_json::Value = response.json().await?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let expires_in = json["expires_in"].as_u64().unwrap_or(3600);

    Ok(OAuthCredentials {
        refresh_token: json["refresh_token"]
            .as_str()
            .unwrap_or(&credentials.refresh_token)
            .to_string(),
        access_token: json["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing access_token"))?
            .to_string(),
        expires_at: now + expires_in,
    })
}
