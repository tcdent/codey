mod edit_file;
mod fetch_url;
mod read_file;
mod shell;
mod web_search;
mod write_file;

pub use edit_file::EditFileTool;
pub use fetch_url::FetchUrlTool;
pub use read_file::ReadFileTool;
pub use shell::ShellTool;
pub use web_search::WebSearchTool;
pub use write_file::WriteFileTool;

use crate::ide::{IdeAction, ToolPreview};
use crate::transcript::Block;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

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

/// Trait for tool implementations
#[async_trait]
pub trait Tool: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &'static str;

    /// Get the tool description
    fn description(&self) -> &'static str;

    /// Get the JSON schema for the tool's parameters
    fn schema(&self) -> serde_json::Value;

    /// Execute the tool with the given parameters
    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult>;

    /// Create a block for displaying this tool call in the TUI
    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block>;

    /// Generate a preview for IDE display before execution
    ///
    /// Tools that modify files should return a preview (diff, file content, etc.)
    /// so the user can see what will change in their editor.
    fn preview(&self, _params: &serde_json::Value) -> Option<ToolPreview> {
        None
    }

    /// Get IDE actions to perform after successful execution
    ///
    /// For example, file-modifying tools should return `ReloadBuffer` so the
    /// editor refreshes the changed file.
    fn post_actions(&self, _params: &serde_json::Value) -> Vec<IdeAction> {
        vec![]
    }
}

/// Registry of available tools
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new tool registry with all default tools
    pub fn new() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Box::new(ReadFileTool));
        registry.register(Box::new(WriteFileTool));
        registry.register(Box::new(EditFileTool));
        registry.register(Box::new(ShellTool::new()));
        registry.register(Box::new(FetchUrlTool::new()));
        registry.register(Box::new(WebSearchTool::new()));

        registry
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> &dyn Tool {
        self.tools
            .get(name)
            .map(|t| t.as_ref())
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
