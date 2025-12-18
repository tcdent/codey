//! Tool parameter filtering with regex patterns
//!
//! Provides auto-approve and auto-deny functionality for tool calls based on
//! regex patterns matched against the tool's primary parameter.
//!
//! # Configuration Format
//!
//! ```toml
//! [tools.shell]
//! allow = ["^ls\\b", "^find\\b"]
//! deny = ["rm\\s+-rf\\s+/", "sudo\\s+rm"]
//!
//! [tools.read_file]
//! allow = ["\\.(rs|md|toml)$"]
//! deny = ["\\.env$"]
//! ```
//!
//! Each tool has a primary parameter that patterns match against:
//! - shell: `command`
//! - read_file: `path`
//! - write_file: `path`
//! - edit_file: `path`
//! - fetch_url: `url`
//!
//! # Evaluation Order
//!
//! 1. If any deny pattern matches → `Some(ToolDecision::Deny)`
//! 2. If any allow pattern matches → `Some(ToolDecision::Approve)`
//! 3. Otherwise → `None` (prompt user)

use anyhow::{Context, Result};
use fancy_regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::permission::ToolDecision;

/// Filter configuration with allow and deny pattern lists
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolFilterConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

/// Compiled filter for efficient repeated matching
#[derive(Debug)]
pub struct CompiledToolFilter {
    pub allow: Vec<Regex>,
    pub deny: Vec<Regex>,
}

impl CompiledToolFilter {
    /// Compile a tool filter configuration into regex patterns
    pub fn compile(tool_name: &str, config: &ToolFilterConfig) -> Result<Self> {
        let allow = config
            .allow
            .iter()
            .map(|p| Regex::new(p).with_context(|| format!("Invalid allow pattern for {}: {}", tool_name, p)))
            .collect::<Result<Vec<_>>>()?;

        let deny = config
            .deny
            .iter()
            .map(|p| Regex::new(p).with_context(|| format!("Invalid deny pattern for {}: {}", tool_name, p)))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self { allow, deny })
    }

    /// Evaluate patterns against a value
    pub fn evaluate(&self, value: &str) -> Option<ToolDecision> {
        // Check deny patterns first
        for pattern in &self.deny {
            if pattern.is_match(value).unwrap_or(false) {
                return Some(ToolDecision::Deny);
            }
        }

        // Check allow patterns
        for pattern in &self.allow {
            if pattern.is_match(value).unwrap_or(false) {
                return Some(ToolDecision::Approve);
            }
        }

        None
    }
}

/// Get the primary parameter name for a tool
fn primary_param(tool_name: &str) -> &'static str {
    match tool_name {
        "shell" => "command",
        "read_file" => "path",
        "write_file" => "path",
        "edit_file" => "path",
        "fetch_url" => "url",
        _ => "command", // Default fallback
    }
}

/// Collection of compiled filters for all tools
#[derive(Debug, Default)]
pub struct ToolFilters {
    tools: HashMap<String, CompiledToolFilter>,
}

impl ToolFilters {
    /// Compile all tool filter configurations
    pub fn compile(configs: &HashMap<String, ToolFilterConfig>) -> Result<Self> {
        let mut tools = HashMap::new();

        for (tool_name, config) in configs {
            // Skip empty configs
            if config.allow.is_empty() && config.deny.is_empty() {
                continue;
            }
            let compiled = CompiledToolFilter::compile(tool_name, config)?;
            tools.insert(tool_name.clone(), compiled);
        }

        Ok(Self { tools })
    }

