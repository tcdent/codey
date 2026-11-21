//! `Agent`: one surface over the backends, picked by `AgentRuntimeConfig::backend`.
//!
//! - `Claude` drives the `claude` binary, which owns the loop and the conversation.
//! - `Rockem` is Codey's own loop over a rockem service, which holds no state.

use super::agent::{Activity, AgentStep, RequestMode, SystemPromptBuilder, Usage};
use super::rockem_agent::RockemAgent;
use super::sdk_agent::SdkAgent;
use crate::config::{AgentRuntimeConfig, Backend};
use crate::tools::ToolRegistry;

pub enum Agent {
    Claude(SdkAgent),
    Rockem(RockemAgent),
}

macro_rules! each {
    ($self:ident, $a:ident => $e:expr) => {
        match $self {
            Agent::Claude($a) => $e,
            Agent::Rockem($a) => $e,
        }
    };
}

impl Agent {
    pub fn new(config: AgentRuntimeConfig, system_prompt: &str, tools: ToolRegistry) -> Self {
        match config.backend.clone() {
            Backend::Claude => Agent::Claude(SdkAgent::new(config, system_prompt, tools)),
            Backend::Rockem { url } => Agent::Rockem(RockemAgent::new(config, &url, system_prompt, tools)),
        }
    }

    pub fn with_dynamic_prompt(
        config: AgentRuntimeConfig,
        prompt_builder: SystemPromptBuilder,
        tools: ToolRegistry,
    ) -> Self {
        match config.backend.clone() {
            Backend::Claude => Agent::Claude(SdkAgent::with_dynamic_prompt(config, prompt_builder, tools)),
            Backend::Rockem { url } => {
                Agent::Rockem(RockemAgent::with_dynamic_prompt(config, &url, prompt_builder, tools))
            },
        }
    }

    pub fn send_request(&mut self, user_input: &str, mode: RequestMode) {
        each!(self, a => a.send_request(user_input, mode))
    }

    pub async fn next(&mut self) -> Option<AgentStep> {
        each!(self, a => a.next().await)
    }

    pub fn submit_tool_result(&mut self, call_id: &str, content: String) {
        each!(self, a => a.submit_tool_result(call_id, content))
    }

    pub fn cancel(&mut self) {
        each!(self, a => a.cancel())
    }

    pub fn total_usage(&self) -> Usage {
        each!(self, a => a.total_usage())
    }

    /// Rebuild the conversation from a saved transcript (`--continue`).
    pub fn restore_from_transcript(&mut self, transcript: &crate::transcript::Transcript) {
        each!(self, a => a.restore_from_transcript(transcript))
    }

    /// The running turn; `None` between turns.
    pub fn activity(&self) -> Option<Activity> {
        each!(self, a => a.activity())
    }

    pub fn last_message(&self) -> Option<String> {
        each!(self, a => a.last_message())
    }
}
