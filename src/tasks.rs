//! Task list for multi-agent coordination.
//!
//! Provides an in-memory task list that multiple agents can read and update.
//! Each task has a name, description, status, and optional agent claim.
//! The list lives for the duration of the session (not persisted to disk).

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Global task list, shared across all agents in the session.
static TASK_LIST: Mutex<Option<TaskList>> = Mutex::new(None);

/// Access the global task list. Initializes on first access.
pub fn with_task_list<F, R>(f: F) -> R
where
    F: FnOnce(&mut TaskList) -> R,
{
    let mut guard = TASK_LIST.lock().unwrap();
    let list = guard.get_or_insert_with(TaskList::default);
    f(list)
}

/// Status of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
        }
    }
}

/// A single task in the task list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: TaskStatus,
    /// Agent label that has claimed this task (None = unclaimed).
    pub agent: Option<String>,
}

/// The task list.
#[derive(Debug, Clone, Default)]
pub struct TaskList {
    pub tasks: Vec<Task>,
    next_id: u32,
}

impl TaskList {
    /// Generate a new unique task ID.
    fn next_id(&mut self) -> String {
        self.next_id += 1;
        format!("task_{}", self.next_id)
    }

    /// Add a new task. Returns the assigned task ID.
    pub fn add(&mut self, name: String, description: String) -> String {
        let id = self.next_id();
        self.tasks.push(Task {
            id: id.clone(),
            name,
            description,
            status: TaskStatus::Pending,
            agent: None,
        });
        id
    }

    /// Claim a task for an agent. Sets status to InProgress.
    pub fn claim(&mut self, task_id: &str, agent: &str) -> Result<(), String> {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        if let Some(ref existing) = task.agent {
            if existing != agent {
                return Err(format!(
                    "Task '{}' is already claimed by '{}'",
                    task_id, existing
                ));
            }
        }

        task.agent = Some(agent.to_string());
        task.status = TaskStatus::InProgress;
        Ok(())
    }

    /// Update a task's status and/or description.
    pub fn update(
        &mut self,
        task_id: &str,
        status: Option<TaskStatus>,
        description: Option<String>,
    ) -> Result<(), String> {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        if let Some(s) = status {
            task.status = s;
        }
        if let Some(d) = description {
            task.description = d;
        }
        Ok(())
    }

    /// Mark a task as completed.
    pub fn complete(&mut self, task_id: &str) -> Result<(), String> {
        let task = self
            .tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        task.status = TaskStatus::Completed;
        Ok(())
    }

    /// Remove a task by ID.
    pub fn remove(&mut self, task_id: &str) -> Result<(), String> {
        let idx = self
            .tasks
            .iter()
            .position(|t| t.id == task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        self.tasks.remove(idx);
        Ok(())
    }

    /// Format the task list as a human-readable string.
    pub fn format(&self) -> String {
        if self.tasks.is_empty() {
            return "No tasks.".to_string();
        }

        self.tasks
            .iter()
            .map(|t| {
                let agent_str = t
                    .agent
                    .as_deref()
                    .map(|a| format!(" (agent: {})", a))
                    .unwrap_or_default();
                format!(
                    "- [{}] {} [{}]{}\n  {}",
                    t.id, t.name, t.status, agent_str, t.description
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_format() {
        let mut list = TaskList::default();
        let id = list.add("Fix bug".into(), "Fix the login bug".into());
        assert_eq!(id, "task_1");
        assert_eq!(list.tasks.len(), 1);
        assert!(list.format().contains("Fix bug"));
    }

    #[test]
    fn test_claim() {
        let mut list = TaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.claim(&id, "agent-1").unwrap();
        assert_eq!(list.tasks[0].agent.as_deref(), Some("agent-1"));
        assert_eq!(list.tasks[0].status, TaskStatus::InProgress);
    }

    #[test]
    fn test_claim_already_claimed() {
        let mut list = TaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.claim(&id, "agent-1").unwrap();
        assert!(list.claim(&id, "agent-2").is_err());
    }

    #[test]
    fn test_complete() {
        let mut list = TaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.complete(&id).unwrap();
        assert_eq!(list.tasks[0].status, TaskStatus::Completed);
    }

    #[test]
    fn test_remove() {
        let mut list = TaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.remove(&id).unwrap();
        assert!(list.tasks.is_empty());
    }

    #[test]
    fn test_update() {
        let mut list = TaskList::default();
        let id = list.add("Task 1".into(), "Original".into());
        list.update(&id, Some(TaskStatus::InProgress), Some("Updated".into()))
            .unwrap();
        assert_eq!(list.tasks[0].status, TaskStatus::InProgress);
        assert_eq!(list.tasks[0].description, "Updated");
    }

    #[test]
    fn test_empty_format() {
        let list = TaskList::default();
        assert_eq!(list.format(), "No tasks.");
    }
}
