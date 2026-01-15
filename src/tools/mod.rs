//! Tool system with effect-based composition
//!
//! Tools are defined as chains of effects that get interpreted by the executor.

mod exec;
pub mod handlers;
mod impls;
mod io;
mod pipeline;

use std::collections::HashMap;
use std::sync::Arc;

pub use exec::{ToolCall, ToolDecision, ToolEvent, ToolExecutor};
pub use impls::{
    EditFileTool, FetchHtmlTool, FetchUrlTool, GetBackgroundTaskTool, ListBackgroundTasksTool,
    OpenFileTool, ReadFileTool, ShellTool, TaskTool, WebSearchTool, WriteFileTool,
};
pub use pipeline::{Effect, Step, Tool};

/// Registry of available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
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
        registry.register(Arc::new(FetchHtmlTool));
        registry.register(Arc::new(WebSearchTool));
        registry.register(Arc::new(OpenFileTool));
        registry.register(Arc::new(TaskTool));
        registry.register(Arc::new(ListBackgroundTasksTool));
        registry.register(Arc::new(GetBackgroundTaskTool));

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
