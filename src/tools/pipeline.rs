//! Effect composition system for tools
//!
//! Tools can be defined as compositions of effects, allowing declarative
//! specification of tool behavior. For example, edit_file becomes:
//!
//! ```text
//! edit_file = [
//!     ValidateParams,     // Check params are well-formed
//!     ValidateFileExists, // Ensure file exists
//!     ComputePreview,     // Generate diff preview
//!     IdeOpen,            // Open file in IDE
//!     IdeShowPreview,     // Show the diff
//!     AwaitApproval,      // Wait for user approval
//!     Execute,            // Apply the edits
//!     IdeReloadBuffer,    // Reload the modified buffer
//! ]
//! ```
//!
//! Effects are pure data describing what should happen. The executor
//! interprets them, handling suspension (for approval) and resumption.

use crate::ide::ToolPreview;
use crate::transcript::Block;
use std::path::PathBuf;

/// Effects that can be composed to form a tool pipeline.
///
/// Effects are executed in order. Some effects (like AwaitApproval) cause
/// the pipeline to suspend until an external condition is met.
#[derive(Debug, Clone)]
pub enum Effect {
    // === Validation Effects ===
    // These run early and short-circuit on failure

    /// Validate that parameters are well-formed
    ValidateParams {
        /// Validation error message if invalid
        error: Option<String>,
    },

    /// Validate that a file exists
    ValidateFileExists {
        path: PathBuf,
    },

    /// Validate that a file is readable
    ValidateFileReadable {
        path: PathBuf,
    },

    /// Custom validation with error message
    Validate {
        ok: bool,
        error: String,
    },

    // === IDE Effects ===
    // These integrate with the user's editor

    /// Open/navigate to a file in the IDE
    IdeOpen {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },

    /// Show a preview (diff, content) in the IDE
    IdeShowPreview {
        preview: ToolPreview,
    },

    /// Reload a buffer after modification
    IdeReloadBuffer {
        path: PathBuf,
    },

    /// Close/dismiss the IDE preview
    IdeClosePreview,

    // === Control Flow Effects ===

    /// Await user approval before continuing
    /// The pipeline suspends here until the user approves or denies
    AwaitApproval,

    /// Stream a delta of output to the user
    StreamDelta {
        content: String,
    },

    /// Produce final output (successful)
    Output {
        content: String,
    },

    /// Produce an error and stop the pipeline
    Error {
        message: String,
    },

    // === File System Effects ===

    /// Read a file's content (stores in context for later use)
    ReadFile {
        path: PathBuf,
        /// Key to store content under in the context
        context_key: String,
    },

    /// Write content to a file
    WriteFile {
        path: PathBuf,
        content: String,
    },

    // === Agent Effects ===

    /// Spawn a background agent
    SpawnAgent {
        task: String,
        context: Option<String>,
    },

    /// Notify the user with a message
    Notify {
        message: String,
    },
}

/// A tool that can be expressed as a composition of effects.
///
/// This trait allows tools to declaratively specify their behavior as a
/// pipeline of effects, rather than imperatively implementing execution.
pub trait ComposableTool: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &'static str;

    /// Get the tool description
    fn description(&self) -> &'static str;

    /// Get the JSON schema for the tool's parameters
    fn schema(&self) -> serde_json::Value;

    /// Compose the effects for this tool given parameters.
    ///
    /// Returns a ToolPipeline containing all effects to execute.
    /// The pipeline is divided into phases:
    /// - pre: validation and setup (before approval)
    /// - approval: whether approval is needed
    /// - execute: the main effects (after approval)
    /// - post: cleanup effects (after execution)
    fn compose(&self, params: serde_json::Value) -> ToolPipeline;

    /// Create a block for displaying this tool call in the TUI
    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block>;

    /// Generate a preview for IDE display before execution
    fn ide_preview(&self, params: &serde_json::Value) -> Option<ToolPreview> {
        // Default: try to extract from compose
        let pipeline = self.compose(params.clone());
        pipeline.pre.iter().find_map(|e| {
            if let Effect::IdeShowPreview { preview } = e {
                Some(preview.clone())
            } else {
                None
            }
        })
    }
}

/// A pipeline of effects that form a complete tool execution.
#[derive(Debug, Clone)]
pub struct ToolPipeline {
    /// Effects to run before asking for approval (validation, preview generation)
    pub pre: Vec<Effect>,

