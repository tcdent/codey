//! Effects and effect queue management.
//!
//! Effects are actions that tools delegate to the app layer, such as:
//! - Requesting user approval for a tool call
//! - Opening files or showing previews in the IDE
//! - Spawning sub-agents
//!
//! The `EffectQueue` (CLI-only) manages pending effects with resource exclusivity:
//! - Only one approval can be shown at a time
//! - Only one IDE preview can be active at a time
#![allow(dead_code)]
//! - Effects are polled until ready

#[cfg(feature = "cli")]
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

#[cfg(feature = "cli")]
use tokio::sync::oneshot;

use crate::ide::{Edit, ToolPreview};
#[cfg(feature = "cli")]
use crate::llm::Agent;
#[cfg(feature = "cli")]
use crate::llm::AgentId;

/// Result type for effect execution
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub type EffectResult = Result<Option<String>, String>;

/// Effects that tools delegate to the app layer
pub enum Effect {
    // === Approval ===
    /// Request user approval for a tool call
    AwaitApproval {
        name: String,
        params: serde_json::Value,
        background: bool,
    },

    // === IDE ===
    IdeOpen {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    IdeShowPreview {
        preview: ToolPreview,
    },
    IdeShowDiffPreview {
        path: PathBuf,
        edits: Vec<Edit>,
    },
    IdeReloadBuffer {
        path: PathBuf,
    },
    IdeClosePreview,
    /// Check if IDE buffer has unsaved changes - fails pipeline if dirty
    IdeCheckUnsavedEdits {
        path: PathBuf,
    },

    // === Background Tasks ===
    ListBackgroundTasks,
    GetBackgroundTask {
        task_id: String,
    },

    // === Sub-Agents ===
    /// Spawn a sub-agent. App registers it and polls through main loop.
    #[cfg(feature = "cli")]
    SpawnAgent {
        agent: Agent,
        label: String,
    },

    // === Agent Management ===
    /// List all spawned agents
    #[cfg(feature = "cli")]
    ListAgents,
    /// Get result from a finished agent
    #[cfg(feature = "cli")]
    GetAgent {
        label: String,
    },
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::AwaitApproval { name, .. } => f
                .debug_struct("AwaitApproval")
                .field("name", name)
                .finish_non_exhaustive(),
            Effect::IdeOpen { path, line, column } => f
                .debug_struct("IdeOpen")
                .field("path", path)
                .field("line", line)
                .field("column", column)
                .finish(),
            Effect::IdeShowPreview { .. } => f.write_str("IdeShowPreview"),
            Effect::IdeShowDiffPreview { path, .. } => f
                .debug_struct("IdeShowDiffPreview")
                .field("path", path)
                .finish_non_exhaustive(),
            Effect::IdeReloadBuffer { path } => {
                f.debug_struct("IdeReloadBuffer").field("path", path).finish()
            }
            Effect::IdeClosePreview => f.write_str("IdeClosePreview"),
            Effect::IdeCheckUnsavedEdits { path } => f
                .debug_struct("IdeCheckUnsavedEdits")
                .field("path", path)
                .finish(),
            Effect::ListBackgroundTasks => f.write_str("ListBackgroundTasks"),
            Effect::GetBackgroundTask { task_id } => f
                .debug_struct("GetBackgroundTask")
                .field("task_id", task_id)
                .finish(),
            #[cfg(feature = "cli")]
            Effect::SpawnAgent { label, .. } => f
                .debug_struct("SpawnAgent")
                .field("label", label)
                .finish_non_exhaustive(),
            #[cfg(feature = "cli")]
            Effect::ListAgents => f.write_str("ListAgents"),
            #[cfg(feature = "cli")]
            Effect::GetAgent { label } => {
                f.debug_struct("GetAgent").field("label", &label).finish()
            }
        }
    }
}

// ============================================================================
// Effect Queue Management (CLI-only)
// ============================================================================

/// Exclusive resources that only one effect can hold at a time
#[cfg(feature = "cli")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub enum Resource {
    /// Approval UI slot - only one approval shown at a time
    ApprovalSlot,
    /// IDE preview slot - only one preview shown at a time
    IdePreview,
}

/// Result of polling an effect
#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub enum EffectPoll {
    /// Effect completed with this result
    Ready(anyhow::Result<Option<String>>),
    /// Effect is waiting, poll again later
    Pending,
}

