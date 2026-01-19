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
#[cfg(feature = "cli")]
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
    
    // === Background Tasks ===
    ListBackgroundTasks,
    GetBackgroundTask { task_id: String },
}

/// When an effect should run
enum EffectTiming {
    /// Normal effect - skipped on deny/error
    Normal(Box<dyn EffectHandler>),
    /// Cleanup effect - always runs
    Finally(Box<dyn EffectHandler>),
}

/// A tool defined as a chain of effect handlers
pub struct ToolPipeline {
    effects: VecDeque<EffectTiming>,
}

impl std::fmt::Debug for ToolPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let normal = self.effects.iter().filter(|e| matches!(e, EffectTiming::Normal(_))).count();
        let finally = self.effects.iter().filter(|e| matches!(e, EffectTiming::Finally(_))).count();
        f.debug_struct("ToolPipeline")
            .field("normal_effects", &normal)
            .field("finally_effects", &finally)
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

    /// Create a pipeline that immediately errors (CLI only)
    #[cfg(feature = "cli")]
    pub fn error(message: impl Into<String>) -> Self {
        use crate::tools::handlers;
        Self::new().then(handlers::Error { message: message.into() })
    }

    /// Create a pipeline that immediately errors (library stub)
    #[cfg(not(feature = "cli"))]
    pub fn error(_message: impl Into<String>) -> Self {
        // Library users handle tools via ToolRequest, so this is never called
        Self::new()
    }

    /// Add an effect handler to the chain
    pub fn then(mut self, handler: impl EffectHandler + 'static) -> Self {
        self.effects.push_back(EffectTiming::Normal(Box::new(handler)));
        self
    }

    /// Add approval checkpoint (CLI only)
    #[cfg(feature = "cli")]
    pub fn await_approval(self) -> Self {
        use crate::tools::handlers;
        self.then(handlers::AwaitApproval)
    }

    /// Add a cleanup effect that always runs (success, error, or deny)
    pub fn finally(mut self, handler: impl EffectHandler + 'static) -> Self {
        self.effects.push_back(EffectTiming::Finally(Box::new(handler)));
        self
    }

    /// Pop the next effect handler
    pub fn pop(&mut self) -> Option<Box<dyn EffectHandler>> {
        self.effects.pop_front().map(|e| match e {
            EffectTiming::Normal(h) | EffectTiming::Finally(h) => h,
        })
    }

    /// Skip to finally effects (for deny/error - removes all Normal effects)
    pub fn skip_to_finally(&mut self) {
        self.effects.retain(|e| matches!(e, EffectTiming::Finally(_)));
    }
    
    /// Check if pipeline has no more effects
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Number of remaining effects (for testing)
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.effects.len()
    }
}

/// Tool that composes effect handlers
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;
    fn compose(&self, params: serde_json::Value) -> ToolPipeline;
    #[cfg(feature = "cli")]
    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block>;
}

#[cfg(all(test, feature = "cli"))]
mod tests {
    use super::*;
    use crate::tools::handlers;

    #[test]
    fn test_pipeline_chain() {
        let pipeline = ToolPipeline::new()
            .then(handlers::ValidateFile { path: "/tmp/test".into() })
            .await_approval()
            .then(handlers::Output { content: "done".into() });

        assert_eq!(pipeline.len(), 3);
    }

    #[test]
    fn test_pipeline_single_handler() {
        let pipeline = ToolPipeline::new()
            .then(handlers::Output { content: "hello".into() });

        assert_eq!(pipeline.len(), 1);
    }

    #[test]
    fn test_skip_to_finally() {
        let mut pipeline = ToolPipeline::new()
            .then(handlers::ValidateFile { path: "/tmp/test".into() })
            .then(handlers::Output { content: "done".into() })
            .finally(handlers::Output { content: "cleanup".into() });

        assert_eq!(pipeline.len(), 3);
        pipeline.skip_to_finally();
        assert_eq!(pipeline.len(), 1);  // only the finally effect remains
    }
}
