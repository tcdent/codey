//! Read file tool

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_prefix, render_result, Block, BlockType, ToolBlock, Status};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

/// Read file display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl ReadFileBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value, background: bool) -> Option<Self> {
        let _: ReadFileParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params, background))
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
            render_prefix(self.background),
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
    pub const NAME: &'static str = "mcp_read_file";
}

impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Optionally specify a line range. \
         Returns the file contents with line numbers prefixed. \
         File content is only shown to the agent and is not displayed to the user."
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
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id; use list_background_tasks/get_background_task to check status and retrieve results."
                }
            },
            "required": ["path"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: ReadFileParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        let path = PathBuf::from(&parsed.path);

        ToolPipeline::new()
            .then(handlers::ValidateFile { path: path.clone() })
            .await_approval()
            .then(handlers::ReadFile {
                path,
                start_line: parsed.start_line,
                end_line: parsed.end_line,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        if let Some(block) = ReadFileBlock::from_params(call_id, self.name(), params.clone(), background) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params, background))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolExecutor, ToolRegistry, ToolCall, ToolDecision};
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_read_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ReadFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ReadFileTool::NAME.to_string(),
            params: json!({ "path": file_path.to_str().unwrap() }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        // Get the completed event
        if let Some(crate::tools::ToolEvent::Completed { content, .. }) = executor.next().await {
            assert!(content.contains("line 1"));
            assert!(content.contains("line 2"));
            assert!(content.contains("line 3"));
        } else {
            panic!("Expected Completed event");
        }
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ReadFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ReadFileTool::NAME.to_string(),
            params: json!({ "path": "/nonexistent/file.txt" }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Error { content, .. }) = executor.next().await {
            assert!(content.contains("not found") || content.contains("File not found"));
        } else {
            panic!("Expected Error event");
        }
    }

    #[tokio::test]
    async fn test_read_file_with_range() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ReadFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ReadFileTool::NAME.to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "start_line": 2,
                "end_line": 4
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, .. }) = executor.next().await {
            // Should contain lines 2-4 only
            assert!(content.contains("line 2"), "Should contain line 2");
            assert!(content.contains("line 3"), "Should contain line 3");
            assert!(content.contains("line 4"), "Should contain line 4");
            // Should NOT contain lines 1 or 5
            assert!(!content.contains("line 1"), "Should not contain line 1");
            assert!(!content.contains("line 5"), "Should not contain line 5");
            // Line numbers should be correct (2, 3, 4 not 1, 2, 3)
            assert!(content.contains("   2│line 2"), "Line 2 should be numbered as 2");
            assert!(content.contains("   3│line 3"), "Line 3 should be numbered as 3");
            assert!(content.contains("   4│line 4"), "Line 4 should be numbered as 4");
        } else {
            panic!("Expected Completed event");
        }
    }

    #[tokio::test]
    async fn test_read_file_with_end_of_file_marker() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ReadFileTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ReadFileTool::NAME.to_string(),
            params: json!({
                "path": file_path.to_str().unwrap(),
                "start_line": 3,
                "end_line": -1  // Read to end of file
            }),
            decision: ToolDecision::Approve,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, .. }) = executor.next().await {
            // Should contain lines 3-5
            assert!(content.contains("line 3"), "Should contain line 3");
            assert!(content.contains("line 4"), "Should contain line 4");
            assert!(content.contains("line 5"), "Should contain line 5");
            // Should NOT contain lines 1 or 2
            assert!(!content.contains("line 1"), "Should not contain line 1");
            assert!(!content.contains("line 2"), "Should not contain line 2");
        } else {
            panic!("Expected Completed event");
        }
    }
}
