//! Task list tool for multi-agent coordination.
//!
//! Provides a single tool `mcp_task_list` that allows agents to read and
//! update a task list. Mutations trigger notifications to all other agents.

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
// TaskList block
// =============================================================================

define_simple_tool_block! {
    /// Block for task_list - shows action and optional task info
    pub struct TaskListBlock {
        max_lines: 15,
        render_header(self, params) {
            let action = params["action"].as_str().unwrap_or("?");
            let detail = match action {
                "add" => params["name"].as_str().unwrap_or("").to_string(),
                "claim" | "complete" | "remove" => {
                    params["task_id"].as_str().unwrap_or("").to_string()
                }
                "update" => params["task_id"].as_str().unwrap_or("").to_string(),
                _ => String::new(),
            };

            let mut spans = vec![
                Span::styled("task_list", Style::default().fg(Color::Magenta)),
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled(action.to_string(), Style::default().fg(Color::Cyan)),
            ];

            if !detail.is_empty() {
                spans.push(Span::styled(", ", Style::default().fg(Color::DarkGray)));
                spans.push(Span::styled(detail, Style::default().fg(Color::Yellow)));
            }

            spans.push(Span::styled(")", Style::default().fg(Color::DarkGray)));
            spans
        }
    }
}

// =============================================================================
// task_list tool
// =============================================================================

/// Tool for managing the task list across agents.
pub struct TaskListTool;

#[derive(Debug, Deserialize)]
struct TaskListParams {
    /// The action to perform: list, add, claim, update, complete, remove
    action: String,
    /// Task name (required for "add")
    name: Option<String>,
    /// Task description (required for "add", optional for "update")
    description: Option<String>,
    /// Task ID (required for claim, update, complete, remove)
    task_id: Option<String>,
    /// Agent label claiming the task (required for "claim")
    agent: Option<String>,
    /// New status (optional for "update": pending, in_progress, completed)
    status: Option<String>,
}

impl TaskListTool {
    pub const NAME: &'static str = "mcp_task_list";
}

impl Tool for TaskListTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Manage a task list for coordinating work across multiple agents. \
         Actions: 'list' (view all tasks), 'add' (create a task), 'claim' (assign a task to an agent), \
         'update' (change status or description), 'complete' (mark done), 'remove' (delete a task). \
         All agents are notified when the task list changes."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The action to perform",
                    "enum": ["list", "add", "claim", "update", "complete", "remove"]
                },
                "name": {
                    "type": "string",
                    "description": "Task name (required for 'add')"
                },
                "description": {
                    "type": "string",
                    "description": "Task description (required for 'add', optional for 'update')"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (required for 'claim', 'update', 'complete', 'remove')"
                },
                "agent": {
                    "type": "string",
                    "description": "Agent label to claim the task (required for 'claim')"
                },
                "status": {
                    "type": "string",
                    "description": "New status for 'update' action",
                    "enum": ["pending", "in_progress", "completed"]
                }
            },
            "required": ["action"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: TaskListParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        match parsed.action.as_str() {
            "list" => {
                // Read-only: no approval needed, no notification
                ToolPipeline::new().then(TaskListHandler {
                    action: TaskAction::List,
                })
            }
            "add" => {
                let name = match parsed.name {
                    Some(n) => n,
                    None => return ToolPipeline::error("'name' is required for 'add' action"),
                };
                let description = parsed.description.unwrap_or_default();
                ToolPipeline::new().await_approval().then(TaskListHandler {
                    action: TaskAction::Add { name, description },
                })
            }
            "claim" => {
                let task_id = match parsed.task_id {
                    Some(id) => id,
                    None => return ToolPipeline::error("'task_id' is required for 'claim' action"),
                };
                let agent = match parsed.agent {
                    Some(a) => a,
                    None => return ToolPipeline::error("'agent' is required for 'claim' action"),
                };
                ToolPipeline::new().await_approval().then(TaskListHandler {
                    action: TaskAction::Claim { task_id, agent },
                })
            }
            "update" => {
                let task_id = match parsed.task_id {
                    Some(id) => id,
                    None => {
                        return ToolPipeline::error("'task_id' is required for 'update' action")
                    }
                };
                let status = parsed.status.map(|s| match s.as_str() {
                    "pending" => crate::tasks::TaskStatus::Pending,
                    "in_progress" => crate::tasks::TaskStatus::InProgress,
                    "completed" => crate::tasks::TaskStatus::Completed,
                    _ => crate::tasks::TaskStatus::Pending,
                });
                ToolPipeline::new().await_approval().then(TaskListHandler {
                    action: TaskAction::Update {
                        task_id,
                        status,
                        description: parsed.description,
                    },
                })
            }
            "complete" => {
                let task_id = match parsed.task_id {
                    Some(id) => id,
                    None => {
                        return ToolPipeline::error("'task_id' is required for 'complete' action")
                    }
                };
                ToolPipeline::new().await_approval().then(TaskListHandler {
                    action: TaskAction::Complete { task_id },
                })
            }
            "remove" => {
                let task_id = match parsed.task_id {
                    Some(id) => id,
                    None => {
                        return ToolPipeline::error("'task_id' is required for 'remove' action")
                    }
                };
                ToolPipeline::new().await_approval().then(TaskListHandler {
                    action: TaskAction::Remove { task_id },
                })
            }
            other => ToolPipeline::error(format!(
                "Unknown action '{}'. Use: list, add, claim, update, complete, remove",
                other
            )),
        }
    }

    fn create_block(
        &self,
        call_id: &str,
        params: serde_json::Value,
        background: bool,
    ) -> Box<dyn Block> {
        Box::new(TaskListBlock::new(
            call_id,
            self.name(),
            params,
            background,
        ))
    }
}