/// An effect waiting to be executed or polled
#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub struct PendingEffect {
    pub call_id: String,
    pub agent_id: AgentId,
    pub effect: Effect,
    pub responder: oneshot::Sender<EffectResult>,
    /// For approval effects, whether UI has been shown to user
    pub acknowledged: bool,
}

#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
impl PendingEffect {
    /// Create a new pending effect
    pub fn new(
        call_id: String,
        agent_id: AgentId,
        effect: Effect,
        responder: oneshot::Sender<EffectResult>,
    ) -> Self {
        Self {
            call_id,
            agent_id,
            effect,
            responder,
            acknowledged: false,
        }
    }

    /// Get the exclusive resource this effect needs, if any
    pub fn resource(&self) -> Option<Resource> {
        match &self.effect {
            Effect::AwaitApproval { .. } => Some(Resource::ApprovalSlot),
            Effect::IdeShowPreview { .. } | Effect::IdeShowDiffPreview { .. } => {
                Some(Resource::IdePreview)
            }
            _ => None,
        }
    }

    /// Check if this is an approval effect
    pub fn is_approval(&self) -> bool {
        matches!(self.effect, Effect::AwaitApproval { .. })
    }

    /// Mark this approval as acknowledged (UI shown to user)
    pub fn acknowledge(&mut self) {
        self.acknowledged = true;
    }

    /// Send a result back to the executor and consume this effect
    pub fn complete(self, result: EffectResult) {
        let _ = self.responder.send(result);
    }
}

/// Manages the queue of pending effects, handling resource exclusivity and polling.
#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub struct EffectQueue {
    pending: VecDeque<PendingEffect>,
}

#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
impl EffectQueue {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
        }
    }

    /// Add a new effect to the queue
    pub fn push(&mut self, effect: PendingEffect) {
        self.pending.push_back(effect);
    }

    /// Re-queue an effect that wasn't ready (puts at back of queue)
    pub fn requeue(&mut self, effect: PendingEffect) {
        self.pending.push_back(effect);
    }

    /// Check if there are any effects that can be polled.
    ///
    /// An effect is pollable if:
    /// - It's not an acknowledged approval (those wait for user input)
    /// - No acknowledged approval exists (only one approval shown at a time)
    /// - No prior effect in the queue holds its required resource
    pub fn has_pollable(&self) -> bool {
        let mut claimed: HashSet<Resource> = HashSet::new();

        // If ANY approval is acknowledged, claim the approval slot upfront.
        // This ensures only one approval is ever displayed at a time.
        if self.pending.iter().any(|p| p.is_approval() && p.acknowledged) {
            claimed.insert(Resource::ApprovalSlot);
        }

        self.pending.iter().any(|p| {
            // Acknowledged approvals aren't pollable (waiting for user input)
            if p.is_approval() && p.acknowledged {
                return false;
            }

            // Check if this effect's resource is already claimed
            if let Some(resource) = p.resource() {
                if claimed.contains(&resource) {
                    return false;
                }
                claimed.insert(resource);
            }

            true
        })
    }

    /// Get the next effect that can be polled, removing it from the queue.
    ///
    /// Returns None if no effects are pollable.
    pub fn poll_next(&mut self) -> Option<PendingEffect> {
        let mut claimed: HashSet<Resource> = HashSet::new();

        // If ANY approval is acknowledged, claim the approval slot upfront.
        // This ensures only one approval is ever displayed at a time.
        if self.pending.iter().any(|p| p.is_approval() && p.acknowledged) {
            claimed.insert(Resource::ApprovalSlot);
        }

        let idx = self.pending.iter().position(|p| {
            // Acknowledged approvals aren't pollable (waiting for user input)
            if p.is_approval() && p.acknowledged {
                return false;
            }

            // Check if this effect's resource is already claimed
            if let Some(resource) = p.resource() {
                if claimed.contains(&resource) {
                    return false;
                }
                claimed.insert(resource);
            }

            true
        })?;

        self.pending.remove(idx)
    }

    /// Take the current active (acknowledged) approval for a user decision.
    /// Returns None if no approval is currently being shown to the user.
    pub fn take_active_approval(&mut self) -> Option<PendingEffect> {
        let idx = self
            .pending
            .iter()
            .position(|p| p.is_approval() && p.acknowledged)?;
        self.pending.remove(idx)
    }

    /// Check if there's an active approval being shown to the user
    pub fn has_active_approval(&self) -> bool {
        self.pending
            .iter()
            .any(|p| p.is_approval() && p.acknowledged)
    }

    /// Check if there are any pending approvals (acknowledged or not)
    pub fn has_pending_approvals(&self) -> bool {
        self.pending.iter().any(|p| p.is_approval())
    }

    /// Find a pending effect by call_id (for updating status, etc.)
    pub fn find_by_call_id(&self, call_id: &str) -> Option<&PendingEffect> {
        self.pending.iter().find(|p| p.call_id == call_id)
    }

    /// Find a pending effect by call_id (mutable)
    pub fn find_by_call_id_mut(&mut self, call_id: &str) -> Option<&mut PendingEffect> {
        self.pending.iter_mut().find(|p| p.call_id == call_id)
    }
}

