//! Credential storage for authentication tokens

use super::OAuthTokens;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stored authentication data
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredAuth {
    pub anthropic: Option<ProviderAuth>,
}

/// Provider-specific authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderAuth {
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at: i64,
    },
    ApiKey {
        api_key: String,
    },
}

impl From<OAuthTokens> for ProviderAuth {
    fn from(tokens: OAuthTokens) -> Self {
        ProviderAuth::OAuth {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
        }
    }
}

impl ProviderAuth {
    /// Convert to OAuthTokens if this is OAuth auth
    pub fn to_oauth_tokens(&self) -> Option<OAuthTokens> {
        match self {
            ProviderAuth::OAuth {
                access_token,
                refresh_token,
                expires_at,
            } => Some(OAuthTokens {
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                expires_at: *expires_at,
                token_type: "Bearer".to_string(),
            }),
            ProviderAuth::ApiKey { .. } => None,
        }
    }
}

/// Authentication storage handler
pub struct AuthStorage {
    path: PathBuf,
}

impl AuthStorage {
    /// Create a new auth storage
    pub fn new() -> Result<Self> {
        let config_dir = crate::config::Config::ensure_config_dir()?;
        let path = config_dir.join("auth.json");
        Ok(Self { path })
    }

    /// Create auth storage with a custom path
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Load stored authentication
    pub fn load(&self) -> Result<StoredAuth> {
        if !self.path.exists() {
            return Ok(StoredAuth::default());
        }

        let content = std::fs::read_to_string(&self.path)
            .with_context(|| format!("Failed to read auth file: {}", self.path.display()))?;

        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse auth file: {}", self.path.display()))
    }

    /// Save authentication
    pub fn save(&self, auth: &StoredAuth) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create auth directory: {}", parent.display())
            })?;
        }

        let content = serde_json::to_string_pretty(auth)
            .context("Failed to serialize auth data")?;

        std::fs::write(&self.path, content)
            .with_context(|| format!("Failed to write auth file: {}", self.path.display()))?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, permissions)?;
        }

        Ok(())
    }

    /// Save Anthropic OAuth tokens
    pub fn save_anthropic_oauth(&self, tokens: OAuthTokens) -> Result<()> {
        let mut auth = self.load().unwrap_or_default();
        auth.anthropic = Some(ProviderAuth::from(tokens));
        self.save(&auth)
    }

    /// Get stored Anthropic OAuth tokens
    pub fn get_anthropic_oauth(&self) -> Result<Option<OAuthTokens>> {
        let auth = self.load()?;
        Ok(auth.anthropic.and_then(|p| p.to_oauth_tokens()))
    }

    /// Clear stored authentication
    pub fn clear(&self) -> Result<()> {
        if self.path.exists() {
            std::fs::remove_file(&self.path)
                .with_context(|| format!("Failed to remove auth file: {}", self.path.display()))?;
        }
        Ok(())
    }
}

impl Default for AuthStorage {
    fn default() -> Self {
        Self::new().expect("Failed to create auth storage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_auth_storage() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let storage = AuthStorage::with_path(path);

        // Initially empty
        let auth = storage.load().unwrap();
        assert!(auth.anthropic.is_none());

        // Save tokens
        let tokens = OAuthTokens {
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            expires_at: 1234567890000,
            token_type: "Bearer".to_string(),
        };
        storage.save_anthropic_oauth(tokens.clone()).unwrap();

        // Load tokens
        let loaded = storage.get_anthropic_oauth().unwrap().unwrap();
        assert_eq!(loaded.access_token, "test_access");
        assert_eq!(loaded.refresh_token, "test_refresh");

        // Clear
        storage.clear().unwrap();
        assert!(storage.get_anthropic_oauth().unwrap().is_none());
    }
}