    /// Whether this tool requires user approval
    pub requires_approval: bool,

    /// Effects to run after approval (the main execution)
    pub execute: Vec<Effect>,

    /// Effects to run after execution completes (cleanup, notifications)
    pub post: Vec<Effect>,
}

impl ToolPipeline {
    /// Create an empty pipeline
    pub fn new() -> Self {
        Self {
            pre: vec![],
            requires_approval: true,
            execute: vec![],
            post: vec![],
        }
    }

    /// Create a pipeline that just returns an error
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            pre: vec![Effect::Error { message: message.into() }],
            requires_approval: false,
            execute: vec![],
            post: vec![],
        }
    }

    /// Add a pre-approval effect
    pub fn pre(mut self, effect: Effect) -> Self {
        self.pre.push(effect);
        self
    }

    /// Add multiple pre-approval effects
    pub fn pre_all(mut self, effects: impl IntoIterator<Item = Effect>) -> Self {
        self.pre.extend(effects);
        self
    }

    /// Set whether approval is required
    pub fn approval(mut self, required: bool) -> Self {
        self.requires_approval = required;
        self
    }

    /// Add an execution effect
    pub fn exec(mut self, effect: Effect) -> Self {
        self.execute.push(effect);
        self
    }

    /// Add multiple execution effects
    pub fn exec_all(mut self, effects: impl IntoIterator<Item = Effect>) -> Self {
        self.execute.extend(effects);
        self
    }

    /// Add a post-execution effect
    pub fn post(mut self, effect: Effect) -> Self {
        self.post.push(effect);
        self
    }

    /// Add multiple post-execution effects
    pub fn post_all(mut self, effects: impl IntoIterator<Item = Effect>) -> Self {
        self.post.extend(effects);
        self
    }

    /// Flatten the pipeline into a single sequence of effects
    /// (with AwaitApproval inserted if needed)
    pub fn flatten(self) -> Vec<Effect> {
        let mut effects = self.pre;
        if self.requires_approval {
            effects.push(Effect::AwaitApproval);
        }
        effects.extend(self.execute);
        effects.extend(self.post);
        effects
    }
}

impl Default for ToolPipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of running an effect
#[derive(Debug, Clone)]
pub enum EffectResult {
    /// Effect completed successfully, continue to next
    Continue,

    /// Effect produced output to stream
    Delta(String),

    /// Effect completed the pipeline with a result
    Done {
        content: String,
        is_error: bool,
    },

    /// Effect requires suspension (e.g., waiting for approval)
    Suspend(SuspendReason),

    /// Effect stored data in context
    StoreContext {
        key: String,
        value: String,
    },
}

/// Reason for suspending pipeline execution
#[derive(Debug, Clone)]
pub enum SuspendReason {
    /// Awaiting user approval
    AwaitingApproval,
}

/// Context passed through effect execution
#[derive(Debug, Clone, Default)]
pub struct EffectContext {
    /// Original tool parameters
    pub params: serde_json::Value,

    /// Data stored by effects (e.g., file contents)
    pub data: std::collections::HashMap<String, String>,

    /// Collected output from the pipeline
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

    /// Get a parameter as a string
    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(|v| v.as_str())
    }

    /// Get a parameter as a PathBuf
    pub fn param_path(&self, key: &str) -> Option<PathBuf> {
        self.param_str(key).map(PathBuf::from)
    }

    /// Store data in context
    pub fn store(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.data.insert(key.into(), value.into());
    }

    /// Retrieve data from context
    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(|s| s.as_str())
    }
}

/// Builder for constructing tool pipelines declaratively
pub struct PipelineBuilder {
    pipeline: ToolPipeline,
}

impl PipelineBuilder {
    pub fn new() -> Self {
        Self {
            pipeline: ToolPipeline::new(),
        }
    }

    /// Validate that a condition is true
    pub fn validate(mut self, ok: bool, error: impl Into<String>) -> Self {
        self.pipeline.pre.push(Effect::Validate {
            ok,
            error: error.into()
        });
        self
    }

    /// Validate that a file exists
    pub fn validate_file_exists(mut self, path: impl Into<PathBuf>) -> Self {
        self.pipeline.pre.push(Effect::ValidateFileExists {
            path: path.into()
        });
        self
    }

