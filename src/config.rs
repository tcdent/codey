//! Configuration loading and validation

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::tool_filter::ToolFilterConfig;
use crate::tools::{
    EditFileTool, FetchUrlTool, ReadFileTool, ShellTool, WebSearchTool, WriteFileTool,
};

/// Directory name for Codey project-level configuration and data
pub const CODEY_DIR: &str = ".codey";

/// Directory name for storing conversation transcripts
pub const TRANSCRIPTS_DIR: &str = "transcripts";

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub agents: AgentsConfig,
    pub auth: AuthConfig,
    pub ui: UiConfig,
    pub tools: ToolsConfig,
    pub ide: IdeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            agents: AgentsConfig::default(),
            auth: AuthConfig::default(),
            ui: UiConfig::default(),
            tools: ToolsConfig::default(),
            ide: IdeConfig::default(),
        }
    }
}

/// Agent configurations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    /// Foreground/primary agent configuration
    pub foreground: AgentConfig,
    /// Background agent configuration (spawned by task tool)
    pub background: AgentConfig,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            foreground: AgentConfig::foreground_default(),
            background: AgentConfig::background_default(),
        }
    }
}

/// Configuration for an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Model to use (defaults to claude-opus-4-5-20251101)
    pub model: String,
    /// Max tokens for responses
    pub max_tokens: u32,
    /// Thinking budget in tokens
    pub thinking_budget: u32,
    /// Tool access level
    pub tool_access: ToolAccess,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::foreground_default()
    }
}

impl AgentConfig {
    /// Default configuration for foreground/primary agent
    pub fn foreground_default() -> Self {
        Self {
            model: "claude-opus-4-5-20251101".to_string(),
            max_tokens: 8192,
            thinking_budget: 2_000,
            tool_access: ToolAccess::Full,
        }
    }

    /// Default configuration for background agents
    pub fn background_default() -> Self {
        Self {
            model: "claude-opus-4-5-20251101".to_string(),
            max_tokens: 4096,
            thinking_budget: 1_024,  // Anthropic minimum is 1024
            tool_access: ToolAccess::ReadOnly,
        }
    }
}

/// Tool access level for agents
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolAccess {
    /// Full access to all tools
    #[default]
    Full,
    /// Read-only tools: read_file, shell, fetch_url, web_search, open_file
    ReadOnly,
    /// No tools
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub working_dir: Option<PathBuf>,
    pub max_retries: u32,
    /// Token threshold at which to trigger context compaction (default: 100,000)
    pub compaction_threshold: u32,
    /// Thinking budget for compaction requests (default: 8,000)
    pub compaction_thinking_budget: u32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            working_dir: None,
            max_retries: 5,
            compaction_threshold: 192_000,
            compaction_thinking_budget: 8_000,
        }
    }
}

/// Runtime configuration for an Agent instance
/// Combines values from AgentConfig and GeneralConfig
#[derive(Debug, Clone)]
pub struct AgentRuntimeConfig {
    pub model: String,
    pub max_tokens: u32,
    pub thinking_budget: u32,
    pub max_retries: u32,
    pub compaction_thinking_budget: u32,
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 8192,
            thinking_budget: 2_000,
            max_retries: 5,
            compaction_thinking_budget: 8_000,
        }
    }
}

impl AgentRuntimeConfig {
    /// Create runtime config for foreground agent
    pub fn foreground(config: &Config) -> Self {
        Self {
            model: config.agents.foreground.model.clone(),
            max_tokens: config.agents.foreground.max_tokens,
            thinking_budget: config.agents.foreground.thinking_budget,
            max_retries: config.general.max_retries,
            compaction_thinking_budget: config.general.compaction_thinking_budget,
        }
    }

