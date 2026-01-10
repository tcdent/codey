//! LLM client and agent loop

mod agent;
pub mod background;
mod registry;

pub use agent::{Agent, AgentStep, RequestMode};
pub use registry::{AgentId, AgentRegistry};
