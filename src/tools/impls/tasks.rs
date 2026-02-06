//! Shared task list tool for multi-agent coordination.
//!
//! Provides a single tool `mcp_shared_task_list` that allows agents to read and
//! update a shared task list. Mutations trigger notifications to all other agents.

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
// SharedTaskList block
// =============================================================================

define_simple_tool_block! {
    /// Block for shared_task_list - shows action and optional task info
    pub struct SharedTaskListBlock {
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
                Span::styled("shared_task_list", Style::default().fg(Color::Magenta)),
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
// shared_task_list tool
// =============================================================================

/// Tool for managing the shared task list across agents.
pub struct SharedTaskListTool;

#[derive(Debug, Deserialize)]
struct SharedTaskListParams {
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

impl SharedTaskListTool {
    pub const NAME: &'static str = "mcp_shared_task_list";
}

impl Tool for SharedTaskListTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Manage a shared task list for coordinating work across multiple agents. \
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
        let parsed: SharedTaskListParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        match parsed.action.as_str() {
            "list" => {
                // Read-only: no approval needed, no notification
                ToolPipeline::new().then(SharedTaskListHandler {
                    action: TaskAction::List,
                })
            }
            "add" => {
                let name = match parsed.name {
                    Some(n) => n,
                    None => return ToolPipeline::error("'name' is required for 'add' action"),
                };
                let description = parsed.description.unwrap_or_default();
                ToolPipeline::new().await_approval().then(SharedTaskListHandler {
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
                ToolPipeline::new().await_approval().then(SharedTaskListHandler {
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
                    "pending" => crate::shared_tasks::TaskStatus::Pending,
                    "in_progress" => crate::shared_tasks::TaskStatus::InProgress,
                    "completed" => crate::shared_tasks::TaskStatus::Completed,
                    _ => crate::shared_tasks::TaskStatus::Pending,
                });
                ToolPipeline::new().await_approval().then(SharedTaskListHandler {
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
                ToolPipeline::new().await_approval().then(SharedTaskListHandler {
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
                ToolPipeline::new().await_approval().then(SharedTaskListHandler {
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
        Box::new(SharedTaskListBlock::new(
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
        status: Option<crate::shared_tasks::TaskStatus>,
        description: Option<String>,
    },
    Complete {
        task_id: String,
    },
    Remove {
        task_id: String,
    },
}

struct SharedTaskListHandler {
    action: TaskAction,
}

#[async_trait::async_trait]
impl crate::tools::pipeline::EffectHandler for SharedTaskListHandler {
    async fn call(self: Box<Self>) -> crate::tools::pipeline::Step {
        use crate::shared_tasks::SharedTaskList;
        use crate::tools::pipeline::Step;

        match self.action {
            TaskAction::List => {
                let list = SharedTaskList::load();
                Step::Output(list.format())
            }
            action => {
                // All mutations: load, mutate, save, then delegate for notification
                let mut list = SharedTaskList::load();

                let result = match action {
                    TaskAction::Add { name, description } => {
                        let id = list.add(name.clone(), description);
                        Ok(format!("Added task '{}' ({})", name, id))
                    }
                    TaskAction::Claim { task_id, agent } => list
                        .claim(&task_id, &agent)
                        .map(|()| format!("Task '{}' claimed by '{}'", task_id, agent)),
                    TaskAction::Update {
                        task_id,
                        status,
                        description,
                    } => list
                        .update(&task_id, status, description)
                        .map(|()| format!("Task '{}' updated", task_id)),
                    TaskAction::Complete { task_id } => list
                        .complete(&task_id)
                        .map(|()| format!("Task '{}' completed", task_id)),
                    TaskAction::Remove { task_id } => list
                        .remove(&task_id)
                        .map(|()| format!("Task '{}' removed", task_id)),
                    TaskAction::List => unreachable!(),
                };

                match result {
                    Ok(msg) => {
                        if let Err(e) = list.save() {
                            return Step::Error(format!("Task list update failed: {}", e));
                        }
                        // Delegate to app for notification broadcasting
                        let summary = format!("{}\n\nCurrent tasks:\n{}", msg, list.format());
                        Step::Delegate(crate::effect::Effect::SharedTaskListChanged {
                            summary: summary.clone(),
                        })
                    }
                    Err(e) => Step::Error(e),
                }
            }
        }
    }
}
