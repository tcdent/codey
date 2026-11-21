//! LLM client and agent loop

mod agent;
mod backend;
mod registry;
mod rockem_agent;
mod sdk_agent;

#[allow(unused_imports)]
pub use agent::{Activity, AgentStep, Phase, RequestMode, SystemPromptBuilder, Usage};
#[allow(unused_imports)]
pub use backend::Agent;
#[allow(unused_imports)]
pub use registry::{AgentId, AgentMetadata, AgentRegistry, AgentStatus, PRIMARY_AGENT_ID};
