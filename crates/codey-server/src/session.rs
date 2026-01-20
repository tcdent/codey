//! Per-connection session management
//!
//! Each WebSocket connection gets its own Session with an Agent and ToolExecutor.

use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

use codey::{
    Agent, AgentRuntimeConfig, AgentStep, RequestMode,
    ToolCall, ToolDecision, ToolRegistry,
};

use crate::protocol::{ClientMessage, ServerMessage, ToolCallInfo};

/// Per-connection session state
pub struct Session {
    /// Unique session identifier
    id: String,

    /// The primary agent
    agent: Agent,

    /// Tool registry (for now we use an empty one - tools execute server-side in full impl)
    #[allow(dead_code)]
    tools: ToolRegistry,

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
        config: AgentRuntimeConfig,
        system_prompt: &str,
        ws_tx: mpsc::UnboundedSender<ServerMessage>,
        ws_rx: mpsc::UnboundedReceiver<ClientMessage>,
    ) -> Self {
        // For the initial implementation, we use an empty tool registry
        // and handle tool execution via AgentStep::ToolRequest
        let tools = ToolRegistry::empty();
        let agent = Agent::new(config, system_prompt, None, tools.clone());

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            agent,
            tools,
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
                // Send tool request notification
                let call_infos: Vec<ToolCallInfo> = calls.iter().map(ToolCallInfo::from).collect();

                self.send(ServerMessage::ToolRequest {
                    agent_id,
                    calls: call_infos.clone(),
                })?;

                // For this initial implementation, we send tools as awaiting approval
                // and let the client decide. In the full implementation with ToolExecutor,
                // we'd check auto-approve filters first.
                for call in &calls {
                    self.send(ServerMessage::ToolAwaitingApproval {
                        agent_id,
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        params: call.params.clone(),
                        background: call.background,
                    })?;
                }

                // TODO: In full implementation, enqueue to ToolExecutor
                // For now, client must handle tool execution and send results
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
}
