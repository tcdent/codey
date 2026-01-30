//! Edit file tool with search/replace
//!
//! The edit_file tool as a chain of effects:
//! ```text
//! edit_file = [
//!     ValidateFile,           // Check file exists and is readable
//!     ValidateNoUnsavedEdits, // Check IDE has no unsaved changes
//!     ValidateFileWritable,   // Check file is writable
//!     ValidateEdits,          // Check edits are valid before prompting user
//!     IdeShowDiffPreview,     // Show hunks with context
//!     AwaitApproval,
//!     ApplyEdits,             // Apply the edits
//!     Output,
//!     IdeReloadBuffer,
//! ] + finally [IdeClosePreview]  // Closes preview on success, deny, or error
//! ```

use std::path::PathBuf;

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::ide::Edit;
use crate::define_tool_block;
use crate::tools::pipeline::{EffectHandler, Step};
use crate::transcript::{
    render_agent_label, render_approval_prompt, render_prefix, render_result, Block, BlockType, Status, ToolBlock,
};

// =============================================================================
// Edit-specific validation handler
// =============================================================================

/// Validate edits can be applied (each old_string exists exactly once)
pub struct ValidateEdits {
    pub path: PathBuf,
    pub edits: Vec<Edit>,
}

#[async_trait::async_trait]
impl EffectHandler for ValidateEdits {
    async fn call(self: Box<Self>) -> Step {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) => return Step::Error(format!("Failed to read file: {}", e)),
        };

        for (i, edit) in self.edits.iter().enumerate() {
            let count = content.matches(&edit.old_string).count();
            match count {
                0 => {
                    return Step::Error(format!(
                        "Edit {}: old_string not found in file. \
                         Make sure the string matches exactly, including whitespace and indentation.",
                        i + 1
                    ));
                },
                1 => {}, // good
                n => {
                    return Step::Error(format!(
                        "Edit {}: old_string found {} times (must be unique). \
                         Include more surrounding context to make the match unique.",
                        i + 1,
                        n
                    ));
                },
            }
        }

        Step::Continue
    }
}

