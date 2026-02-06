//! Shared task list for multi-agent coordination.
//!
//! Provides a persistent JSON task list that multiple agents can read and update.
//! Each task has a name, description, status, and optional agent claim.
//! Stored at `~/.codey/shared_tasks.json`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::config::CODEY_DIR;

const SHARED_TASKS_FILENAME: &str = "shared_tasks.json";

/// Status of a shared task.
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

/// A single task in the shared task list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedTask {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: TaskStatus,
    /// Agent label that has claimed this task (None = unclaimed).
    pub agent: Option<String>,
}

/// The full shared task list.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedTaskList {
    pub tasks: Vec<SharedTask>,
    #[serde(default)]
    next_id: u32,
}

impl SharedTaskList {
    /// Path to the shared tasks JSON file.
    fn path() -> PathBuf {
        PathBuf::from(CODEY_DIR).join(SHARED_TASKS_FILENAME)
    }

    /// Load the task list from disk. Returns an empty list if the file doesn't exist.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save the task list to disk.
    pub fn save(&self) -> Result<(), String> {
        let codey_dir = PathBuf::from(CODEY_DIR);
        if !codey_dir.exists() {
            fs::create_dir_all(&codey_dir)
                .map_err(|e| format!("Failed to create {} directory: {}", CODEY_DIR, e))?;
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize task list: {}", e))?;

        fs::write(Self::path(), json)
            .map_err(|e| format!("Failed to write {}: {}", Self::path().display(), e))
    }

    /// Generate a new unique task ID.
    fn next_id(&mut self) -> String {
        self.next_id += 1;
        format!("task_{}", self.next_id)
    }

    /// Add a new task. Returns the assigned task ID.
    pub fn add(&mut self, name: String, description: String) -> String {
        let id = self.next_id();
        self.tasks.push(SharedTask {
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
            return "No shared tasks.".to_string();
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
        let mut list = SharedTaskList::default();
        let id = list.add("Fix bug".into(), "Fix the login bug".into());
        assert_eq!(id, "task_1");
        assert_eq!(list.tasks.len(), 1);
        assert!(list.format().contains("Fix bug"));
    }

    #[test]
    fn test_claim() {
        let mut list = SharedTaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.claim(&id, "agent-1").unwrap();
        assert_eq!(list.tasks[0].agent.as_deref(), Some("agent-1"));
        assert_eq!(list.tasks[0].status, TaskStatus::InProgress);
    }

    #[test]
    fn test_claim_already_claimed() {
        let mut list = SharedTaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.claim(&id, "agent-1").unwrap();
        assert!(list.claim(&id, "agent-2").is_err());
    }

    #[test]
    fn test_complete() {
        let mut list = SharedTaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.complete(&id).unwrap();
        assert_eq!(list.tasks[0].status, TaskStatus::Completed);
    }

    #[test]
    fn test_remove() {
        let mut list = SharedTaskList::default();
        let id = list.add("Task 1".into(), "Description".into());
        list.remove(&id).unwrap();
        assert!(list.tasks.is_empty());
    }

    #[test]
    fn test_update() {
        let mut list = SharedTaskList::default();
        let id = list.add("Task 1".into(), "Original".into());
        list.update(&id, Some(TaskStatus::InProgress), Some("Updated".into()))
            .unwrap();
        assert_eq!(list.tasks[0].status, TaskStatus::InProgress);
        assert_eq!(list.tasks[0].description, "Updated");
    }

    #[test]
    fn test_empty_format() {
        let list = SharedTaskList::default();
        assert_eq!(list.format(), "No shared tasks.");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut list = SharedTaskList::default();
        list.add("Task 1".into(), "Desc 1".into());
        list.add("Task 2".into(), "Desc 2".into());
        list.claim("task_1", "agent-1").unwrap();

        let json = serde_json::to_string(&list).unwrap();
        let deserialized: SharedTaskList = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tasks.len(), 2);
        assert_eq!(deserialized.tasks[0].agent.as_deref(), Some("agent-1"));
    }
}
