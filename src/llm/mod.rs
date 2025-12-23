//! LLM client and agent loop

mod agent;
mod registry;

pub use agent::{Agent, AgentStep, RequestMode};
pub use registry::{AgentId, AgentRegistry};
