// Background agent runner for sub-agent tasks
//
// Runs an agent to completion without UI interaction, collecting output.

use anyhow::Result;

use super::agent::{Agent, AgentStep};
use crate::tools::{ToolRegistry, Step};

/// Run a background agent to completion, returning collected text output.
///
/// This is a simpler execution model than the main event loop:
/// - No transcript interaction
/// - Auto-approves all tools
/// - Collects text output to a string
/// - Returns when agent finishes or errors
pub async fn run_agent(mut agent: Agent, tools: ToolRegistry) -> Result<String> {
    let mut output = String::new();

    while let Some(step) = agent.next().await {
        match step {
            AgentStep::TextDelta(text) => {
                output.push_str(&text);
            },
            AgentStep::ToolRequest(calls) => {
                for call in calls {
                    tracing::debug!(
                        "Background agent tool call: {} params={:?}",
                        call.name,
                        call.params
                    );
                    let result = execute_tool(&tools, &call.name, call.params).await;
                    tracing::debug!("Background agent tool result: {}", &result);
                    agent.submit_tool_result(&call.call_id, result);
                }
            },
            AgentStep::Finished { usage } => {
                tracing::info!(
                    "Background agent finished: {} output, {} context tokens",
                    usage.output_tokens,
                    usage.context_tokens
                );
                break;
            },
            AgentStep::Error(msg) => {
                tracing::error!("Background agent error: {}", msg);
                return Err(anyhow::anyhow!("Background agent error: {}", msg));
            },
            AgentStep::Retrying { attempt, error } => {
                tracing::warn!("Background agent retrying (attempt {}): {}", attempt, error);
            },
            // Ignore thinking/compaction deltas - they're internal
            AgentStep::ThinkingDelta(_) | AgentStep::CompactionDelta(_) => {},
        }
    }

    Ok(output)
}

/// Execute a single tool, returning its output.
///
/// This bypasses the normal ToolExecutor approval flow:
/// - Auto-approves (skips AwaitApproval steps)
/// - Skips delegate effects (IDE integration not available in background)
/// - Collects Output/Delta to string
async fn execute_tool(tools: &ToolRegistry, name: &str, params: serde_json::Value) -> String {
    let tool = tools.get(name);
    let mut pipeline = tool.compose(params);
    let mut output = String::new();

    while let Some(handler) = pipeline.pop() {
        match handler.call().await {
            Step::Continue => {},
            Step::Output(content) => {
                output = content;
            },
            Step::Delta(content) => {
                output.push_str(&content);
            },
            Step::AwaitApproval => {
                // Auto-approve by continuing
                tracing::debug!("Background tool auto-approved");
            },
            Step::Delegate(effect) => {
                // Skip delegate effects - no IDE/UI in background
                tracing::debug!("Background tool skipping delegate: {:?}", effect);
            },
            Step::Error(msg) => {
                tracing::warn!("Background tool error: {}", msg);
                return format!("Error: {}", msg);
            },
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_tool_simple() {
        // Test with a simple shell command
        let tools = ToolRegistry::read_only();
        let result = execute_tool(
            &tools,
            "mcp_shell",
            serde_json::json!({"command": "echo hello"}),
        )
        .await;

        assert!(result.contains("hello"));
    }
}
