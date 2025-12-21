mod exec;
mod impls;
mod pipeline;

pub use exec::{
    IdeEffect, PipelineEvent, PipelineExecution, PipelinePhase,
    ToolCall, ToolDecision, ToolEffect, ToolEvent, ToolExecutor,
};
pub use impls::{EditFileTool, FetchUrlTool, OpenFileTool, ReadFileTool, ShellTool, TaskTool, WebSearchTool, WriteFileTool};
pub use pipeline::{
    ComposableTool, Effect, EffectContext, EffectResult, PipelineBuilder,
    SuspendReason, ToolPipeline,
};

use crate::ide::ToolPreview;
use crate::transcript::Block;
use anyhow::Result;
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::sync::Arc;

/// Result of a tool execution
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    /// Effects to apply after tool completion (spawn agents, IDE commands, etc.)
    pub effects: Vec<ToolEffect>,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            effects: vec![],
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            is_error: true,
            effects: vec![],
        }
    }

    /// Add effects to a result
    pub fn with_effects(mut self, effects: Vec<ToolEffect>) -> Self {
        self.effects = effects;
        self
    }
}

/// Output from a streaming tool execution
#[derive(Debug)]
pub enum ToolOutput {
    /// Partial output (streamed)
    Delta(String),
    /// Execution complete
    Done(ToolResult),
}

/// Trait for tool implementations
pub trait Tool: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &'static str;

    /// Get the tool description
    fn description(&self) -> &'static str;

    /// Get the JSON schema for the tool's parameters
    fn schema(&self) -> serde_json::Value;

    /// Execute the tool, returning a stream of output
    /// 
    /// The stream yields `Delta` for partial output and ends with `Done`.
    /// For non-streaming tools, just yield a single `Done`.
    fn execute(&self, params: serde_json::Value) -> BoxStream<'static, ToolOutput>;

    /// Create a block for displaying this tool call in the TUI
    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block>;

    /// Generate a preview for IDE display before execution
    ///
    /// Tools that modify files should return a preview (diff, file content, etc.)
    /// so the user can see what will change in their editor.
    fn ide_preview(&self, _params: &serde_json::Value) -> Option<ToolPreview> {
        None
    }
}

/// Helper to create a single-item stream for non-streaming tools
pub fn once_ready(result: Result<ToolResult>) -> BoxStream<'static, ToolOutput> {
    let output = match result {
        Ok(r) => ToolOutput::Done(r),
        Err(e) => ToolOutput::Done(ToolResult::error(e.to_string())),
    };
    Box::pin(futures::stream::once(async move { output }))
}

/// Registry of available tools
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new tool registry with all default tools
    pub fn new() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(EditFileTool));
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool::new()));
        registry.register(Arc::new(WebSearchTool::new()));
        registry.register(Arc::new(OpenFileTool));
        registry.register(Arc::new(TaskTool));

        registry
    }

    /// Create a tool registry with only read-only tools (for sub-agents)
    /// Does not include the task tool to prevent infinite agent spawning
    pub fn read_only() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool::new()));
        registry.register(Arc::new(WebSearchTool::new()));
        registry.register(Arc::new(OpenFileTool));

        registry
    }

    /// Create an empty tool registry (no tools)
    pub fn empty() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> &dyn Tool {
        self.tools
            .get(name)
            .map(|t| t.as_ref())
            .expect("unknown tool")
    }

    /// Get a cloneable Arc to a tool by name (for spawning tasks)
    pub fn get_arc(&self, name: &str) -> Arc<dyn Tool> {
        self.tools
            .get(name)
            .cloned()
            .expect("unknown tool")
    }

    /// List all tools
    pub fn values(&self) -> impl Iterator<Item = &dyn Tool> {
        self.tools.values().map(|t| t.as_ref())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
