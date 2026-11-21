//! Configuration loading and validation
//!
//! This module provides:
//! - `AgentRuntimeConfig` - Runtime configuration for agents (library-public)
//! - `Config` - Full application configuration loaded from config.toml (CLI-only)

#[cfg(feature = "cli")]
#[cfg(feature = "cli")]
use std::path::PathBuf;

#[cfg(feature = "cli")]
use anyhow::{Context, Result};
#[cfg(feature = "cli")]
use serde::{Deserialize, Serialize};


// =============================================================================
// Library-public types (always available)
// =============================================================================

/// Directory name for Codey project-level configuration and data
pub const CODEY_DIR: &str = ".codey";

/// Directory name for storing conversation transcripts
pub const TRANSCRIPTS_DIR: &str = "transcripts";


/// Runtime configuration for an Agent instance.
///
/// This is the public API for library users to configure agents.
/// CLI users typically create this via `AgentRuntimeConfig::foreground(&config)`
/// or `::background(&config)`, while library users construct it directly.
///
/// # Example
///
/// ```
/// use codey::{AgentRuntimeConfig, Backend};
///
/// let config = AgentRuntimeConfig {
///     model: "claude-sonnet-4-5-20250929".to_string(),
///     max_tokens: 8192,
///     thinking_budget: 2_000,
///     max_retries: 5,
///     fast_mode: false,
///     backend: Backend::Claude,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct AgentRuntimeConfig {
    /// Model to request. The `claude` backend only; rockem serves one model per bundle.
    pub model: String,
    // TODO: max_tokens/max_retries are not yet wired to the SDK backend (the
    // binary manages output limits and retries). Retained as config knobs.
    #[allow(dead_code)]
    pub max_tokens: u32,
    pub thinking_budget: u32,
    #[allow(dead_code)]
    pub max_retries: u32,
    /// Enable fast mode (research preview) for lower-latency responses.
    /// Only effective with opus models.
    pub fast_mode: bool,
    /// Which inference backend the agent runs on.
    pub backend: Backend,
}

/// The inference backend an agent runs on.
#[derive(Debug, Clone, Default)]
pub enum Backend {
    /// The `claude` binary via the Agent SDK; the binary owns the agent loop.
    #[default]
    Claude,
    /// A rockem API service at `url`; Codey owns the agent loop and the conversation.
    Rockem { url: String },
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 8192,
            thinking_budget: 2_000,
            max_retries: 5,
            fast_mode: false,
            backend: Backend::Claude,
        }
    }
}

// =============================================================================
// CLI-only types (gated behind "cli" feature)
// =============================================================================

#[cfg(feature = "cli")]
impl AgentRuntimeConfig {
    /// Create runtime config for foreground agent from application Config
    pub fn foreground(config: &Config) -> Self {
        Self {
            model: config.agents.foreground.model.clone(),
            max_tokens: config.agents.foreground.max_tokens,
            thinking_budget: config.agents.foreground.thinking_budget,
            max_retries: config.general.max_retries,
            fast_mode: config.agents.foreground.fast_mode,
            backend: config.backend(&config.agents.foreground),
        }
    }

    /// Create runtime config for background agent from application Config
    pub fn background(config: &Config) -> Self {
        Self {
            model: config.agents.background.model.clone(),
            max_tokens: config.agents.background.max_tokens,
            thinking_budget: config.agents.background.thinking_budget,
            max_retries: config.general.max_retries,
            fast_mode: config.agents.background.fast_mode,
            backend: config.backend(&config.agents.background),
        }
    }
}

/// Main configuration structure loaded from config.toml
#[cfg(feature = "cli")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub agent: AgentPersonaConfig,
    pub agents: AgentsConfig,
    pub ui: UiConfig,
    pub rockem: RockemConfig,
}

#[cfg(feature = "cli")]
impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            agent: AgentPersonaConfig::default(),
            agents: AgentsConfig::default(),
            ui: UiConfig::default(),
            rockem: RockemConfig::default(),
        }
    }
}

#[cfg(feature = "cli")]
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
        Ok(Config::default())
    }

    /// The runtime backend for an agent's configured backend kind.
    pub fn backend(&self, agent: &AgentConfig) -> Backend {
        match agent.backend {
            BackendKind::Claude => Backend::Claude,
            BackendKind::Rockem => Backend::Rockem { url: self.rockem.url.clone() },
        }
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

/// Agent configurations
#[cfg(feature = "cli")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    /// Foreground/primary agent configuration
    pub foreground: AgentConfig,
    /// Background agent configuration (spawned by task tool)
    pub background: AgentConfig,
}

