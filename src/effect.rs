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
//! - Effects are polled until ready
#![allow(dead_code)]

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
pub enum Resource {
    /// Approval UI slot - only one approval shown at a time
    ApprovalSlot,
    /// IDE preview slot - only one preview shown at a time
    IdePreview,
}

/// Result of polling an effect
#[cfg(feature = "cli")]
pub enum EffectPoll {
    /// Effect completed with this result
    Ready(anyhow::Result<Option<String>>),
    /// Effect is waiting, poll again later
    Pending,
}

/// An effect waiting to be executed or polled
#[cfg(feature = "cli")]
pub struct PendingEffect {
    pub call_id: String,
    pub agent_id: AgentId,
    pub effect: Effect,
    pub responder: oneshot::Sender<EffectResult>,
    /// For approval effects, whether UI has been shown to user
    pub acknowledged: bool,
}

#[cfg(feature = "cli")]
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
pub struct EffectQueue {
    pending: VecDeque<PendingEffect>,
}

#[cfg(feature = "cli")]
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
    /// - No prior effect in the queue holds its required resource
    pub fn has_pollable(&self) -> bool {
        let mut claimed: HashSet<Resource> = HashSet::new();

        self.pending.iter().any(|p| {
            // Acknowledged approvals claim the slot but aren't pollable
            if p.is_approval() && p.acknowledged {
                claimed.insert(Resource::ApprovalSlot);
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

        let idx = self.pending.iter().position(|p| {
            // Acknowledged approvals claim the slot but aren't pollable
            if p.is_approval() && p.acknowledged {
                claimed.insert(Resource::ApprovalSlot);
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
