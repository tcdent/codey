//! Spawn agent tool for creating sub-agents

use std::sync::OnceLock;

use super::{Tool, ToolPipeline};
use crate::app::SUB_AGENT_PROMPT;
use crate::tools::ToolRegistry;
use crate::auth::OAuthCredentials;
use crate::config::AgentRuntimeConfig;
use crate::impl_base_block;
use crate::llm::{Agent, RequestMode};
use crate::llm::background::run_agent;
use crate::tools::pipeline::{EffectHandler, Step};
use crate::transcript::{render_approval_prompt, render_prefix, Block, BlockType, ToolBlock, Status};
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
        "Spawn a background agent to handle a subtask. The sub-agent has read-only tool access \
         (read_file, shell, fetch_url, web_search) and will return its findings. \
         Use this for research, exploration, or analysis tasks that don't require file modifications."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Clear description of what the sub-agent should accomplish"
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

        // Create handler - delegates to app for actual execution (Agent is not Send)
        ToolPipeline::new()
            .await_approval()
            .then(RunAgent {
                task: parsed.task,
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
// RunAgent handler - actually runs the sub-agent
// =============================================================================

/// Handler that runs a sub-agent to completion
struct RunAgent {
    task: String,
    task_context: Option<String>,
}

#[async_trait::async_trait]
impl EffectHandler for RunAgent {
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

        // Create the sub-agent with read-only tools
        let tools = ToolRegistry::subagent();
        let mut agent = Agent::new(
            ctx.runtime_config.clone(),
            &system_prompt,
            oauth,
            tools.clone(),
        );
        agent.send_request(&self.task, RequestMode::Normal);

        // Run to completion
        match run_agent(agent, tools).await {
            Ok(output) => Step::Output(output),
            Err(e) => Step::Error(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::lines_to_string;
    use serde_json::json;

    // =========================================================================
    // Render tests
    // =========================================================================

    #[test]
    fn test_render_pending() {
        let block = SpawnAgentBlock::new(
            "call_1",
            "mcp_spawn_agent",
            json!({"task": "Find all TODO comments"}),
            false,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? spawn_agent(Find all TODO comments)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_pending_long_task() {
        let block = SpawnAgentBlock::new(
            "call_1",
            "mcp_spawn_agent",
            json!({"task": "This is a very long task description that should be truncated after sixty characters"}),
            false,
        );
        let output = lines_to_string(&block.render(80));
        // Truncation happens at 60 chars (57 chars + "...")
        assert_eq!(output, "? spawn_agent(This is a very long task description that should be trunc...)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_running() {
        let mut block = SpawnAgentBlock::new(
            "call_1",
            "mcp_spawn_agent",
            json!({"task": "Find all TODO comments"}),
            false,
        );
        block.status = Status::Running;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⚙ spawn_agent(Find all TODO comments)");
    }

    #[test]
    fn test_render_complete_with_output() {
        let mut block = SpawnAgentBlock::new(
            "call_1",
            "mcp_spawn_agent",
            json!({"task": "Find all TODO comments"}),
            false,
        );
        block.status = Status::Complete;
        block.text = "Found 5 TODO comments".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✓ spawn_agent(Find all TODO comments)\n  Found 5 TODO comments");
    }

    #[test]
    fn test_render_denied() {
        let mut block = SpawnAgentBlock::new(
            "call_1",
            "mcp_spawn_agent",
            json!({"task": "Find all TODO comments"}),
            false,
        );
        block.status = Status::Denied;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⊘ spawn_agent(Find all TODO comments)\n  Denied by user");
    }

    #[test]
    fn test_render_background() {
        let block = SpawnAgentBlock::new(
            "call_1",
            "mcp_spawn_agent",
            json!({"task": "Find all TODO comments"}),
            true,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? [bg] spawn_agent(Find all TODO comments)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_error() {
        let mut block = SpawnAgentBlock::new(
            "call_1",
            "mcp_spawn_agent",
            json!({"task": "Find all TODO comments"}),
            false,
        );
        block.status = Status::Error;
        block.text = "Agent context not initialized".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✗ spawn_agent(Find all TODO comments)\n  Agent context not initialized");
    }
}
