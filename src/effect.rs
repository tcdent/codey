//! Effects and effect queue management.
//!
//! Effects are actions that tools delegate to the app layer: spawning sub-agents
//! and listing them. The `EffectQueue` (CLI-only) holds them until the app polls
//! them, first in, first out.
#![allow(dead_code)]

#[cfg(feature = "cli")]
use std::collections::VecDeque;

#[cfg(feature = "cli")]
use tokio::sync::oneshot;

#[cfg(feature = "cli")]
use crate::llm::Agent;
#[cfg(feature = "cli")]
use crate::llm::AgentId;

/// Result type for effect execution
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub type EffectResult = Result<Option<String>, String>;

/// Effects that tools delegate to the app layer
pub enum Effect {
    /// Spawn a sub-agent. App registers it and polls through main loop.
    #[cfg(feature = "cli")]
    SpawnAgent { agent: Agent, label: String },
    /// List all spawned agents
    #[cfg(feature = "cli")]
    ListAgents,
}

impl std::fmt::Debug for Effect {
    // Without the CLI there are no effects, and nothing to format.
    #[cfg_attr(not(feature = "cli"), allow(unused_variables))]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            #[cfg(feature = "cli")]
            Effect::SpawnAgent { ref label, .. } => {
                f.debug_struct("SpawnAgent").field("label", label).finish_non_exhaustive()
            },
            #[cfg(feature = "cli")]
            Effect::ListAgents => f.write_str("ListAgents"),
        }
    }
}

// ============================================================================
// Effect Queue Management (CLI-only)
// ============================================================================

/// An effect waiting to be executed or polled
#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub struct PendingEffect {
    pub call_id: String,
    pub agent_id: AgentId,
    pub effect: Effect,
    pub responder: oneshot::Sender<EffectResult>,
}

#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
impl PendingEffect {
    pub fn new(
        call_id: String,
        agent_id: AgentId,
        effect: Effect,
        responder: oneshot::Sender<EffectResult>,
    ) -> Self {
        Self { call_id, agent_id, effect, responder }
    }

    /// Send a result back to the executor and consume this effect
    pub fn complete(self, result: EffectResult) {
        let _ = self.responder.send(result);
    }
}

/// Pending effects, polled in the order they arrived.
#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
pub struct EffectQueue {
    pending: VecDeque<PendingEffect>,
}

#[cfg(feature = "cli")]
#[allow(dead_code)] // Used by binary crate (app.rs), not library
impl EffectQueue {
    pub fn new() -> Self {
        Self { pending: VecDeque::new() }
    }

    /// Add a new effect to the queue
    pub fn push(&mut self, effect: PendingEffect) {
        self.pending.push_back(effect);
    }

    /// Re-queue an effect that wasn't ready (puts at back of queue)
    pub fn requeue(&mut self, effect: PendingEffect) {
        self.pending.push_back(effect);
    }

    /// Clear all pending effects (used on hard cancel to prevent ghost entries)
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Whether any effect is waiting to be polled
    pub fn has_pollable(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The next effect to poll, removed from the queue
    pub fn poll_next(&mut self) -> Option<PendingEffect> {
        self.pending.pop_front()
    }
}

#[cfg(feature = "cli")]
impl Default for EffectQueue {
    fn default() -> Self {
        Self::new()
    }
}
