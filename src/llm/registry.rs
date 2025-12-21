//! Agent registry for managing multiple agents

use std::collections::HashMap;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Mutex;

use super::agent::{Agent, AgentStep};

/// Unique identifier for an agent
pub type AgentId = u32;

/// Registry for managing multiple agents
pub struct AgentRegistry {
    agents: HashMap<AgentId, Mutex<Agent>>,
    next_id: AgentId,
    /// The primary agent for user interaction
    primary: Option<AgentId>,
}

impl AgentRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            next_id: 0,
            primary: None,
        }
    }

    /// Register a new agent, returns its ID
    /// If this is the first agent, it becomes the primary
    pub fn register(&mut self, agent: Agent) -> AgentId {
        let id = self.next_id;
        self.next_id += 1;
        self.agents.insert(id, Mutex::new(agent));

        // First agent becomes primary by default
        if self.primary.is_none() {
            self.primary = Some(id);
        }

        id
    }

    /// Remove an agent by ID
    pub fn remove(&mut self, id: AgentId) -> Option<Agent> {
        let agent = self.agents.remove(&id)?;

        // Clear primary if we removed it
        if self.primary == Some(id) {
            self.primary = None;
        }

        // into_inner consumes the mutex, returning the inner value
        Some(agent.into_inner())
    }

    /// Get a reference to an agent's mutex
    pub fn get(&self, id: AgentId) -> Option<&Mutex<Agent>> {
        self.agents.get(&id)
    }

    /// Get the primary agent ID
    pub fn primary_id(&self) -> Option<AgentId> {
        self.primary
    }

    /// Get a reference to the primary agent's mutex
    pub fn primary(&self) -> Option<&Mutex<Agent>> {
        self.primary.and_then(|id| self.agents.get(&id))
    }

    /// Set the primary agent
    pub fn set_primary(&mut self, id: AgentId) -> bool {
        if self.agents.contains_key(&id) {
            self.primary = Some(id);
            true
        } else {
            false
        }
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Get the number of agents
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Poll all agents for the next step
    /// Returns the first agent that has something to report
    ///
    /// This method is cancel-safe: agents maintain their own state machines
    pub async fn next(&self) -> Option<(AgentId, AgentStep)> {
        if self.agents.is_empty() {
            return None;
        }

        let futures: FuturesUnordered<_> = self.agents
            .iter()
            .map(|(&id, agent_mutex)| async move {
                let mut agent = agent_mutex.lock().await;
                (id, agent.next().await)
            })
            .collect();

        // Poll until we find an agent with Some(step), or exhaust all
        let mut futures = futures;
        while let Some((agent_id, maybe_step)) = futures.next().await {
            if let Some(step) = maybe_step {
                return Some((agent_id, step));
            }
            // Agent returned None, continue to next completed future
        }

        None
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
