//! Tool system with effect-based composition
//!
//! Tools are defined as chains of effects that get interpreted by the executor.
//!
//! For library users, see [`SimpleTool`] for a way to define tools without
//! implementing the full pipeline.

mod exec;
#[cfg(feature = "cli")]
pub mod browser;
#[cfg(feature = "cli")]
pub mod handlers;
#[cfg(feature = "cli")]
mod impls;
mod io;
mod pipeline;

/// Tool name constants (always available for configuration)
pub mod names {
    pub const READ_FILE: &str = "mcp_read_file";
    pub const WRITE_FILE: &str = "mcp_write_file";
    pub const EDIT_FILE: &str = "mcp_edit_file";
    pub const SHELL: &str = "mcp_shell";
    pub const FETCH_URL: &str = "mcp_fetch_url";
    pub const FETCH_HTML: &str = "mcp_fetch_html";
    pub const WEB_SEARCH: &str = "mcp_web_search";
    pub const OPEN_FILE: &str = "mcp_open_file";
    pub const SPAWN_AGENT: &str = "mcp_spawn_agent";
    pub const LIST_BACKGROUND_TASKS: &str = "mcp_list_background_tasks";
    pub const GET_BACKGROUND_TASK: &str = "mcp_get_background_task";
    pub const LIST_AGENTS: &str = "mcp_list_agents";
    pub const GET_AGENT: &str = "mcp_get_agent";
}

use std::collections::HashMap;
use std::sync::Arc;

pub use crate::effect::EffectResult;
pub use exec::{ToolCall, ToolDecision, ToolEvent, ToolExecutor};
#[cfg(feature = "cli")]
pub use impls::{
    init_agent_context, update_agent_oauth, EditFileTool, FetchHtmlTool, FetchUrlTool,
    GetAgentTool, GetBackgroundTaskTool, ListAgentsTool, ListBackgroundTasksTool, OpenFileTool,
    ReadFileTool, ShellTool, SpawnAgentTool, WebSearchTool, WriteFileTool,
};
#[cfg(feature = "cli")]
pub use browser::init_browser_context;
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

/// Registry of available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a full registry with all tools (CLI only)
    #[cfg(feature = "cli")]
    pub fn new() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(EditFileTool));
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool));
        registry.register(Arc::new(FetchHtmlTool));
        registry.register(Arc::new(WebSearchTool));
        registry.register(Arc::new(OpenFileTool));
        registry.register(Arc::new(SpawnAgentTool));
        registry.register(Arc::new(ListBackgroundTasksTool));
        registry.register(Arc::new(GetBackgroundTaskTool));
        registry.register(Arc::new(ListAgentsTool));
        registry.register(Arc::new(GetAgentTool));

        registry
    }

    /// Tools available to sub-agents (includes write tools - approval routed to user) (CLI only)
    ///
    /// TODO: Sub-agent tools shouldn't have a `background` parameter since sub-agents
    /// are already non-blocking. We should either filter it out of the schema or
    /// create separate tool variants for sub-agents.
    #[cfg(feature = "cli")]
    pub fn subagent() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(EditFileTool));
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool));
        registry.register(Arc::new(FetchHtmlTool));
        registry.register(Arc::new(WebSearchTool));
        registry.register(Arc::new(OpenFileTool));

        registry
    }

    #[cfg(feature = "cli")]
    pub fn read_only() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool));
        registry.register(Arc::new(FetchHtmlTool));
        registry.register(Arc::new(WebSearchTool));
        registry.register(Arc::new(OpenFileTool));

        registry
    }

    pub fn empty() -> Self {
        Self {
            tools: HashMap::new(),
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
