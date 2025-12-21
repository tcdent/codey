mod exec;
mod impls;
mod pipeline;

pub use exec::{
    PipelineEvent, PipelineExecution, PipelinePhase,
    ToolCall, ToolDecision, ToolEvent, ToolExecutor,
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
    /// Effects to apply after tool completion
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

    pub fn with_effects(mut self, effects: Vec<Effect>) -> Self {
        self.effects = effects;
        self
    }
}

/// Output from a streaming tool execution
#[derive(Debug)]
pub enum ToolOutput {
    Delta(String),
    Done(ToolResult),
}

/// Trait for tool implementations
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;
    fn execute(&self, params: serde_json::Value) -> BoxStream<'static, ToolOutput>;
    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block>;

    /// Generate a preview for IDE display before execution
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

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
