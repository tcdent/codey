//! Tool system with effect-based composition
//!
//! Tools are defined as chains of effects that get interpreted by the executor.
//!
//! For library users, see [`SimpleTool`] for a way to define tools without
//! implementing the full pipeline.

mod exec;
pub mod handlers;
mod impls;
mod io;
mod pipeline;

use std::collections::HashMap;
use std::sync::Arc;

pub use exec::{ToolCall, ToolDecision, ToolEvent, ToolExecutor};
pub use impls::{
    init_agent_context, update_agent_oauth, EditFileTool, FetchHtmlTool, FetchUrlTool,
    GetBackgroundTaskTool, ListBackgroundTasksTool, OpenFileTool, ReadFileTool, ShellTool,
    SpawnAgentTool, WebSearchTool, WriteFileTool,
};
pub use pipeline::{Effect, Step, Tool, ToolPipeline};

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
    /// Create a full registry with all tools
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

        registry
    }
    
    /// Tools available to sub-agents (read-only, no spawn_agent)
    pub fn subagent() -> Self {
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
