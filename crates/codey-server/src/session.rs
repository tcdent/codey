//! Per-connection session management
//!
//! Each WebSocket connection gets its own Session with an Agent and ToolExecutor.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

// These imports require codey's "cli" feature for server-side tool execution
use codey::{
    Agent, AgentRuntimeConfig, AgentStep, RequestMode,
    ToolDecision, ToolEvent, ToolExecutor, ToolFilters, ToolRegistry,
};

use crate::protocol::{ClientMessage, ServerMessage, ToolCallInfo};

/// Per-connection session state
pub struct Session {
    /// Unique session identifier
    id: String,

    /// The primary agent
    agent: Agent,

    /// Tool executor for server-side tool execution
    tool_executor: ToolExecutor,

    /// Tool filters for auto-approve/deny (shared across sessions)
    filters: Arc<ToolFilters>,

    /// Pending approvals: call_id -> responder channel
    pending_approvals: HashMap<String, oneshot::Sender<ToolDecision>>,

    /// Channel to send messages to WebSocket writer task
    ws_tx: mpsc::UnboundedSender<ServerMessage>,

    /// Channel to receive messages from WebSocket reader task
    ws_rx: mpsc::UnboundedReceiver<ClientMessage>,
}

impl Session {
    /// Create a new session with the given WebSocket channels
    pub fn new(
        agent_config: AgentRuntimeConfig,
        system_prompt: &str,
        filters: Arc<ToolFilters>,
        ws_tx: mpsc::UnboundedSender<ServerMessage>,
        ws_rx: mpsc::UnboundedReceiver<ClientMessage>,
    ) -> Self {
        // Create tool registry with all available tools
        let tools = ToolRegistry::new();
        let tool_executor = ToolExecutor::new(tools.clone());
        let agent = Agent::new(agent_config, system_prompt, None, tools);

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            agent,
            tool_executor,
            filters,
            pending_approvals: HashMap::new(),
            ws_tx,
            ws_rx,
        }
    }

    /// Get the session ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Main event loop - mirrors app.rs structure
    pub async fn run(&mut self) -> Result<()> {
        // Send connected message
        self.send(ServerMessage::Connected {
            session_id: self.id.clone(),
        })?;

        loop {
            tokio::select! {
                // Priority 1: WebSocket messages from client
                msg = self.ws_rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if self.handle_client_message(msg).await? {
                                break; // Client requested disconnect
                            }
                        }
                        None => {
                            // Channel closed - client disconnected
                            tracing::info!("Session {}: client disconnected", self.id);
                            break;
                        }
                    }
                }

                // Priority 2: Agent steps (streaming, tool requests)
                step = self.agent.next() => {
                    if let Some(step) = step {
                        self.handle_agent_step(step).await?;
                    }
                }

                // Priority 3: Tool executor events
                event = self.tool_executor.next() => {
                    if let Some(event) = event {
                        self.handle_tool_event(event).await?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Send a message to the WebSocket
    fn send(&self, msg: ServerMessage) -> Result<()> {
        self.ws_tx
            .send(msg)
            .map_err(|_| anyhow::anyhow!("WebSocket channel closed"))
    }

    /// Handle a message from the client
    /// Returns true if the session should end
    async fn handle_client_message(&mut self, msg: ClientMessage) -> Result<bool> {
        match msg {
            ClientMessage::SendMessage { content, .. } => {
                tracing::debug!("Session {}: received message: {}", self.id, content);
                self.agent.send_request(&content, RequestMode::Normal);
            }

            ClientMessage::ToolDecision { call_id, approved } => {
                tracing::debug!(
                    "Session {}: tool decision for {}: {}",
                    self.id,
                    call_id,
                    if approved { "approved" } else { "denied" }
                );

                if let Some(responder) = self.pending_approvals.remove(&call_id) {
                    let decision = if approved {
                        ToolDecision::Approve
                    } else {
                        ToolDecision::Deny
                    };
                    let _ = responder.send(decision);
                }
            }

            ClientMessage::Cancel => {
                tracing::debug!("Session {}: cancel requested", self.id);
                self.agent.cancel();
                self.tool_executor.cancel();
            }

            ClientMessage::GetHistory => {
                // TODO: implement history retrieval from transcript
                self.send(ServerMessage::History { messages: vec![] })?;
            }

            ClientMessage::GetState => {
                // TODO: implement full state retrieval
                let pending: Vec<_> = self.pending_approvals
                    .keys()
                    .map(|call_id| crate::protocol::PendingApproval {
                        agent_id: 0,
                        call_id: call_id.clone(),
                        name: String::new(),
                        params: serde_json::Value::Null,
                    })
                    .collect();

                self.send(ServerMessage::State {
                    agents: vec![crate::protocol::AgentInfo {
                        id: 0,
                        name: None,
                        is_streaming: false, // TODO: track this
                    }],
                    pending_approvals: pending,
                })?;
            }

            ClientMessage::Ping => {
                self.send(ServerMessage::Pong)?;
            }
        }

        Ok(false)
    }

    /// Handle an agent step (streaming output, tool requests, etc.)
    async fn handle_agent_step(&mut self, step: AgentStep) -> Result<()> {
        let agent_id = 0; // Primary agent

        match step {
            AgentStep::TextDelta(content) => {
                self.send(ServerMessage::TextDelta { agent_id, content })?;
            }

            AgentStep::ThinkingDelta(content) => {
                self.send(ServerMessage::ThinkingDelta { agent_id, content })?;
            }

            AgentStep::CompactionDelta(_) => {
                // Compaction is internal, don't expose to clients for now
            }

            AgentStep::ToolRequest(calls) => {
                // Send tool request notification to client (informational)
                let call_infos: Vec<ToolCallInfo> = calls.iter().map(ToolCallInfo::from).collect();
                self.send(ServerMessage::ToolRequest {
                    agent_id,
                    calls: call_infos,
                })?;

                // Enqueue tools to executor - it will emit AwaitingApproval events
                self.tool_executor.enqueue(calls);
            }

            AgentStep::Finished { usage } => {
                self.send(ServerMessage::Finished { agent_id, usage })?;
            }

            AgentStep::Retrying { attempt, error } => {
                self.send(ServerMessage::Retrying {
                    agent_id,
                    attempt,
                    error,
                })?;
            }

            AgentStep::Error(message) => {
                self.send(ServerMessage::Error {
                    message,
                    fatal: false,
                })?;
            }
        }

        Ok(())
    }

    /// Handle a tool executor event
    async fn handle_tool_event(&mut self, event: ToolEvent) -> Result<()> {
        match event {
            ToolEvent::AwaitingApproval {
                agent_id,
                call_id,
                name,
                params,
                background,
                responder,
            } => {
                // Check auto-approve/deny filters first
                if let Some(decision) = self.filters.evaluate(&name, &params) {
                    tracing::debug!(
                        "Session {}: auto-{} tool {} ({})",
                        self.id,
                        if decision == ToolDecision::Approve { "approve" } else { "deny" },
                        name,
                        call_id
                    );

                    // Send notification that tool is starting (if approved)
                    if decision == ToolDecision::Approve {
                        self.send(ServerMessage::ToolStarted {
                            agent_id,
                            call_id: call_id.clone(),
                            name: name.clone(),
                        })?;
                    }

                    let _ = responder.send(decision);
                } else {
                    // No filter match - bubble to WebSocket for client decision
                    tracing::debug!(
                        "Session {}: awaiting approval for tool {} ({})",
                        self.id,
                        name,
                        call_id
                    );

                    self.pending_approvals.insert(call_id.clone(), responder);
                    self.send(ServerMessage::ToolAwaitingApproval {
                        agent_id,
                        call_id,
                        name,
                        params,
                        background,
                    })?;
                }
            }

            ToolEvent::Delta { agent_id, call_id, content } => {
                self.send(ServerMessage::ToolDelta {
                    agent_id,
                    call_id,
                    content,
                })?;
            }

            ToolEvent::Completed { agent_id, call_id, content } => {
                // Submit result back to agent
                self.agent.submit_tool_result(&call_id, content.clone());

                self.send(ServerMessage::ToolCompleted {
                    agent_id,
                    call_id,
                    content,
                })?;
            }

            ToolEvent::Error { agent_id, call_id, content } => {
                // Submit error back to agent
                self.agent.submit_tool_result(&call_id, format!("Error: {}", content));

                self.send(ServerMessage::ToolError {
                    agent_id,
                    call_id,
                    error: content,
                })?;
            }

            ToolEvent::Delegate { responder, .. } => {
                // For now, reject delegated effects (IDE integration, sub-agents)
                // These would need special handling over WebSocket
                tracing::warn!("Session {}: delegation not supported, rejecting", self.id);
                let _ = responder.send(Err("Delegation not supported over WebSocket".to_string()));
            }

            ToolEvent::BackgroundStarted { agent_id, call_id, name } => {
                self.send(ServerMessage::ToolStarted {
                    agent_id,
                    call_id,
                    name,
                })?;
            }

            ToolEvent::BackgroundCompleted { agent_id, call_id, .. } => {
                // Retrieve result and submit to agent
                if let Some((_name, output, _status)) = self.tool_executor.take_result(&call_id) {
                    self.agent.submit_tool_result(&call_id, output.clone());
                    self.send(ServerMessage::ToolCompleted {
                        agent_id,
                        call_id,
                        content: output,
                    })?;
                }
            }
        }

        Ok(())
    }
}