    /// Open a file in the IDE
    pub fn ide_open(mut self, path: impl Into<PathBuf>) -> Self {
        self.pipeline.pre.push(Effect::IdeOpen {
            path: path.into(),
            line: None,
            column: None,
        });
        self
    }

    /// Open a file at a specific line in the IDE
    pub fn ide_open_at(mut self, path: impl Into<PathBuf>, line: u32) -> Self {
        self.pipeline.pre.push(Effect::IdeOpen {
            path: path.into(),
            line: Some(line),
            column: None,
        });
        self
    }

    /// Show a preview in the IDE
    pub fn ide_preview(mut self, preview: ToolPreview) -> Self {
        self.pipeline.pre.push(Effect::IdeShowPreview { preview });
        self
    }

    /// Require user approval (this is the default)
    pub fn require_approval(mut self) -> Self {
        self.pipeline.requires_approval = true;
        self
    }

    /// Skip approval requirement
    pub fn no_approval(mut self) -> Self {
        self.pipeline.requires_approval = false;
        self
    }

    /// Write content to a file (execution phase)
    pub fn write_file(mut self, path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        self.pipeline.execute.push(Effect::WriteFile {
            path: path.into(),
            content: content.into(),
        });
        self
    }

    /// Produce output (execution phase)
    pub fn output(mut self, content: impl Into<String>) -> Self {
        self.pipeline.execute.push(Effect::Output {
            content: content.into()
        });
        self
    }

    /// Stream output delta (execution phase)
    pub fn stream(mut self, content: impl Into<String>) -> Self {
        self.pipeline.execute.push(Effect::StreamDelta {
            content: content.into()
        });
        self
    }

    /// Reload IDE buffer after modification (post phase)
    pub fn ide_reload(mut self, path: impl Into<PathBuf>) -> Self {
        self.pipeline.post.push(Effect::IdeReloadBuffer {
            path: path.into()
        });
        self
    }

    /// Close IDE preview (post phase)
    pub fn ide_close(mut self) -> Self {
        self.pipeline.post.push(Effect::IdeClosePreview);
        self
    }

    /// Spawn a background agent (post phase)
    pub fn spawn_agent(mut self, task: impl Into<String>) -> Self {
        self.pipeline.post.push(Effect::SpawnAgent {
            task: task.into(),
            context: None,
        });
        self
    }

    /// Build the pipeline
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
    fn test_pipeline_builder() {
        let pipeline = PipelineBuilder::new()
            .validate(true, "should pass")
            .validate_file_exists("/tmp/test.txt")
            .ide_open("/tmp/test.txt")
            .require_approval()
            .write_file("/tmp/test.txt", "new content")
            .output("File written successfully")
            .ide_reload("/tmp/test.txt")
            .build();

        assert_eq!(pipeline.pre.len(), 3); // validate, validate_file_exists, ide_open
        assert!(pipeline.requires_approval);
        assert_eq!(pipeline.execute.len(), 2); // write_file, output
        assert_eq!(pipeline.post.len(), 1); // ide_reload
    }

    #[test]
    fn test_pipeline_flatten() {
        let pipeline = PipelineBuilder::new()
            .validate(true, "ok")
            .require_approval()
            .output("done")
            .ide_reload("/tmp/test.txt")
            .build();

        let effects = pipeline.flatten();

        // Should be: validate, await_approval, output, ide_reload
        assert_eq!(effects.len(), 4);
        assert!(matches!(effects[0], Effect::Validate { .. }));
        assert!(matches!(effects[1], Effect::AwaitApproval));
        assert!(matches!(effects[2], Effect::Output { .. }));
        assert!(matches!(effects[3], Effect::IdeReloadBuffer { .. }));
    }

    #[test]
    fn test_effect_context() {
        let params = serde_json::json!({
            "path": "/tmp/test.txt",
            "content": "hello"
        });

        let mut ctx = EffectContext::new(params);

        assert_eq!(ctx.param_str("path"), Some("/tmp/test.txt"));
        assert_eq!(ctx.param_path("path"), Some(PathBuf::from("/tmp/test.txt")));

        ctx.store("original", "old content");
        assert_eq!(ctx.get("original"), Some("old content"));
    }
}
