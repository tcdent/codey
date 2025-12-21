//! Write file tool
//!
//! The write_file tool as a composition of effects:
//! ```text
//! write_file = [
//!     Pre:  ValidateParams       // Check params are well-formed
//!           ValidateFileNotExists // Ensure file doesn't exist
//!           IdeShowPreview       // Show the new file content
//!     ---:  AwaitApproval        // Wait for user approval
//!     Exec: WriteFile            // Create the file
//!           Output               // Report success
//!     Post: IdeReloadBuffer      // Reload buffer in IDE
//! ]
//! ```

use super::{once_ready, ComposableTool, Effect, Tool, ToolEffect, ToolOutput, ToolPipeline, ToolResult};
use crate::ide::ToolPreview;
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_result, Block, BlockType, ToolBlock, Status};
use futures::stream::BoxStream;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// Tool name constant to avoid ambiguity between trait implementations
const TOOL_NAME: &str = "write_file";

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

impl WriteFileTool {
    fn execute_inner(&self, params: serde_json::Value) -> ToolResult {
        let params: WriteFileParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid params: {}", e)),
        };
        let path = Path::new(&params.path);

        // Check if file already exists
        if path.exists() {
            return ToolResult::error(format!(
                "File already exists: {}. Use edit_file to modify existing files.",
                params.path
            ));
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return ToolResult::error(format!(
                        "Failed to create parent directories: {}",
                        e
                    ));
                }
            }
        }

        // Write file
        match fs::write(path, &params.content) {
            Ok(()) => {
                let line_count = params.content.lines().count();
                let byte_count = params.content.len();
                let abs_path = path.canonicalize().unwrap_or_else(|_| PathBuf::from(&params.path));
                ToolResult::success(format!(
                    "Created file: {} ({} lines, {} bytes)",
                    params.path, line_count, byte_count
                )).with_effects(vec![ToolEffect::IdeReloadBuffer { path: abs_path }])
            }
            Err(e) => ToolResult::error(format!("Failed to write file: {}", e)),
        }
    }
}

impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        TOOL_NAME
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

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = WriteFileBlock::from_params(call_id, TOOL_NAME, params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, TOOL_NAME, params))
        }
    }

    fn execute(&self, params: serde_json::Value) -> BoxStream<'static, ToolOutput> {
        once_ready(Ok(self.execute_inner(params)))
    }

    fn ide_preview(&self, params: &serde_json::Value) -> Option<ToolPreview> {
        // Delegate to the effect pipeline - extract preview from pre-effects
        let pipeline = ComposableTool::compose(self, params.clone());
        pipeline.pre.into_iter().find_map(|effect| {
            if let Effect::IdeShowPreview { preview } = effect {
                Some(preview)
            } else {
                None
            }
        })
    }
}

// ============================================================================
// ComposableTool Implementation
// ============================================================================

impl ComposableTool for WriteFileTool {
    fn name(&self) -> &'static str {
        TOOL_NAME
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
        // Parse parameters
        let parsed: Result<WriteFileParams, _> = serde_json::from_value(params.clone());
        let params = match parsed {
            Ok(p) => p,
            Err(e) => {
                return ToolPipeline::error(format!("Invalid params: {}", e));
            }
        };

        let path = PathBuf::from(&params.path);

        // Check if file already exists
        if path.exists() {
            return ToolPipeline::error(format!(
                "File already exists: {}. Use edit_file to modify existing files.",
                params.path
            ));
        }

        // Build the effect pipeline
        ToolPipeline::new()
            // Pre-approval: Show preview of file content
            .pre(Effect::IdeShowPreview {
                preview: ToolPreview::FileContent {
                    path: params.path.clone(),
                    content: params.content.clone(),
                },
            })
            // Approval is required by default
            .approval(true)
            // Execute: Write the file
            .exec(Effect::WriteFile {
                path: path.clone(),
                content: params.content.clone(),
            })
            .exec(Effect::Output {
                content: format!(
                    "Created file: {} ({} lines, {} bytes)",
                    params.path,
                    params.content.lines().count(),
                    params.content.len()
                ),
            })
            // Post: Reload buffer
            .post(Effect::IdeReloadBuffer {
                path: path.canonicalize().unwrap_or(path),
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = WriteFileBlock::from_params(call_id, TOOL_NAME, params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, TOOL_NAME, params))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tempfile::tempdir;

    async fn run_tool(tool: &WriteFileTool, params: serde_json::Value) -> ToolResult {
        let mut stream = tool.execute(params);
        while let Some(output) = stream.next().await {
            if let ToolOutput::Done(r) = output {
                return r;
            }
        }
        panic!("Tool should return Done");
    }

    #[tokio::test]
    async fn test_write_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("new_file.txt");

        let tool = WriteFileTool;
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap(),
            "content": "Hello, World!\nLine 2"
        })).await;

        assert!(!result.is_error);
        assert!(result.content.contains("Created file"));

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, World!\nLine 2");
    }

    #[tokio::test]
    async fn test_write_existing_file_fails() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(&file_path, "existing content").unwrap();

        let tool = WriteFileTool;
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap(),
            "content": "new content"
        })).await;

        assert!(result.is_error);
        assert!(result.content.contains("already exists"));
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("subdir").join("nested").join("file.txt");

        let tool = WriteFileTool;
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap(),
            "content": "nested content"
        })).await;

        assert!(!result.is_error);
        assert!(file_path.exists());
    }
}
