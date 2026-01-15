//! Background task management tools
//!
//! Tools for querying and retrieving results from background tool executions.

use serde::Deserialize;
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::transcript::{Block, ToolBlock};

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
        Box::new(ToolBlock::new(call_id, self.name(), params, background))
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
        Box::new(ToolBlock::new(call_id, self.name(), params, background))
    }
}
