//! Edit file tool with search/replace
//!
//! The edit_file tool as a chain of effects:
//! ```text
//! edit_file = [
//!     IdeOpen,          // Open file in IDE
//!     IdeShowPreview,   // Show the diff preview
//!     AwaitApproval,    // Wait for user approval
//!     WriteFile,        // Apply the edits
//!     Output,           // Report success
//!     IdeReloadBuffer,  // Reload the modified buffer
//! ]
//! ```

use super::{ComposableTool, Effect, ToolPipeline};
use crate::ide::ToolPreview;
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_result, Block, BlockType, ToolBlock, Status};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// Edit file display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditFileBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl EditFileBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
        }
    }

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value) -> Option<Self> {
        let _: EditFileParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for EditFileBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let path = self.params["path"].as_str().unwrap_or("");
        let edit_count = self.params.get("edits").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

        // Format: edit_file(path, N edits)
        lines.push(Line::from(vec![
            self.render_status(),
            Span::styled("edit_file", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(path, Style::default().fg(Color::Yellow)),
            Span::styled(format!(", {} edit{}", edit_count, if edit_count == 1 { "" } else { "s" }), Style::default().fg(Color::DarkGray)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
        }

        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }

    fn tool_name(&self) -> Option<&str> {
        Some(&self.tool_name)
    }

    fn params(&self) -> Option<&serde_json::Value> {
        Some(&self.params)
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
    /// Validate the edits and compute the modified content
    fn validate_and_compute(path: &Path, edits: &[SearchReplace]) -> Result<(String, String), String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read file: {}", e))?;

        let mut modified = content.clone();

        for (i, edit) in edits.iter().enumerate() {
            let count = modified.matches(&edit.old_string).count();
            match count {
                0 => {
                    return Err(format!(
                        "Edit {}: old_string not found in file. \
                         Make sure the string matches exactly, including whitespace and indentation.",
                        i + 1
                    ));
                }
                1 => {
                    modified = modified.replacen(&edit.old_string, &edit.new_string, 1);
                }
                n => {
                    return Err(format!(
                        "Edit {}: old_string found {} times (must be unique). \
                         Include more surrounding context to make the match unique.",
                        i + 1,
                        n
                    ));
                }
            }
        }

        Ok((content, modified))
    }
}

impl ComposableTool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
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
            }
        };

        let path = PathBuf::from(&params.path);

        if !path.exists() {
            return ToolPipeline::error(format!(
                "File not found: {}. Use write_file to create new files.",
                params.path
            ));
        }

        if !path.is_file() {
            return ToolPipeline::error(format!("Not a file: {}", params.path));
        }

        let (original, modified) = match Self::validate_and_compute(&path, &params.edits) {
            Ok((orig, mod_)) => (orig, mod_),
            Err(e) => return ToolPipeline::error(e),
        };

        let edit_count = params.edits.len();
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.clone());

        ToolPipeline::new()
            .then(Effect::IdeOpen { path: abs_path.clone(), line: None, column: None })
            .then(Effect::IdeShowPreview {
                preview: ToolPreview::Diff {
                    path: params.path.clone(),
                    original,
                    modified: modified.clone(),
                },
            })
            .await_approval()
            .then(Effect::WriteFile { path: abs_path.clone(), content: modified })
            .then(Effect::Output {
                content: format!("Successfully applied {} edit(s) to {}", edit_count, params.path),
            })
            .then(Effect::IdeReloadBuffer { path: abs_path })
            .then(Effect::IdeClosePreview)
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = EditFileBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolExecutor, ToolRegistry, ToolCall, ToolDecision};
    use tempfile::tempdir;

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
            name: "edit_file".to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [{
                    "old_string": "println!(\"hello\")",
                    "new_string": "println!(\"hello, world!\")"
                }]
            }),
            decision: ToolDecision::Approve,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { is_error, .. }) = executor.next().await {
            assert!(!is_error);
            let content = fs::read_to_string(&file_path).unwrap();
            assert!(content.contains("hello, world!"));
        } else {
            panic!("Expected Completed event");
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
            name: "edit_file".to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [
                    { "old_string": "fn foo() {}", "new_string": "fn foo() -> i32 { 1 }" },
                    { "old_string": "fn bar() {}", "new_string": "fn bar() -> i32 { 2 }" }
                ]
            }),
            decision: ToolDecision::Approve,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { is_error, .. }) = executor.next().await {
            assert!(!is_error);
            let content = fs::read_to_string(&file_path).unwrap();
            assert!(content.contains("fn foo() -> i32 { 1 }"));
            assert!(content.contains("fn bar() -> i32 { 2 }"));
        } else {
            panic!("Expected Completed event");
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
            name: "edit_file".to_string(),
            params: json!({
                "path": "/nonexistent/file.rs",
                "edits": [{ "old_string": "foo", "new_string": "bar" }]
            }),
            decision: ToolDecision::Approve,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, is_error, .. }) = executor.next().await {
            assert!(is_error);
            assert!(content.contains("not found"));
        } else {
            panic!("Expected Completed event");
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
            name: "edit_file".to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [{ "old_string": "foo", "new_string": "bar" }]
            }),
            decision: ToolDecision::Approve,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, is_error, .. }) = executor.next().await {
            assert!(is_error);
            assert!(content.contains("3 times"));
        } else {
            panic!("Expected Completed event");
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
            name: "edit_file".to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "edits": [{ "old_string": "goodbye", "new_string": "farewell" }]
            }),
            decision: ToolDecision::Approve,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, is_error, .. }) = executor.next().await {
            assert!(is_error);
            assert!(content.contains("not found"));
        } else {
            panic!("Expected Completed event");
        }
    }
}
