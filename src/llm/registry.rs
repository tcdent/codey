//! Agent registry for managing multiple agents

use std::collections::HashMap;
use std::time::Instant;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Mutex;

use super::agent::{Agent, AgentStep};

/// Unique identifier for an agent
pub type AgentId = u32;

/// Primary agent is always ID 0
pub const PRIMARY_AGENT_ID: AgentId = 0;

/// Status of a spawned agent
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    Finished,
    Error(String),
}

/// Metadata for spawned agents
pub struct AgentMetadata {
    /// Short label for UI display (e.g., "research codebase")
    pub label: String,
    /// Parent agent that spawned this one
    pub parent_id: AgentId,
    /// When the agent was spawned
    pub created_at: Instant,
    /// Current status
    pub status: AgentStatus,
}

/// Registry for managing multiple agents
pub struct AgentRegistry {
    agents: HashMap<AgentId, Mutex<Agent>>,
    metadata: HashMap<AgentId, AgentMetadata>,
    next_id: AgentId,
    /// The primary agent for user interaction
    primary: Option<AgentId>,
}

impl AgentRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            metadata: HashMap::new(),
            next_id: 1, // Sub-agents start at 1
            primary: None,
        }
    }

    /// Register the primary agent (always ID 0)
    pub fn register(&mut self, agent: Agent) -> AgentId {
        self.agents.insert(PRIMARY_AGENT_ID, Mutex::new(agent));
        self.primary = Some(PRIMARY_AGENT_ID);
        PRIMARY_AGENT_ID
    }

    /// Register a spawned sub-agent
    pub fn register_spawned(
        &mut self,
        agent: Agent,
        label: String,
        parent_id: AgentId,
    ) -> AgentId {
        let id = self.next_id;
        self.next_id += 1;

        self.agents.insert(id, Mutex::new(agent));
        self.metadata.insert(
            id,
            AgentMetadata {
                label,
                parent_id,
                created_at: Instant::now(),
                status: AgentStatus::Running,
            },
        );

        id
    }

    /// Remove an agent by ID (used for cleanup after get_agent retrieves result)
    pub fn remove(&mut self, id: AgentId) -> Option<Agent> {
        let agent = self.agents.remove(&id)?;
        self.metadata.remove(&id);

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

    /// Get metadata for a spawned agent
    pub fn metadata(&self, id: AgentId) -> Option<&AgentMetadata> {
        self.metadata.get(&id)
    }

    /// Get mutable metadata for a spawned agent
    pub fn metadata_mut(&mut self, id: AgentId) -> Option<&mut AgentMetadata> {
        self.metadata.get_mut(&id)
    }

    /// Mark agent as finished
    pub fn finish(&mut self, id: AgentId) {
        if let Some(meta) = self.metadata.get_mut(&id) {
            meta.status = AgentStatus::Finished;
        }
    }

    /// Mark agent as errored
    #[allow(dead_code)]
    pub fn set_error(&mut self, id: AgentId, error: String) {
        if let Some(meta) = self.metadata.get_mut(&id) {
            meta.status = AgentStatus::Error(error);
        }
    }

    /// List all spawned agents with their status
    pub fn list_spawned(&self) -> Vec<(AgentId, &AgentMetadata)> {
        self.metadata.iter().map(|(&id, meta)| (id, meta)).collect()
    }

    /// Find a spawned agent by label
    pub fn find_by_label(&self, label: &str) -> Option<AgentId> {
        self.metadata
            .iter()
            .find(|(_, meta)| meta.label == label)
            .map(|(&id, _)| id)
    }

    /// Count running background agents
    pub fn running_background_count(&self) -> usize {
        self.metadata
            .values()
            .filter(|meta| meta.status == AgentStatus::Running)
            .count()
    }

    /// Get the primary agent ID
    pub fn primary_id(&self) -> Option<AgentId> {
        self.primary
    }

    /// Get a reference to the primary agent's mutex
    pub fn primary(&self) -> Option<&Mutex<Agent>> {
        self.primary.and_then(|id| self.agents.get(&id))
    }

    /// Poll all agents for the next step
    /// Returns the first agent that has something to report
    ///
    /// This method is cancel-safe: agents maintain their own state machines
    pub async fn next(&self) -> Option<(AgentId, AgentStep)> {
        if self.agents.is_empty() {
            return None;
        }

        let futures: FuturesUnordered<_> = self
            .agents
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
