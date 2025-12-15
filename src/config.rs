//! Configuration loading and validation

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub auth: AuthConfig,
    pub ui: UiConfig,
    pub tools: ToolsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            auth: AuthConfig::default(),
            ui: UiConfig::default(),
            tools: ToolsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub model: String,
    pub working_dir: Option<PathBuf>,
    pub max_tokens: u32,
    pub max_retries: u32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-5-20251101".to_string(),
            working_dir: None,
            max_tokens: 8192,
            max_retries: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub method: AuthMethod,
    pub api_key: Option<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            method: AuthMethod::ApiKey,
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    OAuth,
    ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    pub auto_scroll: bool,
    pub show_tokens: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "base16-ocean.dark".to_string(),
            auto_scroll: true,
            show_tokens: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub enabled: Vec<String>,
    pub permissions: HashMap<String, PermissionLevel>,
    pub shell: ShellConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        let mut permissions = HashMap::new();
        permissions.insert("read_file".to_string(), PermissionLevel::Ask);
        permissions.insert("write_file".to_string(), PermissionLevel::Ask);
        permissions.insert("edit_file".to_string(), PermissionLevel::Ask);
        permissions.insert("shell".to_string(), PermissionLevel::Ask);
        permissions.insert("fetch_url".to_string(), PermissionLevel::Ask);

        Self {
            enabled: vec![
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "shell".to_string(),
                "fetch_url".to_string(),
            ],
            permissions,
            shell: ShellConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    Ask,
    Allow,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub allowed_patterns: Vec<String>,
    pub blocked_patterns: Vec<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            allowed_patterns: vec![],
            blocked_patterns: vec!["rm -rf /".to_string(), "sudo rm".to_string()],
        }
    }
}

impl Config {
    /// Load configuration from file, falling back to defaults
    pub fn load() -> Result<Self> {
        if let Some(path) = Self::default_config_path() {
            if path.exists() {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read config file: {}", path.display()))?;
                let config: Config = toml::from_str(&content)
                    .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
                return Ok(config);
            }
        }

        // Return default config if no file found
        Ok(Config::default())
    }

    /// Get the default config file path
    pub fn default_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("codepal").join("config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.general.model, "claude-opus-4-5-20251101");
        assert!(config.tools.enabled.contains(&"read_file".to_string()));
    }

    #[test]
    fn test_parse_config() {
        let toml = r#"
[general]
model = "claude-opus-4-20250514"
max_tokens = 4096

[auth]
method = "api_key"
api_key = "sk-test"

[ui]
theme = "monokai"
auto_scroll = false
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.general.model, "claude-opus-4-20250514");
        assert_eq!(config.auth.method, AuthMethod::ApiKey);
        assert_eq!(config.ui.theme, "monokai");
    }
}
