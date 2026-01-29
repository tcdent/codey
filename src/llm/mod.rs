//! LLM client and agent loop

mod agent;
mod client;
mod registry;

#[allow(unused_imports)]
pub use agent::{Agent, AgentStep, RequestMode, SystemPromptBuilder, Usage};
#[allow(unused_imports)]
pub use client::{build_client, is_openrouter_model, OPENROUTER_PREFIX};
#[allow(unused_imports)]
pub use registry::{AgentId, AgentMetadata, AgentRegistry, AgentStatus, PRIMARY_AGENT_ID};