#[cfg(feature = "cli")]
impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            foreground: AgentConfig::default(),
            background: AgentConfig::default(),
        }
    }
}

/// Configuration for an agent
#[cfg(feature = "cli")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Model to use (defaults to claude-opus-4-6)
    pub model: String,
    /// Max tokens for responses
    pub max_tokens: u32,
    /// Thinking budget in tokens
    pub thinking_budget: u32,
    /// Enable fast mode (research preview) for lower-latency responses at higher cost.
    /// Only works with opus-4-6 models. Adds the research-preview-2026-02-01 beta header.
    pub fast_mode: bool,
    /// Inference backend: "claude" (default) or "rockem" (see `[rockem]`).
    pub backend: BackendKind,
}

/// Backend names as written in config.toml.
#[cfg(feature = "cli")]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    #[default]
    Claude,
    Rockem,
}

/// Where the rockem API service is.
#[cfg(feature = "cli")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RockemConfig {
    /// The service's root.
    pub url: String,
}

#[cfg(feature = "cli")]
impl Default for RockemConfig {
    fn default() -> Self {
        Self { url: "https://rockem.a10k.co".to_string() }
    }
}

#[cfg(feature = "cli")]
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 8192,
            thinking_budget: 2_000,
            fast_mode: false,
            backend: BackendKind::Claude,
        }
    }
}

#[cfg(feature = "cli")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub working_dir: Option<PathBuf>,
    pub max_retries: u32,
}

#[cfg(feature = "cli")]
impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            working_dir: None,
            max_retries: 5,
        }
    }
}

/// Agent persona configuration (name and personality)
#[cfg(feature = "cli")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentPersonaConfig {
    /// Custom name for the agent (defaults to "Codey")
    pub name: Option<String>,
    /// Custom system prompt intro/personality (replaces the default "You are Codey..." paragraph)
    pub system_prompt: Option<String>,
}

#[cfg(feature = "cli")]
impl AgentPersonaConfig {
    /// Get the agent name, falling back to default
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(crate::prompts::DEFAULT_AGENT_NAME)
    }
}

#[cfg(feature = "cli")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    pub auto_scroll: bool,
    pub show_tokens: bool,
}

#[cfg(feature = "cli")]
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "base16-ocean.dark".to_string(),
            auto_scroll: true,
            show_tokens: true,
        }
    }
}

#[cfg(all(test, feature = "cli"))]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.agents.foreground.model, "claude-opus-4-6");
    }

    #[test]
    fn test_parse_config() {
        let toml = r#"
[agents.foreground]
model = "claude-opus-4-20250514"
max_tokens = 4096

[ui]
theme = "monokai"
auto_scroll = false
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.agents.foreground.model, "claude-opus-4-20250514");
        assert_eq!(config.ui.theme, "monokai");
    }

    #[test]
    fn test_parse_agent_configs() {
        let toml = r#"
[agents.foreground]
model = "claude-opus-4-5-20251101"
max_tokens = 8192
thinking_budget = 2000

[agents.background]
model = "claude-sonnet-4-20250514"
max_tokens = 4096
thinking_budget = 1024
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.agents.foreground.model, "claude-opus-4-5-20251101");
        assert_eq!(config.agents.background.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_agent_persona_defaults() {
        let config = Config::default();
        assert!(config.agent.name.is_none());
        assert!(config.agent.system_prompt.is_none());
        assert_eq!(config.agent.name(), "Codey");
    }

    #[test]
    fn test_parse_agent_persona() {
        let toml = r#"
[agent]
name = "Jarvis"
system_prompt = "You are Jarvis, Tony Stark's AI assistant."
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.agent.name, Some("Jarvis".to_string()));
        assert_eq!(
            config.agent.system_prompt,
            Some("You are Jarvis, Tony Stark's AI assistant.".to_string())
        );
        assert_eq!(config.agent.name(), "Jarvis");
    }

    #[test]
    fn test_agent_persona_name_only() {
        let toml = r#"
[agent]
name = "Assistant"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.agent.name, Some("Assistant".to_string()));
        assert!(config.agent.system_prompt.is_none());
        assert_eq!(config.agent.name(), "Assistant");
    }
}