#[cfg(feature = "cli")]
impl Default for EffectQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg(feature = "cli")]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn make_approval_effect(call_id: &str, name: &str) -> PendingEffect {
        let (tx, _rx) = oneshot::channel();
        PendingEffect::new(
            call_id.to_string(),
            0, // AgentId
            Effect::AwaitApproval {
                name: name.to_string(),
                params: serde_json::json!({}),
                background: false,
            },
            tx,
        )
    }

    fn make_ide_effect(call_id: &str) -> PendingEffect {
        let (tx, _rx) = oneshot::channel();
        PendingEffect::new(
            call_id.to_string(),
            0,
            Effect::IdeOpen {
                path: std::path::PathBuf::from("/tmp/test"),
                line: Some(1),
                column: None,
            },
            tx,
        )
    }

    #[test]
    fn test_effect_queue_approval_slot_exclusivity() {
        let mut queue = EffectQueue::new();

        // Queue multiple approvals
        queue.push(make_approval_effect("call_1", "shell"));
        queue.push(make_approval_effect("call_2", "read_file"));
        queue.push(make_approval_effect("call_3", "write_file"));

        // First poll should return first approval
        let polled = queue.poll_next();
        assert!(polled.is_some());
        assert_eq!(polled.unwrap().call_id, "call_1");

        // Next poll should return second (first is gone)
        let polled = queue.poll_next();
        assert!(polled.is_some());
        assert_eq!(polled.unwrap().call_id, "call_2");
    }

    #[test]
    fn test_effect_queue_acknowledged_blocks_polling() {
        let mut queue = EffectQueue::new();

        // Queue approvals
        queue.push(make_approval_effect("call_1", "shell"));
        queue.push(make_approval_effect("call_2", "read_file"));
        queue.push(make_approval_effect("call_3", "write_file"));

        // Acknowledge first one (simulating UI shown)
        if let Some(effect) = queue.find_by_call_id_mut("call_1") {
            effect.acknowledge();
        }

        // Polling should skip acknowledged and return call_2
        // But wait - call_1 claims the slot, so nothing should be pollable
        assert!(!queue.has_pollable());
    }

    #[test]
    fn test_effect_queue_take_active_approval() {
        let mut queue = EffectQueue::new();

        queue.push(make_approval_effect("call_1", "shell"));
        queue.push(make_approval_effect("call_2", "read_file"));

        // Acknowledge call_1
        if let Some(effect) = queue.find_by_call_id_mut("call_1") {
            effect.acknowledge();
        }

        assert!(queue.has_active_approval());

        // Take the active approval
        let active = queue.take_active_approval();
        assert!(active.is_some());
        assert_eq!(active.unwrap().call_id, "call_1");

        // No more active approvals
        assert!(!queue.has_active_approval());

        // But call_2 is still pending
        assert!(queue.has_pending_approvals());
    }

    #[test]
    fn test_effect_queue_mixed_effect_types() {
        let mut queue = EffectQueue::new();

        // Mix of approvals and IDE effects
        queue.push(make_approval_effect("call_1", "shell"));
        queue.push(make_ide_effect("ide_1"));
        queue.push(make_approval_effect("call_2", "read_file"));
        queue.push(make_ide_effect("ide_2"));

        // Poll first - should be call_1 (approval)
        let polled = queue.poll_next();
        assert_eq!(polled.unwrap().call_id, "call_1");

        // Next should be ide_1 (IDE effect, different resource)
        let polled = queue.poll_next();
        assert_eq!(polled.unwrap().call_id, "ide_1");
    }

    #[test]
    fn test_effect_queue_high_volume() {
        let mut queue = EffectQueue::new();

        // Simulate rapid queuing from multiple agents
        for i in 0..50 {
            queue.push(make_approval_effect(
                &format!("agent{}_{}", i % 4, i),
                if i % 2 == 0 { "shell" } else { "read_file" },
            ));
        }

        assert_eq!(queue.pending.len(), 50);

        // Acknowledge one in the middle
        if let Some(effect) = queue.find_by_call_id_mut("agent2_10") {
            effect.acknowledge();
        }

        // Should have active approval
        assert!(queue.has_active_approval());

        // Polling blocked by acknowledged approval
        assert!(!queue.has_pollable());

        // Take the active approval (user decision)
        let active = queue.take_active_approval();
        assert!(active.is_some());
        assert_eq!(active.unwrap().call_id, "agent2_10");

        // Now polling should work
        assert!(queue.has_pollable());
        let next = queue.poll_next();
        assert_eq!(next.unwrap().call_id, "agent0_0");
    }

    #[test]
    fn test_effect_queue_requeue() {
        let mut queue = EffectQueue::new();

        queue.push(make_ide_effect("ide_1"));
        queue.push(make_ide_effect("ide_2"));

        // Poll first
        let polled = queue.poll_next();
        assert_eq!(polled.as_ref().unwrap().call_id, "ide_1");

        // Requeue it (simulating pending state)
        queue.requeue(polled.unwrap());

        // Now ide_2 should come next (ide_1 went to back)
        let polled = queue.poll_next();
        assert_eq!(polled.unwrap().call_id, "ide_2");

        // And ide_1 is last
        let polled = queue.poll_next();
        assert_eq!(polled.unwrap().call_id, "ide_1");
    }

    #[test]
    fn test_effect_queue_find_by_call_id_consistency() {
        let mut queue = EffectQueue::new();

        for i in 0..20 {
            queue.push(make_approval_effect(&format!("call_{}", i), "shell"));
        }

        // All should be findable
        for i in 0..20 {
            assert!(
                queue.find_by_call_id(&format!("call_{}", i)).is_some(),
                "Failed to find call_{}",
                i
            );
        }

        // Poll some away
        for _ in 0..5 {
            queue.poll_next();
        }

        // First 5 gone
        for i in 0..5 {
            assert!(queue.find_by_call_id(&format!("call_{}", i)).is_none());
        }

        // Rest still there
        for i in 5..20 {
            assert!(queue.find_by_call_id(&format!("call_{}", i)).is_some());
        }
    }

    /// Regression test: acknowledging an approval that's NOT first in queue
    /// should still block all other approvals from being polled.
    #[test]
    fn test_acknowledged_approval_blocks_all_approvals_regardless_of_position() {
        let mut queue = EffectQueue::new();

        // Queue 5 approvals
        queue.push(make_approval_effect("call_0", "shell"));
        queue.push(make_approval_effect("call_1", "shell"));
        queue.push(make_approval_effect("call_2", "shell"));
        queue.push(make_approval_effect("call_3", "shell"));
        queue.push(make_approval_effect("call_4", "shell"));

        // Acknowledge the THIRD one (not first!)
        if let Some(effect) = queue.find_by_call_id_mut("call_2") {
            effect.acknowledge();
        }

        // Even though call_0 and call_1 are before call_2 in the queue,
        // they should NOT be pollable because call_2 is acknowledged
        assert!(!queue.has_pollable(), "No approvals should be pollable when one is acknowledged");

        // poll_next should return None
        assert!(queue.poll_next().is_none(), "poll_next should return None when approval is acknowledged");

        // Take the acknowledged approval (user makes decision)
        let active = queue.take_active_approval();
        assert_eq!(active.unwrap().call_id, "call_2");

        // NOW polling should work, and call_0 should be next (first in queue)
        assert!(queue.has_pollable());
        let next = queue.poll_next();
        assert_eq!(next.unwrap().call_id, "call_0");
    }

    /// Test that IDE effects can still be polled while an approval is acknowledged
    #[test]
    fn test_ide_effects_pollable_during_acknowledged_approval() {
        let mut queue = EffectQueue::new();

        queue.push(make_approval_effect("call_1", "shell"));
        queue.push(make_ide_effect("ide_1"));
        queue.push(make_approval_effect("call_2", "shell"));

        // Acknowledge call_1
        if let Some(effect) = queue.find_by_call_id_mut("call_1") {
            effect.acknowledge();
        }

        // IDE effects use a different resource, so should still be pollable
        assert!(queue.has_pollable());
        let polled = queue.poll_next();
        assert_eq!(polled.unwrap().call_id, "ide_1");

        // But approval call_2 should NOT be pollable
        assert!(!queue.has_pollable());
    }
}
