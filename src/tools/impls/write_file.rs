//! Write file tool
//!
//! The write_file tool as a chain of effects:
//! ```text
//! write_file = [
//!     IdeShowPreview,   // Show the new file content
//!     AwaitApproval,    // Wait for user approval
//!     WriteFile,        // Create the file
//!     Output,           // Report success
//!     IdeReloadBuffer,  // Reload buffer in IDE
//! ]
//! ```

use super::{handlers, Tool, ToolPipeline};
use crate::ide::ToolPreview;
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_result, Block, BlockType, ToolBlock, Status};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Write file display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl WriteFileBlock {
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
        let _: WriteFileParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for WriteFileBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let path = self.params["path"].as_str().unwrap_or("");
        let content_len = self.params.get("content").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);

        // Format: write_file(path, N bytes)
        lines.push(Line::from(vec![
            self.render_status(),
            Span::styled("write_file", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(path, Style::default().fg(Color::Green)),
            Span::styled(format!(", {} bytes", content_len), Style::default().fg(Color::DarkGray)),
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

/// Tool for creating new files
pub struct WriteFileTool;

#[derive(Debug, Deserialize)]
struct WriteFileParams {
    path: String,
    content: String,
}

impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create a new file with the specified content. Fails if the file already exists. \
         Use edit_file to modify existing files."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path where the file will be created"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: Result<WriteFileParams, _> = serde_json::from_value(params.clone());
        let params = match parsed {
            Ok(p) => p,
            Err(e) => {
                return ToolPipeline::error(format!("Invalid params: {}", e));
            }
        };

        let path = PathBuf::from(&params.path);
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.clone());

        ToolPipeline::new()
            .then(handlers::ValidateFileNotExists {
                path: path.clone(),
                message: format!(
                    "File already exists: {}. Use edit_file to modify existing files.",
                    params.path
                ),
            })
            .then(handlers::IdeShowPreview {
                preview: ToolPreview::File {
                    path: params.path.clone(),
                    content: params.content.clone(),
                },
            })
            .await_approval()
            .then(handlers::WriteFile { path: path.clone(), content: params.content.clone() })
            .then(handlers::Output {
                content: format!(
                    "Created file: {} ({} lines, {} bytes)",
                    params.path,
                    params.content.lines().count(),
                    params.content.len()
                ),
            })
            .then(handlers::IdeReloadBuffer { path: abs_path })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = WriteFileBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolExecutor, ToolRegistry, ToolCall, ToolDecision, ToolEvent};
    use std::fs;
    use tempfile::tempdir;

    /// Helper to run a tool to completion, auto-responding to Delegate events
    async fn run_to_completion(executor: &mut ToolExecutor) -> ToolEvent {
        loop {
            match executor.next().await {
                Some(ToolEvent::Delegate { responder, .. }) => {
                    let _ = responder.send(Ok(()));
                },
                Some(event @ ToolEvent::Completed { .. }) => return event,
                Some(event @ ToolEvent::Error { .. }) => return event,
                Some(_) => continue,
                None => panic!("Executor returned None before completion"),
            }
        }
    }

    #[tokio::test]
    async fn test_write_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("new_file.txt");

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(WriteFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: "write_file".to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "content": "Hello, World!\nLine 2"
            }),
            decision: ToolDecision::Approve,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Completed { content, .. } => {
                let file_content = fs::read_to_string(&file_path).expect(&format!("Failed to read file, tool output: {}", content));
                assert_eq!(file_content, "Hello, World!\nLine 2");
            },
            other => panic!("Expected Completed event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_write_existing_file_fails() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(&file_path, "existing content").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(WriteFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: "write_file".to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "content": "new content"
            }),
            decision: ToolDecision::Approve,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Error { content, .. } => {
                assert!(content.contains("already exists"));
            },
            other => panic!("Expected Error event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("subdir").join("nested").join("file.txt");

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(WriteFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: "write_file".to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "content": "nested content"
            }),
            decision: ToolDecision::Approve,
        }]);

        match run_to_completion(&mut executor).await {
            ToolEvent::Completed { .. } => {
                assert!(file_path.exists());
            },
            other => panic!("Expected Completed event, got {:?}", other),
        }
    }
}