// =============================================================================
// Handler - performs the action and delegates mutations for notification
// =============================================================================

/// Internal action representation after parameter validation.
enum TaskAction {
    List,
    Add {
        name: String,
        description: String,
    },
    Claim {
        task_id: String,
        agent: String,
    },
    Update {
        task_id: String,
        status: Option<crate::tasks::TaskStatus>,
        description: Option<String>,
    },
    Complete {
        task_id: String,
    },
    Remove {
        task_id: String,
    },
}

struct TaskListHandler {
    action: TaskAction,
}

#[async_trait::async_trait]
impl crate::tools::pipeline::EffectHandler for TaskListHandler {
    async fn call(self: Box<Self>) -> crate::tools::pipeline::Step {
        use crate::tasks::with_task_list;
        use crate::tools::pipeline::Step;

        match self.action {
            TaskAction::List => {
                let formatted = with_task_list(|list| list.format());
                Step::Output(formatted)
            }
            action => {
                // All mutations happen inside the global lock
                let result = with_task_list(|list| match action {
                    TaskAction::Add { name, description } => {
                        let id = list.add(name.clone(), description);
                        let msg = format!("Added task '{}' ({})", name, id);
                        let summary = format!("{}\n\nCurrent tasks:\n{}", msg, list.format());
                        Ok(summary)
                    }
                    TaskAction::Claim { task_id, agent } => list
                        .claim(&task_id, &agent)
                        .map(|()| {
                            let msg = format!("Task '{}' claimed by '{}'", task_id, agent);
                            format!("{}\n\nCurrent tasks:\n{}", msg, list.format())
                        }),
                    TaskAction::Update {
                        task_id,
                        status,
                        description,
                    } => list
                        .update(&task_id, status, description)
                        .map(|()| {
                            let msg = format!("Task '{}' updated", task_id);
                            format!("{}\n\nCurrent tasks:\n{}", msg, list.format())
                        }),
                    TaskAction::Complete { task_id } => list
                        .complete(&task_id)
                        .map(|()| {
                            let msg = format!("Task '{}' completed", task_id);
                            format!("{}\n\nCurrent tasks:\n{}", msg, list.format())
                        }),
                    TaskAction::Remove { task_id } => list
                        .remove(&task_id)
                        .map(|()| {
                            let msg = format!("Task '{}' removed", task_id);
                            format!("{}\n\nCurrent tasks:\n{}", msg, list.format())
                        }),
                    TaskAction::List => unreachable!(),
                });

                match result {
                    Ok(summary) => {
                        // Delegate to app for notification broadcasting
                        Step::Delegate(crate::effect::Effect::TaskListChanged {
                            summary,
                        })
                    }
                    Err(e) => Step::Error(e),
                }
            }
        }
    }
}
