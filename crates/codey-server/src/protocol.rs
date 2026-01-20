//! WebSocket protocol definitions
//!
//! Defines the message types for client-server communication.

use codey::{AgentStep, ToolCall, ToolEventMessage, Usage};
use serde::{Deserialize, Serialize};

// ============================================================================
// Client → Server Messages
// ============================================================================

/// Messages sent from client to server
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// Send a message to the agent
    SendMessage {
        content: String,
        /// Optional: specify agent ID for multi-agent sessions
        #[serde(default)]
        agent_id: Option<u32>,
    },

    /// Approve or deny a pending tool execution
    ToolDecision {
        call_id: String,
        approved: bool,
    },

    /// Cancel current operation (interrupt streaming, cancel tools)
    Cancel,

    /// Request conversation history
    GetHistory,

    /// Request current session state (for reconnection)
    GetState,

    /// Ping to keep connection alive
    Ping,
}

// ============================================================================
// Server → Client Messages
// ============================================================================

/// Messages sent from server to client
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Session established
    Connected {
        session_id: String,
    },

    /// Streaming text from agent
    TextDelta {
        agent_id: u32,
        content: String,
    },

    /// Streaming thinking/reasoning from agent
    ThinkingDelta {
        agent_id: u32,
        content: String,
    },

    /// Agent requesting tool execution
    ToolRequest {
        agent_id: u32,
        calls: Vec<ToolCallInfo>,
    },

    /// Tool awaiting user approval (didn't pass auto-approve filters)
    ToolAwaitingApproval {
        agent_id: u32,
        call_id: String,
        name: String,
        params: serde_json::Value,
        background: bool,
    },

    /// Tool execution started (after approval)
    ToolStarted {
        agent_id: u32,
        call_id: String,
        name: String,
    },

    /// Streaming output from tool execution
    ToolDelta {
        agent_id: u32,
        call_id: String,
        content: String,
    },

    /// Tool execution completed successfully
    ToolCompleted {
        agent_id: u32,
        call_id: String,
        content: String,
    },

    /// Tool execution failed or was denied
    ToolError {
        agent_id: u32,
        call_id: String,
        error: String,
    },

    /// Agent finished processing (turn complete)
    Finished {
        agent_id: u32,
        usage: Usage,
    },

    /// Agent is retrying after transient error
    Retrying {
        agent_id: u32,
        attempt: u32,
        error: String,
    },

    /// Conversation history (response to GetHistory)
    History {
        messages: Vec<HistoryMessage>,
    },

    /// Session state (response to GetState)
    State {
        agents: Vec<AgentInfo>,
        pending_approvals: Vec<PendingApproval>,
    },

    /// Pong response to Ping
    Pong,

    /// Error occurred
    Error {
        message: String,
        /// If true, the session is no longer usable
        fatal: bool,
    },
}

// ============================================================================
// Supporting Types
// ============================================================================

/// Tool call information for protocol
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallInfo {
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub background: bool,
}

impl From<&ToolCall> for ToolCallInfo {
    fn from(tc: &ToolCall) -> Self {
        Self {
            call_id: tc.call_id.clone(),
            name: tc.name.clone(),
            params: tc.params.clone(),
            background: tc.background,
        }
    }
}

/// History message for protocol
#[derive(Debug, Clone, Serialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<String>,
}

/// Agent info for state response
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: u32,
    pub name: Option<String>,
    pub is_streaming: bool,
}

/// Pending approval for state response
#[derive(Debug, Clone, Serialize)]
pub struct PendingApproval {
    pub agent_id: u32,
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
}
