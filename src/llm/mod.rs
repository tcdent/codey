//! LLM client and agent loop

mod agent;
mod client;
mod registry;

pub use agent::{Agent, AgentStep, RequestMode, SystemPromptBuilder, Usage};
pub use client::{build_client, is_openrouter_model, OPENROUTER_PREFIX};
pub use registry::{AgentId, AgentMetadata, AgentRegistry, AgentStatus, PRIMARY_AGENT_ID};
