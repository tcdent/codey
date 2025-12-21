mod exec;
mod impls;

pub use exec::{ToolCall, ToolDecision, ToolEvent, ToolExecutor};
pub use impls::{EditFileTool, FetchUrlTool, ReadFileTool, ShellTool, WebSearchTool, WriteFileTool};

use crate::ide::{IdeAction, ToolPreview};
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
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            is_error: true,
        }
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

    /// Get IDE actions to perform after successful execution
    ///
    /// For example, file-modifying tools should return `ReloadBuffer` so the
    /// editor refreshes the changed file.
    fn ide_post_actions(&self, _params: &serde_json::Value) -> Vec<IdeAction> {
        vec![]
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

        registry
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
