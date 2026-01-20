//! Background task management tools
//!
//! Tools for querying and retrieving results from background tool executions.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_prefix, render_result, Block, BlockType, Status};

// =============================================================================
// ListBackgroundTasks block
// =============================================================================

/// Block for list_background_tasks - shows as `list_background_tasks()`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBackgroundTasksBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl ListBackgroundTasksBlock {
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
}

#[typetag::serde]
impl Block for ListBackgroundTasksBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        // Format: list_background_tasks()
        let spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("list_background_tasks", Style::default().fg(Color::Magenta)),
            Span::styled("()", Style::default().fg(Color::DarkGray)),
        ];
        lines.push(Line::from(spans));

        // Approval prompt if pending
        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        // Output if completed
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
        }

        // Denied message
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

// =============================================================================
// GetBackgroundTask block
// =============================================================================

/// Block for get_background_task - shows as `get_background_task(task_id)`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBackgroundTaskBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl GetBackgroundTaskBlock {
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
}

#[typetag::serde]
impl Block for GetBackgroundTaskBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let task_id = self.params["task_id"].as_str().unwrap_or("");

        // Format: get_background_task(task_id)
        let spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("get_background_task", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(task_id, Style::default().fg(Color::White)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ];
        lines.push(Line::from(spans));

        // Approval prompt if pending
        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        // Output if completed
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
        }

        // Denied message
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

// =============================================================================
// list_background_tasks tool
// =============================================================================

/// Tool for listing all background tasks
pub struct ListBackgroundTasksTool;

impl ListBackgroundTasksTool {
    pub const NAME: &'static str = "mcp_list_background_tasks";
}

impl Tool for ListBackgroundTasksTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "List all background tasks and their status. Returns task IDs, tool names, and status \
         (Running, Complete, or Error). Use get_background_task to retrieve results."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn compose(&self, _params: serde_json::Value) -> ToolPipeline {
        // TODO: Add a Started event so tools without approval still render
        ToolPipeline::new()
            .await_approval()
            .then(handlers::ListBackgroundTasks)
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(ListBackgroundTasksBlock::new(call_id, self.name(), params, background))
    }
}

// =============================================================================
// get_background_task tool
// =============================================================================

/// Tool for retrieving a specific background task result
pub struct GetBackgroundTaskTool;

#[derive(Debug, Deserialize)]
struct GetBackgroundTaskParams {
    /// The task_id returned when the background task was started
    task_id: String,
}

impl GetBackgroundTaskTool {
    pub const NAME: &'static str = "mcp_get_background_task";
}

impl Tool for GetBackgroundTaskTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Retrieve the result of a completed background task by its task_id. \
         The result is removed from tracking after retrieval. \
         Returns an error if the task is still running or doesn't exist."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task_id returned when the background task was started"
                }
            },
            "required": ["task_id"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: GetBackgroundTaskParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        // TODO: Add a Started event so tools without approval still render
        ToolPipeline::new()
            .await_approval()
            .then(handlers::GetBackgroundTask {
                task_id: parsed.task_id,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(GetBackgroundTaskBlock::new(call_id, self.name(), params, background))
    }
}
