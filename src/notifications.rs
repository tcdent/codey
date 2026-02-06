//! Notification system for asynchronous events.
//!
//! Notifications allow events (user messages, background completions, etc.) to be
//! delivered to the agent even when it's busy executing tools.
#![allow(dead_code)]
//!
//! - User messages → `Notification::Message`
//! - Background tool completions → `Notification::BackgroundTool`
//! - Background agent completions → `Notification::BackgroundAgent`
//! - Task list changes → `Notification::TaskListChanged`
//! - Slash commands → `Notification::Command`
//!
//! The queue supports two drain modes:
//! - `drain_injectable()` - for tool result injection (Message, BackgroundTool, BackgroundAgent)
//! - `drain_all()` - for idle processing (all notification types)
//!
//! Notifications can optionally target a specific agent by ID. When draining,
//! only untargeted notifications and those matching the target agent are returned.

use std::collections::VecDeque;

use crate::llm::AgentId;

/// A notification queued for delivery to the agent.
///
/// Notifications live in the staging area until consumed. When consumed,
/// they become blocks in the transcript.
#[derive(Debug, Clone)]
pub enum Notification {
    /// User message to send to agent
    Message {
        content: String,
        block_id: usize,
    },

    /// Slash command to execute
    Command {
        name: String,
        block_id: usize,
    },

    /// Background tool (mcp_shell with background: true, etc.) completed
    BackgroundTool {
        label: String,
        result: String,
        block_id: usize,
    },

    /// Background agent (mcp_spawn_agent) completed
    BackgroundAgent {
        label: String,
        result: String,
        block_id: usize,
    },

    /// Task list was modified by another agent
    TaskListChanged {
        summary: String,
        block_id: usize,
    },

    /// Compaction request
    Compaction {
        block_id: usize,
    },
}

impl Notification {
    /// Get the block_id for this notification.
    pub fn block_id(&self) -> usize {
        match self {
            Notification::Message { block_id, .. } => *block_id,
            Notification::Command { block_id, .. } => *block_id,
            Notification::BackgroundTool { block_id, .. } => *block_id,
            Notification::BackgroundAgent { block_id, .. } => *block_id,
            Notification::TaskListChanged { block_id, .. } => *block_id,
            Notification::Compaction { block_id } => *block_id,
        }
    }

    /// Whether this notification can interrupt a streaming turn.
    /// Commands and Compaction must wait for idle; background results can be injected.
    fn can_interrupt(&self) -> bool {
        match self {
            Notification::Message { .. }
            | Notification::BackgroundTool { .. }
            | Notification::BackgroundAgent { .. }
            | Notification::TaskListChanged { .. } => true,
            Notification::Command { .. } | Notification::Compaction { .. } => false,
        }
    }

    /// Format as XML for injection into tool results.
    /// Returns None for notifications that shouldn't be injected (Commands, Compaction).
    pub fn to_xml(&self) -> Option<String> {
        match self {
            Notification::Message { content, .. } => Some(format!(
                "<notification source=\"user\">\n{}\n</notification>",
                content
            )),
            Notification::BackgroundTool { label, result, .. } => Some(format!(
                "<notification source=\"background_task\" label=\"{}\">\n{}\n</notification>",
                label, result
            )),
            Notification::BackgroundAgent { label, result, .. } => Some(format!(
                "<notification source=\"background_agent\" label=\"{}\">\n{}\n</notification>",
                label, result
            )),
            Notification::TaskListChanged { summary, .. } => Some(format!(
                "<notification source=\"task_list\">\n{}\n</notification>",
                summary
            )),
            Notification::Command { .. } | Notification::Compaction { .. } => None,
        }
    }
}

/// A notification in the queue, optionally targeted at a specific agent.
#[derive(Debug, Clone)]
struct QueuedNotification {
    notification: Notification,
    /// If Some, only deliver to this specific agent. If None, deliver to any agent.
    target_agent_id: Option<AgentId>,
}

/// Queue for pending notifications with context-aware draining.
#[derive(Debug, Default)]
pub struct NotificationQueue {
    queue: VecDeque<QueuedNotification>,
}

