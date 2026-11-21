//! Tool system with effect-based composition
//!
//! Tools are defined as chains of effects that get interpreted by the executor.
//!
//! For library users, see [`SimpleTool`] for a way to define tools without
//! implementing the full pipeline.
#![allow(dead_code)]
#![allow(unused_imports)]

mod exec;
#[cfg(feature = "cli")]
pub mod handlers;
#[cfg(feature = "cli")]
mod impls;
pub mod io;
mod pipeline;

/// Tool name constants (always available for configuration)
pub mod names {
    pub const SHELL: &str = "mcp_shell";
    pub const FETCH_URL: &str = "mcp_fetch_url";
    pub const WEB_SEARCH: &str = "mcp_web_search";
    pub const SPAWN_AGENT: &str = "mcp_spawn_agent";
    pub const LIST_AGENTS: &str = "mcp_list_agents";
}

use std::collections::BTreeMap;
use std::sync::Arc;

pub use crate::effect::EffectResult;
pub use exec::{ToolCall, ToolEvent, ToolExecutor};
#[cfg(feature = "cli")]
pub use impls::{init_agent_context, FetchUrlTool, ListAgentsTool, ShellTool, SpawnAgentTool, WebSearchTool};
pub use pipeline::{Effect, Step, Tool, ToolPipeline};

#[cfg(feature = "cli")]
use crate::transcript::{Block, BlockType, ToolBlock};

/// A simple tool definition for library users.
///
/// This allows defining tools that can be advertised to the LLM without
/// implementing the full effect pipeline. Tool execution is handled by
/// the library user via [`AgentStep::ToolRequest`] and [`Agent::submit_tool_result`].
///
/// # Example
///
/// ```ignore
/// use codey::{SimpleTool, ToolRegistry};
/// use serde_json::json;
/// use std::sync::Arc;
///
/// let weather_tool = SimpleTool::new(
///     "get_weather",
///     "Get the current weather for a location",
///     json!({
///         "type": "object",
///         "properties": {
///             "location": {
///                 "type": "string",
///                 "description": "City name"
///             }
///         },
///         "required": ["location"]
///     }),
/// );
///
/// let mut tools = ToolRegistry::empty();
/// tools.register(Arc::new(weather_tool));
/// ```
pub struct SimpleTool {
    name: &'static str,
    description: &'static str,
    schema: serde_json::Value,
}

impl SimpleTool {
    /// Create a new simple tool definition.
    ///
    /// - `name`: The tool name (used by the LLM to call it)
    /// - `description`: Human-readable description of what the tool does
    /// - `schema`: JSON Schema describing the tool's parameters
    pub fn new(name: &'static str, description: &'static str, schema: serde_json::Value) -> Self {
        Self { name, description, schema }
    }
}

impl Tool for SimpleTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    fn compose(&self, _params: serde_json::Value) -> ToolPipeline {
        // SimpleTool is for library users who handle tool execution themselves.
        // This method should never be called in that context.
        ToolPipeline::error("SimpleTool does not support compose() - handle tool calls via AgentStep::ToolRequest")
    }

    #[cfg(feature = "cli")]
    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        // Return a basic ToolBlock for compatibility
        Box::new(ToolBlock::new(call_id, self.name, params, background))
    }
}

/// Registry of available tools, kept sorted by name: the tool list leads every
/// prompt, so it must come out the same in every process for the model server's
/// prompt cache to match across launches.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a full registry with all tools (CLI only)
    #[cfg(feature = "cli")]
    pub fn new() -> Self {
        let mut registry = Self::subagent();
        registry.register(Arc::new(SpawnAgentTool));
        registry.register(Arc::new(ListAgentsTool));
        registry
    }

    /// Tools available to sub-agents: everything but agent management (CLI only)
    ///
    /// TODO: Sub-agent tools shouldn't have a `background` parameter since sub-agents
    /// are already non-blocking. We should either filter it out of the schema or
    /// create separate tool variants for sub-agents.
    #[cfg(feature = "cli")]
    pub fn subagent() -> Self {
        let mut registry = Self::empty();
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool));
        registry.register(Arc::new(WebSearchTool));
        registry
    }

    pub fn empty() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> &dyn Tool {
        self.tools
            .get(name)
            .map(|t| t.as_ref())
            .expect("unknown tool")
    }

    pub fn get_arc(&self, name: &str) -> Arc<dyn Tool> {
        self.tools
            .get(name)
            .cloned()
            .expect("unknown tool")
    }

    pub fn values(&self) -> impl Iterator<Item = &dyn Tool> {
        self.tools.values().map(|t| t.as_ref())
    }
}

#[cfg(all(test, feature = "cli"))]
mod tests {
    use super::*;

    /// The tool list leads every prompt; it must come out the same in every process.
    #[test]
    fn tools_come_out_sorted_by_name() {
        for registry in [ToolRegistry::new(), ToolRegistry::subagent()] {
            let names: Vec<&str> = registry.values().map(|t| t.name()).collect();
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(names, sorted);
        }
    }
}
