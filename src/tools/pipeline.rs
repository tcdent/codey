//! Effect-based tool definitions
//!
//! Tools are defined as a chain of effect handlers:
//! ```text
//! edit_file = [
//!     IdeOpen,           // Open file in IDE
//!     IdeShowPreview,    // Show diff preview
//!     AwaitApproval,     // <-- user decides here
//!     WriteFile,         // Apply changes
//!     Output,            // Report success
//!     IdeReloadBuffer,   // Refresh IDE
//! ]
//! ```

use crate::ide::{Edit, ToolPreview};
use crate::transcript::Block;
use std::collections::VecDeque;
use std::path::PathBuf;

/// Result of calling an effect handler
pub enum Step {
    /// Continue to next effect
    Continue,
    /// Set pipeline output
    Output(String),
    /// Emit streaming content
    Delta(String),
    /// Delegate effect to app layer
    Delegate(Effect),
    /// Pause and wait for user approval
    AwaitApproval,
    /// Pipeline failed
    Error(String),
}

/// Trait for effect handlers - each effect knows how to execute itself
#[async_trait::async_trait]
pub trait EffectHandler: Send {
    async fn call(self: Box<Self>) -> Step;
}

/// Effects that must be delegated to the app layer
#[derive(Debug, Clone)]
pub enum Effect {
    // === IDE ===
    IdeOpen { path: PathBuf, line: Option<u32>, column: Option<u32> },
    IdeShowPreview { preview: ToolPreview },
    IdeShowDiffPreview { path: PathBuf, edits: Vec<Edit> },
    IdeReloadBuffer { path: PathBuf },
    IdeClosePreview,
    /// Check if IDE buffer has unsaved changes - fails pipeline if dirty
    IdeCheckUnsavedEdits { path: PathBuf },

    // === Agents ===
    SpawnAgent { task: String, context: Option<String> },
    Notify { message: String },
}

/// A tool defined as a chain of effect handlers
pub struct ToolPipeline {
    pub effects: VecDeque<Box<dyn EffectHandler>>,
}

impl std::fmt::Debug for ToolPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolPipeline")
            .field("effects_count", &self.effects.len())
            .finish()
    }
}

impl Default for ToolPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolPipeline {
    pub fn new() -> Self {
        Self {
            effects: VecDeque::new(),
        }
    }

    /// Create a pipeline that immediately errors
    pub fn error(message: impl Into<String>) -> Self {
        use crate::tools::handlers;
        Self::new().then(handlers::Error { message: message.into() })
    }

    /// Add an effect handler to the chain
    pub fn then(mut self, handler: impl EffectHandler + 'static) -> Self {
        self.effects.push_back(Box::new(handler));
        self
    }

    /// Add approval checkpoint
    pub fn await_approval(self) -> Self {
        use crate::tools::handlers;
        self.then(handlers::AwaitApproval)
    }

    /// Pop the next effect handler
    pub fn pop(&mut self) -> Option<Box<dyn EffectHandler>> {
        self.effects.pop_front()
    }
}

/// Tool that composes effect handlers
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;
    fn compose(&self, params: serde_json::Value) -> ToolPipeline;
    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::handlers;

    #[test]
    fn test_pipeline_chain() {
        let pipeline = ToolPipeline::new()
            .then(handlers::ValidateFile { path: "/tmp/test".into() })
            .await_approval()
            .then(handlers::Output { content: "done".into() });

        assert_eq!(pipeline.effects.len(), 3);
    }

    #[test]
    fn test_pipeline_single_handler() {
        let pipeline = ToolPipeline::new()
            .then(handlers::Output { content: "hello".into() });

        assert_eq!(pipeline.effects.len(), 1);
    }
}
