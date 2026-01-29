//! Notification system for asynchronous events.
//!
//! Notifications allow events (user messages, background completions, etc.) to be
//! delivered to the agent even when it's busy executing tools.
//!
//! - User messages → `Notification::Message`
//! - Background tool completions → `Notification::BackgroundTool`
//! - Background agent completions → `Notification::BackgroundAgent`
//! - Slash commands → `Notification::Command`
//!
//! The queue supports two drain modes:
//! - `drain_injectable()` - for tool result injection (Message, BackgroundTool, BackgroundAgent)
//! - `drain_all()` - for idle processing (all notification types)

use std::collections::VecDeque;

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
            Notification::Compaction { block_id } => *block_id,
        }
    }

    /// Whether this notification can interrupt a streaming turn.
    /// Commands and Compaction must wait for idle; background results can be injected.
    fn can_interrupt(&self) -> bool {
        match self {
            Notification::Message { .. } 
            | Notification::BackgroundTool { .. } 
            | Notification::BackgroundAgent { .. } => true,
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
            Notification::Command { .. } | Notification::Compaction { .. } => None,
        }
    }
}

/// Queue for pending notifications with context-aware draining.
#[derive(Debug, Default)]
pub struct NotificationQueue {
    queue: VecDeque<Notification>,
}

impl NotificationQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Add a notification to the queue.
    pub fn push(&mut self, notification: Notification) {
        self.queue.push_back(notification);
    }

    /// Check if there are any pending notifications.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Pop the next notification (for event loop when agent is idle).
    pub fn pop(&mut self) -> Option<Notification> {
        self.queue.pop_front()
    }

    /// Drain notifications that can be injected into tool results.
    /// Returns Message and BackgroundTool notifications; Commands remain queued.
    pub fn drain_injectable(&mut self) -> Vec<Notification> {
        let mut injectable = Vec::new();
        let mut remaining = VecDeque::new();

        for notification in self.queue.drain(..) {
            if notification.can_interrupt() {
                injectable.push(notification);
            } else {
                remaining.push_back(notification);
            }
        }

        self.queue = remaining;
        injectable
    }

    /// Drain all pending notifications for idle processing.
    /// Returns all notifications at once so they can be batched.
    pub fn drain_all(&mut self) -> Vec<Notification> {
        self.queue.drain(..).collect()
    }

    /// Format all injectable notifications as XML for tool result injection.
    /// Returns None if no injectable notifications are pending.
    pub fn drain_injectable_xml(&mut self) -> Option<String> {
        let injectable = self.drain_injectable();
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

        let injectable = queue.drain_injectable();

        // Should get both messages
        assert_eq!(injectable.len(), 2);
        // Command should remain
        assert_eq!(queue.queue.len(), 1);
        assert!(matches!(queue.pop(), Some(Notification::Command { .. })));
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

        let xml = queue.drain_injectable_xml().unwrap();
        assert!(xml.contains("source=\"user\""));
        assert!(xml.contains("source=\"background_task\""));
        assert!(xml.contains("label=\"build\""));
    }
}
