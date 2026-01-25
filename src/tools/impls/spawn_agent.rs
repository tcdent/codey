//! Spawn agent tool for creating sub-agents

use std::sync::OnceLock;

use super::{Tool, ToolPipeline};
use crate::auth::OAuthCredentials;
use crate::config::AgentRuntimeConfig;
use crate::impl_base_block;
use crate::llm::{Agent, RequestMode};
use crate::prompts::SUB_AGENT_PROMPT;
use crate::tools::pipeline::{Effect, EffectHandler, Step};
use crate::tools::ToolRegistry;
use crate::transcript::{render_approval_prompt, render_prefix, Block, BlockType, Status, ToolBlock};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;

// =============================================================================
// Agent Context - global state for spawning sub-agents
// =============================================================================

// TODO: Refactor to avoid global state. We use OnceLock here because tool handlers
// (EffectHandler::call) don't have access to app-level context like config and OAuth
// credentials. Ideally we'd pass this context through the tool pipeline, but that
// would require changes to the Tool/EffectHandler traits. For now, the app initializes
// this at startup and updates OAuth after refresh.

/// Context needed to spawn sub-agents, initialized at app startup
pub struct AgentContext {
    pub runtime_config: AgentRuntimeConfig,
    /// OAuth credentials - wrapped in RwLock so app can update after refresh
    pub oauth: RwLock<Option<OAuthCredentials>>,
}

static AGENT_CONTEXT: OnceLock<AgentContext> = OnceLock::new();

/// Initialize the agent context. Called once at app startup.
pub fn init_agent_context(runtime_config: AgentRuntimeConfig, oauth: Option<OAuthCredentials>) {
    AGENT_CONTEXT.set(AgentContext {
        runtime_config,
        oauth: RwLock::new(oauth),
    }).ok();
}

/// Update the oauth credentials (called after refresh)
pub async fn update_agent_oauth(oauth: Option<OAuthCredentials>) {
    if let Some(ctx) = AGENT_CONTEXT.get() {
        *ctx.oauth.write().await = oauth;
    }
}

/// Get the agent context, if initialized.
fn agent_context() -> Option<&'static AgentContext> {
    AGENT_CONTEXT.get()
}

/// Spawn agent block - shows the task description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnAgentBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl SpawnAgentBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value, background: bool) -> Option<Self> {
        let _: SpawnAgentParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params, background))
    }
}

#[typetag::serde]
impl Block for SpawnAgentBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let task = self.params["task"].as_str().unwrap_or("");
        // Truncate task for display
        let task_display = if task.len() > 60 {
            format!("{}...", &task[..57])
        } else {
            task.to_string()
        };

        lines.push(Line::from(vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("spawn_agent", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(task_display, Style::default().fg(Color::Yellow)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {}", self.text),
                Style::default().fg(Color::DarkGray),
            )));
        }

        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }

    fn tool_name(&self) -> Option<&str> {
        Some(&self.tool_name)
    }

    fn params(&self) -> Option<&serde_json::Value> {
        Some(&self.params)
    }
}

/// Tool for spawning sub-agents to handle tasks
// NOTE: Currently delegates to app for execution because Agent is not Send-safe.
// When Agent becomes Send, this tool can run the agent directly in its handler.
pub struct SpawnAgentTool;

#[derive(Debug, Deserialize)]
struct SpawnAgentParams {
    /// Description of the task for the sub-agent
    task: String,
    /// Short label for the agent (1-2 hyphenated words, e.g. "code-review")
    label: Option<String>,
    /// Optional context to provide to the sub-agent
    context: Option<String>,
}

impl SpawnAgentTool {
    pub const NAME: &'static str = "mcp_spawn_agent";
}

impl Tool for SpawnAgentTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Spawn a sub-agent to handle a subtask. Returns immediately with an agent_id. \
         The sub-agent has full tool access including edit_file and write_file (approval routed to user). \
         Use list_agents to check status and get_agent to retrieve results when finished."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Clear description of what the sub-agent should accomplish"
                },
                "label": {
                    "type": "string",
                    "description": "Short label for the agent (1-2 hyphenated words, e.g. 'code-review', 'find-bugs')"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context or background information for the sub-agent"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id; use list_background_tasks/get_background_task to check status and retrieve results."
                }
            },
            "required": ["task"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: SpawnAgentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        // Spawn agent via Effect - returns immediately with agent_id
        // Use list_agents/get_agent to check status and retrieve results
        ToolPipeline::new()
            .await_approval()
            .then(SpawnAgentHandler {
                task: parsed.task,
                label: parsed.label,
                task_context: parsed.context,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        if let Some(block) = SpawnAgentBlock::from_params(call_id, self.name(), params.clone(), background) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params, background))
        }
    }
}

// =============================================================================
// SpawnAgentHandler - delegates to App via Effect::SpawnAgent
// =============================================================================

/// Handler that spawns a sub-agent via Effect::SpawnAgent.
/// The agent is registered with the App and polled through the main event loop.
/// Returns immediately with agent_id - use list_agents/get_agent to check status.
struct SpawnAgentHandler {
    task: String,
    label: Option<String>,
    task_context: Option<String>,
}

#[async_trait::async_trait]
impl EffectHandler for SpawnAgentHandler {
    async fn call(self: Box<Self>) -> Step {
        // Get the agent context (initialized at app startup)
        let ctx = match agent_context() {
            Some(c) => c,
            None => return Step::Error("Agent context not initialized".into()),
        };

        // Get OAuth credentials (may be None for API key auth)
        let oauth = ctx.oauth.read().await.clone();

        // Build system prompt
        let system_prompt = if let Some(context) = &self.task_context {
            format!("{}\n\n## Context\n{}", SUB_AGENT_PROMPT, context)
        } else {
            SUB_AGENT_PROMPT.to_string()
        };

        // Create the sub-agent with full tool access
        // Write tools (edit_file, write_file, shell) route approval to user
        let tools = ToolRegistry::subagent();
        let mut agent = Agent::new(
            ctx.runtime_config.clone(),
            &system_prompt,
            oauth,
            tools,
        );
        agent.send_request(&self.task, RequestMode::Normal);

        // Use provided label or fall back to task
        let label = self.label.unwrap_or_else(|| self.task.clone());

        // Delegate to App to register the agent
        // Returns "agent:{id}" - primary agent can use list_agents/get_agent
        Step::Delegate(Effect::SpawnAgent {
            agent,
            label,
        })
    }
}
