//! Tool parameter filtering with regex patterns
//!
//! Provides auto-approve and auto-deny functionality for tool calls based on
//! regex patterns matched against parameter values.
//!
//! # Configuration Format
//!
//! ```toml
//! [tools.filters.shell]
//! command.allow = ["^ls\\b", "^find\\b"]
//! command.deny = ["rm\\s+-rf\\s+/", "sudo\\s+rm"]
//!
//! [tools.filters.read_file]
//! path.allow = ["\\.(rs|md|toml)$"]
//! path.deny = ["\\.env$"]
//! ```
//!
//! # Evaluation Order
//!
//! 1. If any deny pattern matches → `FilterResult::Deny`
//! 2. If any allow pattern matches → `FilterResult::Allow`
//! 3. Otherwise → `FilterResult::NoMatch` (use permission level)

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result of evaluating filters against tool parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterResult {
    /// A deny pattern matched - block the tool call
    Deny,
    /// An allow pattern matched (no deny matched) - auto-approve
    Allow,
    /// No patterns matched - fall back to permission level
    NoMatch,
}

/// Raw filter configuration as it appears in TOML
/// Maps parameter names to their allow/deny patterns
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolFilterConfig {
    #[serde(flatten)]
    pub params: HashMap<String, ParamFilterConfig>,
}

