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
use crate::define_simple_tool_block;
use crate::transcript::{render_approval_prompt, render_prefix, render_result, Block, BlockType, Status};

// =============================================================================
// ListBackgroundTasks block
// =============================================================================

define_simple_tool_block! {
    /// Block for list_background_tasks - shows as `list_background_tasks()`
    pub struct ListBackgroundTasksBlock {
        max_lines: 10,
        render_header(self, params) {
            vec![
                Span::styled("list_background_tasks", Style::default().fg(Color::Magenta)),
                Span::styled("()", Style::default().fg(Color::DarkGray)),
            ]
        }
    }
}

// =============================================================================
// GetBackgroundTask block
// =============================================================================

define_simple_tool_block! {
    /// Block for get_background_task - shows as `get_background_task(task_id)`
    pub struct GetBackgroundTaskBlock {
        max_lines: 10,
        render_header(self, params) {
            let task_id = params["task_id"].as_str().unwrap_or("");

            vec![
                Span::styled("get_background_task", Style::default().fg(Color::Magenta)),
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled(task_id.to_string(), Style::default().fg(Color::White)),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]
        }
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
