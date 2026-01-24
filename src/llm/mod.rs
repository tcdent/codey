//! LLM client and agent loop

mod agent;
mod registry;

pub use agent::{Agent, AgentStep, RequestMode, SystemPromptBuilder, Usage};
pub use registry::{AgentId, AgentMetadata, AgentRegistry, AgentStatus, PRIMARY_AGENT_ID};
