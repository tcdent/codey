//! Tool system with effect-based composition
//!
//! Tools are defined as chains of effects that get interpreted by the executor.

mod exec;
mod impls;
mod pipeline;

pub use exec::{ToolCall, ToolDecision, ToolEvent, ToolExecutor};
pub use impls::{
    EditFileTool, FetchUrlTool, OpenFileTool, ReadFileTool, ShellTool, TaskTool,
    WebSearchTool, WriteFileTool,
};
pub use pipeline::{ComposableTool, Effect, ToolPipeline};

use crate::ide::ToolPreview;
use crate::transcript::Block;
use std::collections::HashMap;
use std::sync::Arc;

/// Result of a tool execution (used for compatibility)
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub effects: Vec<Effect>,
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

    pub fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }
}

/// Registry of available tools
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ComposableTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(EditFileTool));
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool));
        registry.register(Arc::new(WebSearchTool));
        registry.register(Arc::new(OpenFileTool));
        registry.register(Arc::new(TaskTool));

        registry
    }

    pub fn read_only() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(ShellTool::new()));
        registry.register(Arc::new(FetchUrlTool));
        registry.register(Arc::new(WebSearchTool));
        registry.register(Arc::new(OpenFileTool));

        registry
    }

    pub fn empty() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn ComposableTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> &dyn ComposableTool {
        self.tools
            .get(name)
            .map(|t| t.as_ref())
            .expect("unknown tool")
    }

    pub fn get_arc(&self, name: &str) -> Arc<dyn ComposableTool> {
        self.tools
            .get(name)
            .cloned()
            .expect("unknown tool")
    }

    pub fn values(&self) -> impl Iterator<Item = &dyn ComposableTool> {
        self.tools.values().map(|t| t.as_ref())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
