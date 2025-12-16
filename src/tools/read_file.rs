//! Read file tool

use super::{Tool, ToolResult};
use crate::transcript::{render_approval_prompt, render_result, Block, ToolBlock, Status};
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

/// Read file display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub result: Option<String>,
}

impl ReadFileBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            result: None,
        }
    }

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value) -> Option<Self> {
        let _: ReadFileParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for ReadFileBlock {
    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let (icon, color) = match self.status {
            Status::Pending => ("?", Color::Yellow),
            Status::Running => ("⚙", Color::Blue),
            Status::Complete => ("✓", Color::Green),
            Status::Error => ("✗", Color::Red),
            Status::Denied => ("⊘", Color::DarkGray),
            Status::Cancelled => ("⊘", Color::Yellow),
        };

        let path = self.params["path"].as_str().unwrap_or("");
        let start_line = self.params.get("start_line").and_then(|v| v.as_i64());
        let end_line = self.params.get("end_line").and_then(|v| v.as_i64());

        // Icon and path
        let mut header = vec![
            Span::styled(format!("{} ", icon), Style::default().fg(color)),
            Span::styled("read ", Style::default().fg(Color::DarkGray)),
            Span::styled(path, Style::default().fg(Color::Cyan)),
        ];

        // Line range if specified
        if start_line.is_some() || end_line.is_some() {
            let start = start_line.map(|n| n.to_string()).unwrap_or_default();
            let end = end_line.map(|n| n.to_string()).unwrap_or_default();
            header.push(Span::styled(
                format!(" [{}:{}]", start, end),
                Style::default().fg(Color::DarkGray),
            ));
        }

        lines.push(Line::from(header));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if let Some(ref result) = self.result {
            lines.extend(render_result(result, 10));
        }

        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn status(&self) -> Status {
        self.status
    }

    fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    fn set_result(&mut self, result: String) {
        self.result = Some(result);
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

    fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Tool for reading file contents
pub struct ReadFileTool;

#[derive(Debug, Deserialize)]
struct ReadFileParams {
    path: String,
    start_line: Option<i32>,
    end_line: Option<i32>,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Optionally specify a line range. \
         Returns the file contents with line numbers prefixed."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed, optional)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "Ending line number (inclusive, optional). Use -1 for end of file."
                }
            },
            "required": ["path"]
        })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = ReadFileBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let params: ReadFileParams = serde_json::from_value(params)?;
        let path = Path::new(&params.path);

        // Check if file exists
        if !path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}",
                params.path
            )));
        }

        // Check if it's a file (not a directory)
        if !path.is_file() {
            return Ok(ToolResult::error(format!(
                "Not a file: {}",
                params.path
            )));
        }

        // Read file contents
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file: {}",
                    e
                )));
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Calculate line range
        let start = params.start_line.unwrap_or(1).max(1) as usize;
        let end = match params.end_line {
            Some(-1) | None => total_lines,
            Some(n) => (n as usize).min(total_lines),
        };

        if start > total_lines {
            return Ok(ToolResult::error(format!(
                "Start line {} exceeds file length ({} lines)",
                start, total_lines
            )));
        }

        // Format output with line numbers
        let mut output = String::new();
        let line_num_width = end.to_string().len().max(4);

        for (i, line) in lines.iter().enumerate() {
            let line_num = i + 1;
            if line_num >= start && line_num <= end {
                output.push_str(&format!(
                    "{:>width$}│{}\n",
                    line_num,
                    line,
                    width = line_num_width
                ));
            }
        }

        // Add metadata
        if start > 1 || end < total_lines {
            output.push_str(&format!(
                "\n[Showing lines {}-{} of {}]",
                start, end, total_lines
            ));
        }

        Ok(ToolResult::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_read_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let tool = ReadFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap()
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("line 1"));
        assert!(result.content.contains("line 2"));
        assert!(result.content.contains("line 3"));
    }

    #[tokio::test]
    async fn test_read_file_with_range() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        let tool = ReadFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "start_line": 2,
                "end_line": 4
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(!result.content.contains("line 1"));
        assert!(result.content.contains("line 2"));
        assert!(result.content.contains("line 4"));
        assert!(!result.content.contains("line 5"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tool = ReadFileTool;
        let result = tool
            .execute(json!({
                "path": "/nonexistent/file.txt"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }
}
