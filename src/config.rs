//! Configuration loading and validation

use crate::tool_filter::ToolFilterConfig;
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
    /// Token threshold at which to trigger context compaction (default: 100,000)
    pub compaction_threshold: u32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-5-20251101".to_string(),
            working_dir: None,
            max_tokens: 8192,
            max_retries: 5,
            compaction_threshold: 100_000,
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
    /// Filter patterns for shell tool (matches against command)
    pub shell: ToolFilterConfig,
    /// Filter patterns for read_file tool (matches against path)
    pub read_file: ToolFilterConfig,
    /// Filter patterns for write_file tool (matches against path)
    pub write_file: ToolFilterConfig,
    /// Filter patterns for edit_file tool (matches against path)
    pub edit_file: ToolFilterConfig,
    /// Filter patterns for fetch_url tool (matches against url)
    pub fetch_url: ToolFilterConfig,
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
            shell: ToolFilterConfig::default(),
            read_file: ToolFilterConfig::default(),
            write_file: ToolFilterConfig::default(),
            edit_file: ToolFilterConfig::default(),
            fetch_url: ToolFilterConfig::default(),
        }
    }
}

impl ToolsConfig {
    /// Build a HashMap of tool filters for compilation
    pub fn filters(&self) -> HashMap<String, ToolFilterConfig> {
        let mut map = HashMap::new();
        map.insert("shell".to_string(), self.shell.clone());
        map.insert("read_file".to_string(), self.read_file.clone());
        map.insert("write_file".to_string(), self.write_file.clone());
        map.insert("edit_file".to_string(), self.edit_file.clone());
        map.insert("fetch_url".to_string(), self.fetch_url.clone());
        map
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    Ask,
    Allow,
    Deny,
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
        dirs::config_dir().map(|p| p.join("codey").join("config.toml"))
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

    #[test]
    fn test_parse_tool_filters() {
        let toml = r#"
[tools.shell]
allow = ["^ls\\b", "^cat\\b"]
deny = ["rm -rf"]

[tools.read_file]
allow = ["\\.rs$"]
deny = ["\\.env$"]
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.tools.shell.allow, vec!["^ls\\b", "^cat\\b"]);
        assert_eq!(config.tools.shell.deny, vec!["rm -rf"]);
        assert_eq!(config.tools.read_file.allow, vec!["\\.rs$"]);
        assert_eq!(config.tools.read_file.deny, vec!["\\.env$"]);
    }
}
