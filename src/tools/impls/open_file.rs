//! Open file tool - opens a file in the IDE at a specific line

use super::{handlers, Tool, ToolPipeline};
use crate::transcript::{render_approval_prompt, Block, BlockType, Status};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

/// Tool for opening files in the IDE
pub struct OpenFileTool;

#[derive(Debug, Deserialize)]
struct OpenFileParams {
    path: String,
    line: Option<u32>,
}

impl Tool for OpenFileTool {
    fn name(&self) -> &'static str {
        "open_file"
    }

    fn description(&self) -> &'static str {
        "Open a file in the user's IDE/editor at a specific line. \
         Use this to show the user where something is located in their codebase."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to open"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number to navigate to (1-indexed, optional)"
                }
            },
            "required": ["path"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: OpenFileParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        let path = PathBuf::from(&parsed.path);
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.clone());

        let message = match parsed.line {
            Some(line) => format!("Opening {} at line {}", parsed.path, line),
            None => format!("Opening {}", parsed.path),
        };

        ToolPipeline::new()
            .then(handlers::ValidateFile { path })
            .await_approval()
            .then(handlers::IdeOpen {
                path: abs_path,
                line: parsed.line,
                column: None,
            })
            .then(handlers::Output { content: message })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        Box::new(OpenFileBlock::new(call_id, self.name(), params))
    }
}

/// Display block for open_file tool
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenFileBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl OpenFileBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
        }
    }
}

#[typetag::serde]
impl Block for OpenFileBlock {
    crate::impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let path = self.params["path"].as_str().unwrap_or("");
        let line = self.params.get("line").and_then(|v| v.as_u64());

        let location = match line {
            Some(l) => format!("{}:{}", path, l),
            None => path.to_string(),
        };

        let mut lines = vec![Line::from(vec![
            self.render_status(),
            Span::styled("open_file", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(location, Style::default().fg(Color::Cyan)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ])];

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
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