impl NotificationQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Add an untargeted notification to the queue (delivered to next completing agent).
    pub fn push(&mut self, notification: Notification) {
        self.queue.push_back(QueuedNotification {
            notification,
            target_agent_id: None,
        });
    }

    /// Add a notification targeted at a specific agent.
    pub fn push_for_agent(&mut self, notification: Notification, agent_id: AgentId) {
        self.queue.push_back(QueuedNotification {
            notification,
            target_agent_id: Some(agent_id),
        });
    }

    /// Check if there are any pending notifications.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Pop the next notification (for event loop when agent is idle).
    pub fn pop(&mut self) -> Option<Notification> {
        self.queue.pop_front().map(|q| q.notification)
    }

    /// Drain notifications that can be injected into tool results for a specific agent.
    /// Returns injectable notifications that are either untargeted or targeted at this agent.
    /// Notifications targeted at other agents remain in the queue.
    pub fn drain_injectable(&mut self, agent_id: AgentId) -> Vec<Notification> {
        let mut injectable = Vec::new();
        let mut remaining = VecDeque::new();

        for queued in self.queue.drain(..) {
            if !queued.notification.can_interrupt() {
                // Non-injectable: keep in queue
                remaining.push_back(queued);
            } else if queued.target_agent_id.is_none()
                || queued.target_agent_id == Some(agent_id)
            {
                // Injectable and matches this agent (or untargeted)
                injectable.push(queued.notification);
            } else {
                // Injectable but targeted at a different agent: keep in queue
                remaining.push_back(queued);
            }
        }

        self.queue = remaining;
        injectable
    }

    /// Drain all pending notifications for idle processing.
    /// Returns all notifications at once so they can be batched.
    pub fn drain_all(&mut self) -> Vec<Notification> {
        self.queue.drain(..).map(|q| q.notification).collect()
    }

    /// Format all injectable notifications as XML for tool result injection.
    /// Returns None if no injectable notifications are pending.
    pub fn drain_injectable_xml(&mut self, agent_id: AgentId) -> Option<String> {
        let injectable = self.drain_injectable(agent_id);
        if injectable.is_empty() {
            return None;
        }

        let xml = injectable
            .iter()
            .filter_map(|n| n.to_xml())
            .collect::<Vec<_>>()
            .join("\n\n");

        Some(xml)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_can_interrupt() {
        let msg = Notification::Message {
            content: "hello".to_string(),
            block_id: 0,
        };
        assert!(msg.can_interrupt());
    }

    #[test]
    fn test_command_cannot_interrupt() {
        let cmd = Notification::Command {
            name: "help".to_string(),
            block_id: 1,
        };
        assert!(!cmd.can_interrupt());
    }

    #[test]
    fn test_background_task_can_interrupt() {
        let task = Notification::BackgroundTool {
            block_id: 0,
            label: "build".to_string(),
            result: "success".to_string(),
        };
        assert!(task.can_interrupt());
    }

    #[test]
    fn test_task_list_changed_can_interrupt() {
        let notif = Notification::TaskListChanged {
            summary: "task added".to_string(),
            block_id: 0,
        };
        assert!(notif.can_interrupt());
    }

    #[test]
    fn test_drain_injectable_filters_commands() {
        let mut queue = NotificationQueue::new();
        queue.push(Notification::Message {
            content: "msg1".to_string(),
            block_id: 0,
        });
        queue.push(Notification::Command {
            name: "help".to_string(),
            block_id: 1,
        });
        queue.push(Notification::Message {
            content: "msg2".to_string(),
            block_id: 2,
        });

        let injectable = queue.drain_injectable(0);

        // Should get both messages
        assert_eq!(injectable.len(), 2);
        // Command should remain
        assert_eq!(queue.queue.len(), 1);
        assert!(matches!(queue.pop(), Some(Notification::Command { .. })));
    }

    #[test]
    fn test_drain_injectable_agent_targeting() {
        let mut queue = NotificationQueue::new();

        // Untargeted notification
        queue.push(Notification::Message {
            content: "global".to_string(),
            block_id: 0,
        });

        // Targeted at agent 1
        queue.push_for_agent(
            Notification::TaskListChanged {
                summary: "for agent 1".to_string(),
                block_id: 1,
            },
            1,
        );

        // Targeted at agent 2
        queue.push_for_agent(
            Notification::TaskListChanged {
                summary: "for agent 2".to_string(),
                block_id: 2,
            },
            2,
        );

        // Drain for agent 1: should get the global + agent 1's notification
        let injectable = queue.drain_injectable(1);
        assert_eq!(injectable.len(), 2);

        // Agent 2's notification should remain
        assert_eq!(queue.queue.len(), 1);
        let remaining = queue.drain_injectable(2);
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn test_to_xml_message() {
        let msg = Notification::Message {
            content: "test content".to_string(),
            block_id: 0,
        };
        let xml = msg.to_xml().unwrap();
        assert!(xml.contains("source=\"user\""));
        assert!(xml.contains("test content"));
    }

    #[test]
    fn test_to_xml_task_list() {
        let notif = Notification::TaskListChanged {
            summary: "task_1 completed".to_string(),
            block_id: 0,
        };
        let xml = notif.to_xml().unwrap();
        assert!(xml.contains("source=\"task_list\""));
        assert!(xml.contains("task_1 completed"));
    }

    #[test]
    fn test_to_xml_command_returns_none() {
        let cmd = Notification::Command {
            name: "help".to_string(),
            block_id: 1,
        };
        assert!(cmd.to_xml().is_none());
    }

    #[test]
    fn test_drain_injectable_xml() {
        let mut queue = NotificationQueue::new();
        queue.push(Notification::Message {
            content: "hello".to_string(),
            block_id: 0,
        });
        queue.push(Notification::BackgroundTool {
            block_id: 1,
            label: "build".to_string(),
            result: "done".to_string(),
        });

        let xml = queue.drain_injectable_xml(0).unwrap();
        assert!(xml.contains("source=\"user\""));
        assert!(xml.contains("source=\"background_task\""));
        assert!(xml.contains("label=\"build\""));
    }
}