define_tool_block! {
    /// Edit file display block
    pub struct EditFileBlock {
        max_lines: 5,
        params_type: EditFileParams,
        render_header(self, params) {
            let path = params["path"].as_str().unwrap_or("");
            let edit_count = params
                .get("edits")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            vec![
                Span::styled("edit_file", Style::default().fg(Color::Magenta)),
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled(path.to_string(), Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!(
                        ", {} edit{}",
                        edit_count,
                        if edit_count == 1 { "" } else { "s" }
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]
        }
    }
}

/// Tool for editing existing files with search/replace
pub struct EditFileTool;

#[derive(Debug, Deserialize)]
struct EditFileParams {
    path: String,
    edits: Vec<SearchReplace>,
}

#[derive(Debug, Deserialize)]
struct SearchReplace {
    old_string: String,
    new_string: String,
}

impl EditFileTool {
    pub const NAME: &'static str = "mcp_edit_file";
}

impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Apply search/replace edits to an existing file. Each old_string must match exactly \
         and appear exactly once in the file. Edits are applied sequentially. \
         Use read_file first to see the current file contents."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "edits": {
                    "type": "array",
                    "description": "List of search/replace operations to apply sequentially",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": {
                                "type": "string",
                                "description": "Exact string to find (must be unique in file)"
                            },
                            "new_string": {
                                "type": "string",
                                "description": "String to replace it with"
                            }
                        },
                        "required": ["old_string", "new_string"]
                    }
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id; use list_background_tasks/get_background_task to check status and retrieve results."
                }
            },
            "required": ["path", "edits"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: Result<EditFileParams, _> = serde_json::from_value(params.clone());
        let params = match parsed {
            Ok(p) => p,
            Err(e) => {
                return ToolPipeline::error(format!("Invalid params: {}", e));
            },
        };

        let path = PathBuf::from(&params.path);
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        let edit_count = params.edits.len();

        // Convert to Edit type for handlers
        let edits: Vec<Edit> = params
            .edits
            .iter()
            .map(|e| Edit {
                old_string: e.old_string.clone(),
                new_string: e.new_string.clone(),
            })
            .collect();

        ToolPipeline::new()
            .then(handlers::ValidateFile { path: path.clone() })
            .then(handlers::ValidateNoUnsavedEdits { path: path.clone() })
            .then(handlers::ValidateFileWritable { path: path.clone() })
            .then(ValidateEdits {
                path: path.clone(),
                edits: edits.clone(),
            })
            .then(handlers::IdeShowDiffPreview {
                path: abs_path.clone(),
                edits: edits.clone(),
            })
            .await_approval()
            .then(handlers::ApplyEdits {
                path: abs_path.clone(),
                edits,
            })
            .then(handlers::Output {
                content: format!(
                    "Successfully applied {} edit(s) to {}",
                    edit_count, params.path
                ),
            })
            .then(handlers::IdeReloadBuffer { path: abs_path })
            .finally(handlers::IdeClosePreview)
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        if let Some(block) = EditFileBlock::from_params(call_id, self.name(), params.clone(), background) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params, background))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::tools::{ToolCall, ToolDecision, ToolEvent, ToolExecutor, ToolRegistry};

    /// Helper to run a tool to completion, auto-responding to Delegate events
    async fn run_to_completion(executor: &mut ToolExecutor) -> ToolEvent {
        loop {
            match executor.next().await {
                Some(ToolEvent::Delegate { responder, .. }) => {
                    // Auto-respond with Ok to IDE effects
                    let _ = responder.send(Ok(None));
                },
                Some(event @ ToolEvent::Completed { .. }) => return event,
                Some(event @ ToolEvent::Error { .. }) => return event,
                Some(_) => continue,
                None => panic!("Executor returned None before completion"),
            }
        }
    }

    #[tokio::test]
    async fn test_edit_file_single() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(EditFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: EditFileTool::NAME.to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [{
                    "old_string": "println!(\"hello\")",
                    "new_string": "println!(\"hello, world!\")"
                }]
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Completed { .. } => {
                let content = fs::read_to_string(&file_path).unwrap();
                assert!(content.contains("hello, world!"));
            },
            other => panic!("Expected Completed event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_edit_file_multiple() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "fn foo() {}\n\nfn bar() {}\n\nfn baz() {}").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(EditFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: EditFileTool::NAME.to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [
                    { "old_string": "fn foo() {}", "new_string": "fn foo() -> i32 { 1 }" },
                    { "old_string": "fn bar() {}", "new_string": "fn bar() -> i32 { 2 }" }
                ]
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Completed { .. } => {
                let content = fs::read_to_string(&file_path).unwrap();
                assert!(content.contains("fn foo() -> i32 { 1 }"));
                assert!(content.contains("fn bar() -> i32 { 2 }"));
            },
            other => panic!("Expected Completed event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(EditFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: EditFileTool::NAME.to_string(),
            params: json!({
                "path": "/nonexistent/file.rs",
                "edits": [{ "old_string": "foo", "new_string": "bar" }]
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Error { content, .. } => {
                assert!(content.contains("not found"));
            },
            other => panic!("Expected Error event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_edit_file_ambiguous() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "foo foo foo").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(EditFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: EditFileTool::NAME.to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [{ "old_string": "foo", "new_string": "bar" }]
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Error { content, .. } => {
                assert!(content.contains("3 times"));
            },
            other => panic!("Expected Error event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_edit_file_string_not_found() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "hello world").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(EditFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: EditFileTool::NAME.to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [{ "old_string": "goodbye", "new_string": "farewell" }]
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Error { content, .. } => {
                assert!(content.contains("not found"));
            },
            other => panic!("Expected Error event, got {:?}", other),
        }
    }
}
