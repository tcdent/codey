//! Effect-based tool definitions
//!
//! Tools are defined as a chain of effects:
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

use crate::ide::ToolPreview;
use crate::transcript::Block;
use std::path::PathBuf;

/// Effects that compose a tool's behavior
#[derive(Debug, Clone)]
pub enum Effect {
    // === Validation ===
    ValidateParams { error: Option<String> },
    ValidateFileExists { path: PathBuf },
    ValidateFileReadable { path: PathBuf },
    Validate { ok: bool, error: String },

    // === IDE ===
    IdeOpen { path: PathBuf, line: Option<u32>, column: Option<u32> },
    IdeShowPreview { preview: ToolPreview },
    IdeReloadBuffer { path: PathBuf },
    IdeClosePreview,

    // === Control Flow ===
    AwaitApproval,
    Output { content: String },
    StreamDelta { content: String },
    Error { message: String },

    // === File System ===
    ReadFile { path: PathBuf },
    WriteFile { path: PathBuf, content: String },

    // === Shell ===
    Shell { command: String, working_dir: Option<String>, timeout_secs: u64 },

    // === Network ===
    FetchUrl { url: String, max_length: Option<usize> },
    WebSearch { query: String, count: u32 },

    // === Agents ===
    SpawnAgent { task: String, context: Option<String> },
    Notify { message: String },
}

/// A tool defined as a chain of effects
#[derive(Debug, Clone, Default)]
pub struct ToolPipeline {
    pub effects: Vec<Effect>,
}

impl ToolPipeline {
    pub fn new() -> Self {
        Self { effects: vec![] }
    }

    /// Create a pipeline that immediately errors
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            effects: vec![Effect::Error { message: message.into() }],
        }
    }

    /// Add an effect to the chain
    pub fn then(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    /// Add approval point (convenience method)
    pub fn await_approval(self) -> Self {
        self.then(Effect::AwaitApproval)
    }

    /// Check if pipeline requires approval
    pub fn requires_approval(&self) -> bool {
        self.effects.iter().any(|e| matches!(e, Effect::AwaitApproval))
    }

    /// Get effects before approval
    pub fn pre_approval(&self) -> Vec<Effect> {
        self.effects
            .iter()
            .take_while(|e| !matches!(e, Effect::AwaitApproval))
            .cloned()
            .collect()
    }

    /// Get effects after approval
    pub fn post_approval(&self) -> Vec<Effect> {
        self.effects
            .iter()
            .skip_while(|e| !matches!(e, Effect::AwaitApproval))
            .skip(1) // skip the AwaitApproval itself
            .cloned()
            .collect()
    }
}

/// Tool that composes effects
pub trait ComposableTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> serde_json::Value;
    fn compose(&self, params: serde_json::Value) -> ToolPipeline;
    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block>;

    fn ide_preview(&self, params: &serde_json::Value) -> Option<ToolPreview> {
        let pipeline = self.compose(params.clone());
        pipeline.effects.iter().find_map(|e| {
            if let Effect::IdeShowPreview { preview } = e {
                Some(preview.clone())
            } else {
                None
            }
        })
    }
}

/// Context for effect execution
#[derive(Debug, Clone, Default)]
pub struct EffectContext {
    pub params: serde_json::Value,
    pub data: std::collections::HashMap<String, String>,
    pub output: String,
}

impl EffectContext {
    pub fn new(params: serde_json::Value) -> Self {
        Self {
            params,
            data: std::collections::HashMap::new(),
            output: String::new(),
        }
    }

    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(|v| v.as_str())
    }

    pub fn param_path(&self, key: &str) -> Option<PathBuf> {
        self.param_str(key).map(PathBuf::from)
    }

    pub fn store(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.data.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(|s| s.as_str())
    }
}

// These are unused but kept for API compatibility
#[derive(Debug, Clone)]
pub enum EffectResult {
    Continue,
    Delta(String),
    Done { content: String, is_error: bool },
    Suspend(SuspendReason),
    StoreContext { key: String, value: String },
}

#[derive(Debug, Clone)]
pub enum SuspendReason {
    AwaitingApproval,
}

// Keep PipelineBuilder for convenience but simplify it
pub struct PipelineBuilder {
    pipeline: ToolPipeline,
}

impl PipelineBuilder {
    pub fn new() -> Self {
        Self { pipeline: ToolPipeline::new() }
    }

    pub fn then(mut self, effect: Effect) -> Self {
        self.pipeline = self.pipeline.then(effect);
        self
    }

    pub fn await_approval(self) -> Self {
        self.then(Effect::AwaitApproval)
    }

    pub fn build(self) -> ToolPipeline {
        self.pipeline
    }
}

impl Default for PipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_chain() {
        let pipeline = ToolPipeline::new()
            .then(Effect::ValidateFileExists { path: "/tmp/test".into() })
            .then(Effect::IdeOpen { path: "/tmp/test".into(), line: None, column: None })
            .await_approval()
            .then(Effect::WriteFile { path: "/tmp/test".into(), content: "hello".into() })
            .then(Effect::Output { content: "done".into() });

        assert_eq!(pipeline.effects.len(), 5);
        assert!(pipeline.requires_approval());
        assert_eq!(pipeline.pre_approval().len(), 2);
        assert_eq!(pipeline.post_approval().len(), 2);
    }

    #[test]
    fn test_no_approval() {
        let pipeline = ToolPipeline::new()
            .then(Effect::Output { content: "hello".into() });

        assert!(!pipeline.requires_approval());
        assert_eq!(pipeline.pre_approval().len(), 1);
        assert_eq!(pipeline.post_approval().len(), 0);
    }
}
