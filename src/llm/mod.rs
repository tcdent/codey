//! LLM client and agent loop

mod agent;
pub mod background;
mod registry;

pub use agent::{Agent, AgentStep, RequestMode, SystemPromptBuilder, Usage};
pub use registry::{AgentId, AgentRegistry};
