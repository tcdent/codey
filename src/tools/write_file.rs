//! Write file tool

use super::{Tool, ToolResult};
use crate::message::{render_approval_prompt, render_result, ContentBlock, ToolBlock, Status};
use anyhow::Result;
use async_trait::async_trait;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::Path;

/// Write file display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileBlock {
    pub call_id: String,
    pub path: String,
    pub content_len: usize,
    pub status: Status,
    pub result: Option<String>,
}

impl WriteFileBlock {
    pub fn new(call_id: impl Into<String>, path: impl Into<String>, content_len: usize) -> Self {
        Self {
            call_id: call_id.into(),
            path: path.into(),
            content_len,
            status: Status::Pending,
            result: None,
        }
    }

    pub fn from_params(call_id: &str, params: &serde_json::Value) -> Option<Self> {
        let path = params.get("path")?.as_str()?;
        let content_len = params.get("content").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);
        Some(Self::new(call_id, path, content_len))
    }
}

impl ContentBlock for WriteFileBlock {
    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let (icon, color) = match self.status {
            Status::Pending => ("?", Color::Yellow),
            Status::Running => ("⚙", Color::Blue),
            Status::Success => ("✓", Color::Green),
            Status::Error => ("✗", Color::Red),
            Status::Denied => ("⊘", Color::DarkGray),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", icon), Style::default().fg(color)),
            Span::styled("write ", Style::default().fg(Color::DarkGray)),
            Span::styled(&self.path, Style::default().fg(Color::Green)),
            Span::styled(
                format!(" ({} bytes)", self.content_len),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if let Some(ref result) = self.result {
            lines.extend(render_result(result, 5));
        }

        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn status(&self) -> Option<Status> {
        Some(self.status)
    }

    fn tool_name(&self) -> Option<&str> {
        Some("write_file")
    }

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }

    fn approve(&mut self) {
        self.status = Status::Running;
    }

    fn deny(&mut self) {
        self.status = Status::Denied;
    }

    fn complete(&mut self, result: String, is_error: bool) {
        self.status = if is_error {
            Status::Error
        } else {
            Status::Success
        };
        self.result = Some(result);
    }
}

/// Tool for creating new files
pub struct WriteFileTool;

#[derive(Debug, Deserialize)]
struct WriteFileParams {
    path: String,
    content: String,
}

#[async_trait]
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

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn ContentBlock> {
        if let Some(block) = WriteFileBlock::from_params(call_id, &params) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let params: WriteFileParams = serde_json::from_value(params)?;
        let path = Path::new(&params.path);

        // Check if file already exists
        if path.exists() {
            return Ok(ToolResult::error(format!(
                "File already exists: {}. Use edit_file to modify existing files.",
                params.path
            )));
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Ok(ToolResult::error(format!(
                        "Failed to create parent directories: {}",
                        e
                    )));
                }
            }
        }

        // Write file
        match fs::write(path, &params.content) {
            Ok(()) => {
                let line_count = params.content.lines().count();
                let byte_count = params.content.len();
                Ok(ToolResult::success(format!(
                    "Created file: {} ({} lines, {} bytes)",
                    params.path, line_count, byte_count
                )))
            }
            Err(e) => Ok(ToolResult::error(format!("Failed to write file: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_write_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("new_file.txt");

        let tool = WriteFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "Hello, World!\nLine 2"
            }))
            .await
            .unwrap();

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
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "new content"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("already exists"));
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("subdir").join("nested").join("file.txt");

        let tool = WriteFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "nested content"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(file_path.exists());
    }
}
