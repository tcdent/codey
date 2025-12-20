//! Read file tool

use super::{once_ready, Tool, ToolOutput, ToolResult};
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
use std::path::Path;

/// Read file display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl ReadFileBlock {
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
        let _: ReadFileParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for ReadFileBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let path = self.params["path"].as_str().unwrap_or("");
        let start_line = self.params.get("start_line").and_then(|v| v.as_i64());
        let end_line = self.params.get("end_line").and_then(|v| v.as_i64());

        // Format: read_file(path:start-end) or read_file(path)
        let range_str = match (start_line, end_line) {
            (Some(s), Some(e)) => format!(":{}:{}", s, e),
            (Some(s), None) => format!(":{}:", s),
            (None, Some(e)) => format!(":{}", e),
            (None, None) => String::new(),
        };

        lines.push(Line::from(vec![
            self.render_status(),
            Span::styled("read_file", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(path, Style::default().fg(Color::Cyan)),
            Span::styled(range_str, Style::default().fg(Color::DarkGray)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
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

/// Tool for reading file contents
pub struct ReadFileTool;

#[derive(Debug, Deserialize)]
struct ReadFileParams {
    path: String,
    start_line: Option<i32>,
    end_line: Option<i32>,
}

impl ReadFileTool {
    fn execute_inner(&self, params: serde_json::Value) -> ToolResult {
        let params: ReadFileParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid params: {}", e)),
        };
        let path = Path::new(&params.path);

        // Check if file exists
        if !path.exists() {
            return ToolResult::error(format!("File not found: {}", params.path));
        }

        // Check if it's a file (not a directory)
        if !path.is_file() {
            return ToolResult::error(format!("Not a file: {}", params.path));
        }

        // Read file contents
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult::error(format!("Failed to read file: {}", e));
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
            return ToolResult::error(format!(
                "Start line {} exceeds file length ({} lines)",
                start, total_lines
            ));
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

        ToolResult::success(output)
    }
}

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

    fn execute(&self, params: serde_json::Value) -> BoxStream<'static, ToolOutput> {
        once_ready(Ok(self.execute_inner(params)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tempfile::tempdir;

    async fn run_tool(tool: &ReadFileTool, params: serde_json::Value) -> ToolResult {
        let mut stream = tool.execute(params);
        while let Some(output) = stream.next().await {
            if let ToolOutput::Done(r) = output {
                return r;
            }
        }
        panic!("Tool should return Done");
    }

    #[tokio::test]
    async fn test_read_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let tool = ReadFileTool;
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap()
        })).await;

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
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap(),
            "start_line": 2,
            "end_line": 4
        })).await;

        assert!(!result.is_error);
        assert!(!result.content.contains("line 1"));
        assert!(result.content.contains("line 2"));
        assert!(result.content.contains("line 4"));
        assert!(!result.content.contains("line 5"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tool = ReadFileTool;
        let result = run_tool(&tool, json!({
            "path": "/nonexistent/file.txt"
        })).await;

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }
}