    /// Create runtime config for background agent
    pub fn background(config: &Config) -> Self {
        Self {
            model: config.agents.background.model.clone(),
            max_tokens: config.agents.background.max_tokens,
            thinking_budget: config.agents.background.thinking_budget,
            max_retries: config.general.max_retries,
            compaction_thinking_budget: config.general.compaction_thinking_budget,
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
    /// Filter patterns for web_search tool (matches against query)
    pub web_search: ToolFilterConfig,
    /// Filter patterns for list_background_tasks tool (no params - use ".*" to auto-approve)
    pub list_background_tasks: ToolFilterConfig,
    /// Filter patterns for get_background_task tool (matches against task_id)
    pub get_background_task: ToolFilterConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            enabled: vec![
                ReadFileTool::NAME.to_string(),
                WriteFileTool::NAME.to_string(),
                EditFileTool::NAME.to_string(),
                ShellTool::NAME.to_string(),
                FetchUrlTool::NAME.to_string(),
                WebSearchTool::NAME.to_string(),
            ],
            shell: ToolFilterConfig::default(),
            read_file: ToolFilterConfig::default(),
            write_file: ToolFilterConfig::default(),
            edit_file: ToolFilterConfig::default(),
            fetch_url: ToolFilterConfig::default(),
            web_search: ToolFilterConfig::default(),
            list_background_tasks: ToolFilterConfig::default(),
            get_background_task: ToolFilterConfig::default(),
        }
    }
}

impl ToolsConfig {
    /// Build a HashMap of tool filters for compilation
    pub fn filters(&self) -> HashMap<String, ToolFilterConfig> {
        use crate::tools::{GetBackgroundTaskTool, ListBackgroundTasksTool};
        let mut map = HashMap::new();
        map.insert(ShellTool::NAME.to_string(), self.shell.clone());
        map.insert(ReadFileTool::NAME.to_string(), self.read_file.clone());
        map.insert(WriteFileTool::NAME.to_string(), self.write_file.clone());
        map.insert(EditFileTool::NAME.to_string(), self.edit_file.clone());
        map.insert(FetchUrlTool::NAME.to_string(), self.fetch_url.clone());
        map.insert(WebSearchTool::NAME.to_string(), self.web_search.clone());
        map.insert(ListBackgroundTasksTool::NAME.to_string(), self.list_background_tasks.clone());
        map.insert(GetBackgroundTaskTool::NAME.to_string(), self.get_background_task.clone());
        map
    }
}

/// IDE integration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdeConfig {
    pub nvim: NvimConfig,
}

impl Default for IdeConfig {
    fn default() -> Self {
        Self {
            nvim: NvimConfig::default(),
        }
    }
}

/// Neovim integration configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NvimConfig {
    /// Enable neovim integration
    pub enabled: bool,
    /// Explicit socket path (if not set, auto-discovers from tmux or $NVIM_LISTEN_ADDRESS)
    pub socket: Option<PathBuf>,
    /// Show diffs in nvim after file edits
    pub show_diffs: bool,
    /// Auto-reload buffers after file edits
    pub auto_reload: bool,
}

impl Default for NvimConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            socket: None,
            show_diffs: true,
            auto_reload: true,
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

    /// Get the config directory path (~/.config/codey)
    pub fn config_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|p| p.join(".config").join("codey"))
    }

    /// Get the default config file path
    pub fn default_config_path() -> Option<PathBuf> {
        Self::config_dir().map(|p| p.join("config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.agents.foreground.model, "claude-opus-4-5-20251101");
        assert!(config.tools.enabled.contains(&ReadFileTool::NAME.to_string()));
    }

    #[test]
    fn test_parse_config() {
        let toml = r#"
[agents.foreground]
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
        assert_eq!(config.agents.foreground.model, "claude-opus-4-20250514");
        assert_eq!(config.auth.method, AuthMethod::ApiKey);
        assert_eq!(config.ui.theme, "monokai");
    }

    #[test]
    fn test_parse_agent_configs() {
        let toml = r#"
[agents.foreground]
model = "claude-opus-4-5-20251101"
max_tokens = 8192
thinking_budget = 2000
tool_access = "full"

[agents.background]
model = "claude-sonnet-4-20250514"
max_tokens = 4096
thinking_budget = 1024
tool_access = "read_only"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.agents.foreground.model, "claude-opus-4-5-20251101");
        assert_eq!(config.agents.foreground.tool_access, ToolAccess::Full);
        assert_eq!(config.agents.background.model, "claude-sonnet-4-20250514");
        assert_eq!(config.agents.background.tool_access, ToolAccess::ReadOnly);
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
