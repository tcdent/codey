//! Streamable tool executor

use crate::llm::AgentId;
use crate::tools::ToolOutput;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::collections::VecDeque;
use std::path::PathBuf;

/// A tool call pending execution
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub agent_id: AgentId,
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub decision: ToolDecision,
}

impl ToolCall {
    /// Set the agent_id for this tool call
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = agent_id;
        self
    }
}

/// Decision state for a pending tool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolDecision {
    /// Initial state, not yet requested
    #[default]
    Pending,
    /// Approval has been requested from the user
    Requested,
    /// User approved execution
    Approve,
    /// User denied execution
    Deny,
}

/// Effects that tools can request
/// These are processed by App after tool completion
#[derive(Debug, Clone)]
pub enum ToolEffect {
    /// Spawn a background agent with a task
    SpawnAgent {
        task: String,
        context: Option<String>,
    },
    /// Reload a buffer in the IDE (after file modification)
    IdeReloadBuffer {
        path: PathBuf,
    },
    /// Open/navigate to a file in the IDE
    IdeOpen {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// Show a notification to the user
    #[allow(dead_code)]
    Notify {
        message: String,
    },
}

/// Events emitted by the tool executor
#[derive(Debug)]
pub enum ToolEvent {
    /// A tool needs user approval
    AwaitingApproval(ToolCall),
    /// Streaming output from tool execution
    OutputDelta {
        agent_id: AgentId,
        call_id: String,
        delta: String,
    },
    /// Tool execution completed
    Completed {
        agent_id: AgentId,
        call_id: String,
        content: String,
        is_error: bool,
        effects: Vec<ToolEffect>,
    },
}

/// Active tool execution state
struct ActiveExecution {
    agent_id: AgentId,
    call_id: String,
    stream: BoxStream<'static, ToolOutput>,
    collected_output: String,
}

/// Streamable tool executor
pub struct ToolExecutor {
    tools: crate::tools::ToolRegistry,
    pending: VecDeque<ToolCall>,
    active: Option<ActiveExecution>,
}

impl ToolExecutor {
    pub fn new(tools: crate::tools::ToolRegistry) -> Self {
        Self {
            tools,
            pending: VecDeque::new(),
            active: None,
        }
    }

    /// Get a reference to the tool registry
    pub fn tools(&self) -> &crate::tools::ToolRegistry {
        &self.tools
    }

    /// Add tool calls to the pending queue
    pub fn enqueue(&mut self, tool_calls: Vec<ToolCall>) {
        self.pending.extend(tool_calls);
    }

    /// Get front pending tool
    pub fn front(&self) -> Option<&ToolCall> {
        self.pending.front()
    }

    /// Mark a pending tool with a decision
    pub fn decide(&mut self, call_id: &str, decision: ToolDecision) {
        tracing::debug!("ToolExecutor decision for {}: {:?}", call_id, decision);
        if let Some(tool) = self.pending.iter_mut().find(|t| t.call_id == call_id) {
            tool.decision = decision;
        }
    }

    /// Poll for the next event
    pub async fn next(&mut self) -> Option<ToolEvent> {
        loop {
            // If executing, poll the stream
            if let Some(active) = &mut self.active {
                match active.stream.next().await {
                    Some(ToolOutput::Delta(delta)) => {
                        active.collected_output.push_str(&delta);
                        return Some(ToolEvent::OutputDelta {
                            agent_id: active.agent_id,
                            call_id: active.call_id.clone(),
                            delta,
                        });
                    }
                    Some(ToolOutput::Done(result)) => {
                        let active = self.active.take().unwrap();
                        return Some(ToolEvent::Completed {
                            agent_id: active.agent_id,
                            call_id: active.call_id,
                            content: result.content,
                            is_error: result.is_error,
                            effects: result.effects,
                        });
                    }
                    None => {
                        // Stream ended without Done - use collected output
                        // TODO this should be unreachable
                        let active = self.active.take().unwrap();
                        return Some(ToolEvent::Completed {
                            agent_id: active.agent_id,
                            call_id: active.call_id,
                            content: active.collected_output,
                            is_error: false,
                            effects: vec![],
                        });
                    }
                }
            }

            // Check front of pending queue
            let tool_call = self.pending.front_mut()?;
            match tool_call.decision {
                ToolDecision::Pending => {
                    // Send approval request
                    tracing::debug!("ToolExecutor requesting approval for {}", tool_call.call_id);
                    tool_call.decision = ToolDecision::Requested;
                    return Some(ToolEvent::AwaitingApproval(tool_call.clone()));
                }
                ToolDecision::Requested => {
                    // Already sent approval request, waiting for decide()
                    return None;
                }
                ToolDecision::Deny => {
                    let tool_call = self.pending.pop_front().unwrap();
                    return Some(ToolEvent::Completed {
                        agent_id: tool_call.agent_id,
                        call_id: tool_call.call_id,
                        content: "Denied by user".to_string(),
                        is_error: true,
                        effects: vec![],
                    });
                }
                ToolDecision::Approve => {
                    tracing::debug!("ToolExecutor executing approved tool {}", tool_call.call_id);
                    let tool_call = self.pending.pop_front().unwrap();
                    let tool = self.tools.get_arc(&tool_call.name);
                    let stream = tool.execute(tool_call.params.clone());

                    self.active = Some(ActiveExecution {
                        agent_id: tool_call.agent_id,
                        call_id: tool_call.call_id,
                        stream,
                        collected_output: String::new(),
                    });
                    // Loop back to poll the new stream
                    continue;
                }
            }
        }
    }
}