    /// Evaluate filters for a specific tool
    pub fn evaluate(&self, tool_name: &str, params: &serde_json::Value) -> Option<ToolDecision> {
        let filter = self.tools.get(tool_name)?;

        // Get the primary parameter value for this tool
        let param_name = primary_param(tool_name);
        let value = match params.get(param_name) {
            Some(serde_json::Value::String(s)) => s.as_str(),
            Some(v) => {
                // For non-string values, convert to string
                let s = v.to_string();
                return filter.evaluate(&s);
            }
            None => return None,
        };

        filter.evaluate(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_allow_pattern_match() {
        let config = ToolFilterConfig {
            allow: vec![r"^ls\b".to_string(), r"^cat\b".to_string()],
            deny: vec![],
        };
        let filter = CompiledToolFilter::compile("shell", &config).unwrap();

        assert_eq!(filter.evaluate("ls -la"), Some(ToolDecision::Approve));
        assert_eq!(filter.evaluate("cat file.txt"), Some(ToolDecision::Approve));
        assert_eq!(filter.evaluate("rm -rf /"), None);
    }

    #[test]
    fn test_deny_pattern_match() {
        let config = ToolFilterConfig {
            allow: vec![],
            deny: vec![r"rm\s+-rf\s+/".to_string(), r"sudo\s+rm".to_string()],
        };
        let filter = CompiledToolFilter::compile("shell", &config).unwrap();

        assert_eq!(filter.evaluate("rm -rf /"), Some(ToolDecision::Deny));
        assert_eq!(filter.evaluate("sudo rm -rf"), Some(ToolDecision::Deny));
        assert_eq!(filter.evaluate("ls -la"), None);
    }

    #[test]
    fn test_deny_takes_precedence() {
        let config = ToolFilterConfig {
            allow: vec![r"^ls".to_string()],
            deny: vec![r"sudo".to_string()],
        };
        let filter = CompiledToolFilter::compile("shell", &config).unwrap();

        // "sudo ls" matches both allow (^ls) and deny (sudo), deny wins
        assert_eq!(filter.evaluate("sudo ls"), Some(ToolDecision::Deny));
        assert_eq!(filter.evaluate("ls -la"), Some(ToolDecision::Approve));
    }

    #[test]
    fn test_tool_filter_with_params() {
        let config = ToolFilterConfig {
            allow: vec![r"^ls\b".to_string()],
            deny: vec![r"rm\s+-rf".to_string()],
        };

        let mut configs = HashMap::new();
        configs.insert("shell".to_string(), config);
        let filters = ToolFilters::compile(&configs).unwrap();

        assert_eq!(
            filters.evaluate("shell", &json!({"command": "ls -la"})),
            Some(ToolDecision::Approve)
        );
        assert_eq!(
            filters.evaluate("shell", &json!({"command": "rm -rf /"})),
            Some(ToolDecision::Deny)
        );
        assert_eq!(
            filters.evaluate("shell", &json!({"command": "echo hello"})),
            None
        );
    }

    #[test]
    fn test_read_file_filter() {
        let config = ToolFilterConfig {
            allow: vec![r"\.rs$".to_string()],
            deny: vec![r"\.env$".to_string()],
        };

        let mut configs = HashMap::new();
        configs.insert("read_file".to_string(), config);
        let filters = ToolFilters::compile(&configs).unwrap();

        assert_eq!(
            filters.evaluate("read_file", &json!({"path": "src/main.rs"})),
            Some(ToolDecision::Approve)
        );
        assert_eq!(
            filters.evaluate("read_file", &json!({"path": ".env"})),
            Some(ToolDecision::Deny)
        );
        assert_eq!(
            filters.evaluate("read_file", &json!({"path": "README.md"})),
            None
        );
    }

    #[test]
    fn test_missing_param() {
        let config = ToolFilterConfig {
            allow: vec![r"^ls\b".to_string()],
            deny: vec![],
        };

        let mut configs = HashMap::new();
        configs.insert("shell".to_string(), config);
        let filters = ToolFilters::compile(&configs).unwrap();

        // Missing "command" param should result in None
        assert_eq!(
            filters.evaluate("shell", &json!({"other_param": "value"})),
            None
        );
    }

    #[test]
    fn test_invalid_regex() {
        let config = ToolFilterConfig {
            allow: vec![r"[invalid".to_string()],
            deny: vec![],
        };
        let result = CompiledToolFilter::compile("shell", &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_tool() {
        let config = ToolFilterConfig {
            allow: vec![r"^ls\b".to_string()],
            deny: vec![],
        };

        let mut configs = HashMap::new();
        configs.insert("shell".to_string(), config);
        let filters = ToolFilters::compile(&configs).unwrap();

        // Unknown tool returns None
        assert_eq!(
            filters.evaluate("unknown_tool", &json!({"command": "ls"})),
            None
        );
    }

    #[test]
    fn test_empty_config_skipped() {
        let mut configs = HashMap::new();
        configs.insert("shell".to_string(), ToolFilterConfig::default());
        let filters = ToolFilters::compile(&configs).unwrap();

        // Empty config means no filter registered
        assert_eq!(
            filters.evaluate("shell", &json!({"command": "ls"})),
            None
        );
    }
}