/// Allow and deny patterns for a single parameter
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ParamFilterConfig {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

/// Compiled filter for efficient repeated matching
#[derive(Debug)]
pub struct CompiledToolFilter {
    pub tool_name: String,
    pub params: HashMap<String, CompiledParamFilter>,
}

/// Compiled regex patterns for a single parameter
#[derive(Debug)]
pub struct CompiledParamFilter {
    pub allow: Vec<Regex>,
    pub deny: Vec<Regex>,
}

impl CompiledToolFilter {
    /// Compile a tool filter configuration into regex patterns
    pub fn compile(tool_name: &str, config: &ToolFilterConfig) -> Result<Self> {
        let mut params = HashMap::new();

        for (param_name, param_config) in &config.params {
            let compiled = CompiledParamFilter::compile(param_config)
                .with_context(|| format!("Failed to compile filter for {}.{}", tool_name, param_name))?;
            params.insert(param_name.clone(), compiled);
        }

        Ok(Self {
            tool_name: tool_name.to_string(),
            params,
        })
    }

    /// Evaluate this filter against tool parameters
    ///
    /// Returns `FilterResult::Deny` if any deny pattern matches any parameter.
    /// Returns `FilterResult::Allow` if any allow pattern matches and no deny matched.
    /// Returns `FilterResult::NoMatch` if no patterns matched.
    pub fn evaluate(&self, params: &serde_json::Value) -> FilterResult {
        let mut any_allow_matched = false;

        for (param_name, param_filter) in &self.params {
            // Get the parameter value as a string
            let value = match params.get(param_name) {
                Some(serde_json::Value::String(s)) => s.as_str(),
                Some(v) => {
                    // For non-string values, convert to JSON string representation
                    // This allows matching against numbers, booleans, etc.
                    let s = v.to_string();
                    return self.evaluate_param(param_filter, &s, &mut any_allow_matched);
                }
                None => continue,
            };

            match param_filter.evaluate(value) {
                FilterResult::Deny => return FilterResult::Deny,
                FilterResult::Allow => any_allow_matched = true,
                FilterResult::NoMatch => {}
            }
        }

        if any_allow_matched {
            FilterResult::Allow
        } else {
            FilterResult::NoMatch
        }
    }

    fn evaluate_param(
        &self,
        param_filter: &CompiledParamFilter,
        value: &str,
        any_allow_matched: &mut bool,
    ) -> FilterResult {
        match param_filter.evaluate(value) {
            FilterResult::Deny => FilterResult::Deny,
            FilterResult::Allow => {
                *any_allow_matched = true;
                FilterResult::NoMatch // Continue checking other params
            }
            FilterResult::NoMatch => FilterResult::NoMatch,
        }
    }
}

impl CompiledParamFilter {
    /// Compile allow and deny patterns into regex
    pub fn compile(config: &ParamFilterConfig) -> Result<Self> {
        let allow = config
            .allow
            .iter()
            .map(|p| Regex::new(p).with_context(|| format!("Invalid allow pattern: {}", p)))
            .collect::<Result<Vec<_>>>()?;

        let deny = config
            .deny
            .iter()
            .map(|p| Regex::new(p).with_context(|| format!("Invalid deny pattern: {}", p)))
            .collect::<Result<Vec<_>>>()?;

        Ok(Self { allow, deny })
    }

    /// Evaluate patterns against a value
    pub fn evaluate(&self, value: &str) -> FilterResult {
        // Check deny patterns first
        for pattern in &self.deny {
            if pattern.is_match(value) {
                return FilterResult::Deny;
            }
        }

        // Check allow patterns
        for pattern in &self.allow {
            if pattern.is_match(value) {
                return FilterResult::Allow;
            }
        }

        FilterResult::NoMatch
    }
}

/// Collection of compiled filters for all tools
#[derive(Debug, Default)]
pub struct ToolFilters {
    pub tools: HashMap<String, CompiledToolFilter>,
}

impl ToolFilters {
    /// Compile all tool filter configurations
    pub fn compile(configs: &HashMap<String, ToolFilterConfig>) -> Result<Self> {
        let mut tools = HashMap::new();

        for (tool_name, config) in configs {
            let compiled = CompiledToolFilter::compile(tool_name, config)?;
            tools.insert(tool_name.clone(), compiled);
        }

        Ok(Self { tools })
    }

    /// Evaluate filters for a specific tool
    pub fn evaluate(&self, tool_name: &str, params: &serde_json::Value) -> FilterResult {
        match self.tools.get(tool_name) {
            Some(filter) => filter.evaluate(params),
            None => FilterResult::NoMatch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_allow_pattern_match() {
        let config = ParamFilterConfig {
            allow: vec![r"^ls\b".to_string(), r"^cat\b".to_string()],
            deny: vec![],
        };
        let filter = CompiledParamFilter::compile(&config).unwrap();

        assert_eq!(filter.evaluate("ls -la"), FilterResult::Allow);
        assert_eq!(filter.evaluate("cat file.txt"), FilterResult::Allow);
        assert_eq!(filter.evaluate("rm -rf /"), FilterResult::NoMatch);
    }

    #[test]
    fn test_deny_pattern_match() {
        let config = ParamFilterConfig {
            allow: vec![],
            deny: vec![r"rm\s+-rf\s+/".to_string(), r"sudo\s+rm".to_string()],
        };
        let filter = CompiledParamFilter::compile(&config).unwrap();

        assert_eq!(filter.evaluate("rm -rf /"), FilterResult::Deny);
        assert_eq!(filter.evaluate("sudo rm -rf"), FilterResult::Deny);
        assert_eq!(filter.evaluate("ls -la"), FilterResult::NoMatch);
    }

    #[test]
    fn test_deny_takes_precedence() {
        let config = ParamFilterConfig {
            allow: vec![r"^ls".to_string()],
            deny: vec![r"sudo".to_string()],
        };
        let filter = CompiledParamFilter::compile(&config).unwrap();

        // "sudo ls" matches both allow (^ls) and deny (sudo), deny wins
        assert_eq!(filter.evaluate("sudo ls"), FilterResult::Deny);
        assert_eq!(filter.evaluate("ls -la"), FilterResult::Allow);
    }

    #[test]
    fn test_tool_filter_evaluation() {
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            ParamFilterConfig {
                allow: vec![r"^ls\b".to_string()],
                deny: vec![r"rm\s+-rf".to_string()],
            },
        );
        let config = ToolFilterConfig { params };
        let filter = CompiledToolFilter::compile("shell", &config).unwrap();

        assert_eq!(
            filter.evaluate(&json!({"command": "ls -la"})),
            FilterResult::Allow
        );
        assert_eq!(
            filter.evaluate(&json!({"command": "rm -rf /"})),
            FilterResult::Deny
        );
        assert_eq!(
            filter.evaluate(&json!({"command": "echo hello"})),
            FilterResult::NoMatch
        );
    }

    #[test]
    fn test_multiple_params() {
        let mut params = HashMap::new();
        params.insert(
            "path".to_string(),
            ParamFilterConfig {
                allow: vec![r"\.rs$".to_string()],
                deny: vec![r"\.env$".to_string()],
            },
        );
        let config = ToolFilterConfig { params };
        let filter = CompiledToolFilter::compile("read_file", &config).unwrap();

        assert_eq!(
            filter.evaluate(&json!({"path": "src/main.rs"})),
            FilterResult::Allow
        );
        assert_eq!(
            filter.evaluate(&json!({"path": ".env"})),
            FilterResult::Deny
        );
        assert_eq!(
            filter.evaluate(&json!({"path": "README.md"})),
            FilterResult::NoMatch
        );
    }

    #[test]
    fn test_missing_param() {
        let mut params = HashMap::new();
        params.insert(
            "command".to_string(),
            ParamFilterConfig {
                allow: vec![r"^ls\b".to_string()],
                deny: vec![],
            },
        );
        let config = ToolFilterConfig { params };
        let filter = CompiledToolFilter::compile("shell", &config).unwrap();

        // Missing "command" param should result in NoMatch
        assert_eq!(
            filter.evaluate(&json!({"other_param": "value"})),
            FilterResult::NoMatch
        );
    }

    #[test]
    fn test_invalid_regex() {
        let config = ParamFilterConfig {
            allow: vec![r"[invalid".to_string()],
            deny: vec![],
        };
        let result = CompiledParamFilter::compile(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_filters_collection() {
        let mut configs = HashMap::new();

        let mut shell_params = HashMap::new();
        shell_params.insert(
            "command".to_string(),
            ParamFilterConfig {
                allow: vec![r"^ls\b".to_string()],
                deny: vec![],
            },
        );
        configs.insert("shell".to_string(), ToolFilterConfig { params: shell_params });

        let filters = ToolFilters::compile(&configs).unwrap();

        assert_eq!(
            filters.evaluate("shell", &json!({"command": "ls -la"})),
            FilterResult::Allow
        );
        assert_eq!(
            filters.evaluate("shell", &json!({"command": "rm -rf"})),
            FilterResult::NoMatch
        );
        // Unknown tool returns NoMatch
        assert_eq!(
            filters.evaluate("unknown_tool", &json!({"command": "ls"})),
            FilterResult::NoMatch
        );
    }
}
