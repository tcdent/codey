//! SDK-backed agent: drives the `claude` binary via claude-agent-sdk-rs.
//!
//! Replaces the genai/direct-HTTP backend. The `claude` binary owns the agent
//! loop, conversation history, context management, caching, and billing
//! (subscription OAuth or `ANTHROPIC_API_KEY`). Codey owns its tool registry,
//! permission UX, IDE hooks, and transcript — surfaced to the binary through an
//! in-process MCP server (`SdkMcpServer`).
//!
//! Architecture: a driver task owns the `ClaudeClient`, forwards `Message`s as
//! `AgentStep`s over an mpsc channel that `next()` drains, and the in-process
//! MCP bridge answers `tools/call` by emitting an `AgentStep::ToolRequest` and
//! awaiting a oneshot fulfilled by `submit_tool_result`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, warn};

use claude_agent_sdk_rs::types::mcp::McpSdkServerConfig;
use claude_agent_sdk_rs::{
    ClaudeAgentOptions, ClaudeClient, McpServerConfig, McpServers, Message, Result as SdkResult,
    SdkMcpServer, SystemPrompt,
};

use super::agent::{AgentStep, RequestMode, SystemPromptBuilder, Usage};
use crate::config::AgentRuntimeConfig;
use crate::tools::{ToolCall, ToolDecision, ToolRegistry};

/// Name of the in-process MCP server that exposes Codey's tools to the binary.
const CODEY_MCP_SERVER: &str = "codey";

/// Commands sent from the `SdkAgent` to its driver task.
enum DriverCmd {
    /// Send a new user prompt to the binary.
    Query(String),
    /// Interrupt the current turn.
    Interrupt,
}

/// In-process MCP server bridging the binary's `tools/*` calls to Codey's
/// `ToolRegistry` via the `AgentStep::ToolRequest` / `submit_tool_result` flow.
struct CodeyToolBridge {
    /// Tool descriptors for `tools/list`: (name, description, input schema).
    tools: Vec<(String, String, Value)>,
    /// Channel to surface tool requests to `next()`.
    step_tx: mpsc::UnboundedSender<AgentStep>,
    /// Pending tool calls awaiting `submit_tool_result`, keyed by call_id.
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
}

impl CodeyToolBridge {
    fn tools_list_json(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|(name, desc, schema)| {
                json!({ "name": name, "description": desc, "inputSchema": schema })
            })
            .collect()
    }
}

#[async_trait]
impl SdkMcpServer for CodeyToolBridge {
    async fn handle_message(&self, message: Value) -> SdkResult<Value> {
        let method = message["method"].as_str().unwrap_or_default();
        let id = message.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": CODEY_MCP_SERVER, "version": env!("CARGO_PKG_VERSION") }
                }
            })),
            "tools/list" => Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": self.tools_list_json() }
            })),
            "tools/call" => {
                let params = &message["params"];
                let name = params["name"].as_str().unwrap_or_default().to_string();
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);

                // Bridge to Codey's approval/execution flow.
                let call_id = uuid::Uuid::new_v4().to_string();
                let (tx, rx) = oneshot::channel::<String>();
                self.pending.lock().await.insert(call_id.clone(), tx);

                let tool_call = ToolCall {
                    agent_id: 0, // set by App when handling ToolRequest
                    call_id: call_id.clone(),
                    name,
                    params: args,
                    decision: ToolDecision::Pending,
                    background: false,
                };
                if self
                    .step_tx
                    .send(AgentStep::ToolRequest(vec![tool_call]))
                    .is_err()
                {
                    return Err(claude_agent_sdk_rs::ClaudeError::Transport(
                        "agent dropped before tool could be requested".to_string(),
                    ));
                }

                let (text, is_error) = match rx.await {
                    Ok(content) => (content, false),
                    Err(_) => {
                        self.pending.lock().await.remove(&call_id);
                        ("tool execution cancelled".to_string(), true)
                    },
                };

                Ok(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [ { "type": "text", "text": text } ],
                        "isError": is_error
                    }
                }))
            },
            // Notifications require no response.
            "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
            other => Err(claude_agent_sdk_rs::ClaudeError::Transport(format!(
                "unknown MCP method: {other}"
            ))),
        }
    }
}

/// SDK-backed agent. Public surface mirrors the legacy genai `Agent` so callers
/// (App, AgentRegistry) are unchanged.
pub struct SdkAgent {
    config: AgentRuntimeConfig,
    tools: ToolRegistry,
    system_prompt: String,
    system_prompt_builder: Option<SystemPromptBuilder>,
    total_usage: Usage,

    /// Driver task handle (spawned on first `send_request`).
    driver: Option<tokio::task::JoinHandle<()>>,
    /// Commands to the driver (query / interrupt).
    cmd_tx: Option<mpsc::UnboundedSender<DriverCmd>>,
    /// Steps drained by `next()`.
    step_rx: Option<mpsc::UnboundedReceiver<AgentStep>>,
    /// Pending tool calls awaiting results, shared with the MCP bridge.
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,

    /// Accumulated assistant text of the current/last turn (for `last_message`).
    last_text: String,
}

impl SdkAgent {
    pub fn new(
        config: AgentRuntimeConfig,
        system_prompt: &str,
        tools: ToolRegistry,
    ) -> Self {
        Self {
            config,
            tools,
            system_prompt: system_prompt.to_string(),
            system_prompt_builder: None,
            total_usage: Usage::default(),
            driver: None,
            cmd_tx: None,
            step_rx: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            last_text: String::new(),
        }
    }

    pub fn with_dynamic_prompt(
        config: AgentRuntimeConfig,
        prompt_builder: SystemPromptBuilder,
        tools: ToolRegistry,
    ) -> Self {
        let system_prompt = prompt_builder();
        Self {
            config,
            tools,
            system_prompt,
            system_prompt_builder: Some(prompt_builder),
            total_usage: Usage::default(),
            driver: None,
            cmd_tx: None,
            step_rx: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            last_text: String::new(),
        }
    }

    pub fn total_usage(&self) -> Usage {
        self.total_usage
    }

    pub fn last_message(&self) -> Option<String> {
        if self.last_text.is_empty() {
            None
        } else {
            Some(self.last_text.clone())
        }
    }

    pub fn is_fast_mode(&self) -> bool {
        self.config.fast_mode && self.config.model.to_lowercase().contains("opus")
    }

    pub fn model_icon(&self) -> &'static str {
        if self.is_fast_mode() {
            "ϟ"
        } else {
            ""
        }
    }

    /// Tool descriptors for the in-process MCP server, from the registry.
    fn tool_descriptors(&self) -> Vec<(String, String, Value)> {
        self.tools
            .values()
            .map(|t| (t.name().to_string(), t.description().to_string(), t.schema()))
            .collect()
    }

    /// Build the options for spawning the binary.
    fn build_options(&self, bridge: Arc<CodeyToolBridge>) -> ClaudeAgentOptions {
        let mut servers = HashMap::new();
        servers.insert(
            CODEY_MCP_SERVER.to_string(),
            McpServerConfig::Sdk(McpSdkServerConfig {
                name: CODEY_MCP_SERVER.to_string(),
                instance: bridge,
            }),
        );

        // Only Codey's tools are allowed; the binary's built-ins are disabled.
        let allowed_tools: Vec<String> = self
            .tools
            .values()
            .map(|t| format!("mcp__{CODEY_MCP_SERVER}__{}", t.name()))
            .collect();

        let mut options = ClaudeAgentOptions {
            system_prompt: Some(SystemPrompt::Text(self.system_prompt.clone())),
            allowed_tools,
            mcp_servers: McpServers::Dict(servers),
            model: Some(self.config.model.clone()),
            include_partial_messages: true,
            ..Default::default()
        };

        if self.config.thinking_budget > 0 {
            options.max_thinking_tokens = Some(self.config.thinking_budget);
        }
        if self.is_fast_mode() {
            options.settings = Some(json!({ "fastMode": true }).to_string());
        }
        options
    }

    pub fn send_request(&mut self, user_input: &str, _mode: RequestMode) {
        self.refresh_system_prompt();

        // First request: spawn the driver task and wire channels.
        if self.cmd_tx.is_none() {
            let (step_tx, step_rx) = mpsc::unbounded_channel::<AgentStep>();
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<DriverCmd>();

            let bridge = Arc::new(CodeyToolBridge {
                tools: self.tool_descriptors(),
                step_tx: step_tx.clone(),
                pending: self.pending.clone(),
            });
            let options = self.build_options(bridge);

            self.step_rx = Some(step_rx);
            self.cmd_tx = Some(cmd_tx);
            self.driver = Some(tokio::spawn(driver_loop(options, cmd_rx, step_tx)));
        }

        self.last_text.clear();
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(DriverCmd::Query(user_input.to_string()));
        }
    }

    pub async fn next(&mut self) -> Option<AgentStep> {
        let rx = self.step_rx.as_mut()?;
        let step = rx.recv().await?;
        match &step {
            AgentStep::TextDelta(t) => self.last_text.push_str(t),
            AgentStep::Finished { usage } => self.total_usage += *usage,
            _ => {},
        }
        Some(step)
    }

    pub fn submit_tool_result(&mut self, call_id: &str, content: String) {
        let pending = self.pending.clone();
        let call_id = call_id.to_string();
        // Fulfill the oneshot the MCP bridge is awaiting.
        tokio::spawn(async move {
            if let Some(tx) = pending.lock().await.remove(&call_id) {
                let _ = tx.send(content);
            } else {
                warn!("submit_tool_result: no pending tool for call_id={call_id}");
            }
        });
    }

    pub fn cancel(&mut self) {
        debug!("SdkAgent::cancel");
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(DriverCmd::Interrupt);
        }
    }

    fn refresh_system_prompt(&mut self) {
        if let Some(builder) = &self.system_prompt_builder {
            let new_prompt = builder();
            if new_prompt != self.system_prompt {
                self.system_prompt = new_prompt;
            }
        }
    }
}

impl Drop for SdkAgent {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
        }
    }
}

/// Driver task: owns the `ClaudeClient`, sends prompts, and forwards messages.
async fn driver_loop(
    options: ClaudeAgentOptions,
    mut cmd_rx: mpsc::UnboundedReceiver<DriverCmd>,
    step_tx: mpsc::UnboundedSender<AgentStep>,
) {
    let mut client = ClaudeClient::new(options);
    if let Err(e) = client.connect().await {
        let _ = step_tx.send(AgentStep::Error(format!("failed to start claude: {e:#}")));
        return;
    }

    loop {
        // Wait for the next prompt (ignore stray interrupts while idle).
        let prompt = loop {
            match cmd_rx.recv().await {
                Some(DriverCmd::Query(p)) => break p,
                Some(DriverCmd::Interrupt) => continue,
                None => return,
            }
        };

        if let Err(e) = client.query(prompt).await {
            let _ = step_tx.send(AgentStep::Error(format!("query failed: {e:#}")));
            continue;
        }

        let mut stream = client.receive_messages();
        loop {
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        Some(Ok(message)) => {
                            if forward_message(message, &step_tx) {
                                break; // turn complete (Result)
                            }
                        },
                        Some(Err(e)) => {
                            let _ = step_tx.send(AgentStep::Error(format!("stream error: {e:#}")));
                            break;
                        },
                        None => break,
                    }
                },
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(DriverCmd::Interrupt) => {
                            let _ = client.interrupt().await;
                        },
                        Some(DriverCmd::Query(_)) => { /* ignore mid-turn */ },
                        None => return,
                    }
                },
            }
        }
    }
}

/// Forward a single `Message` to the step channel. Returns true when the turn
/// is complete (a `Result` message was seen).
fn forward_message(message: Message, step_tx: &mpsc::UnboundedSender<AgentStep>) -> bool {
    match message {
        Message::StreamEvent(ev) => {
            // `ev.event` is the raw Anthropic streaming event JSON.
            if let Some(delta) = ev.event.get("delta") {
                match delta.get("type").and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        if let Some(t) = delta.get("text").and_then(|t| t.as_str()) {
                            let _ = step_tx.send(AgentStep::TextDelta(t.to_string()));
                        }
                    },
                    Some("thinking_delta") => {
                        if let Some(t) = delta.get("thinking").and_then(|t| t.as_str()) {
                            let _ = step_tx.send(AgentStep::ThinkingDelta(t.to_string()));
                        }
                    },
                    _ => {},
                }
            }
            false
        },
        Message::Result(result) => {
            let usage = parse_usage(result.usage.as_ref());
            let _ = step_tx.send(AgentStep::Finished { usage });
            true
        },
        Message::Assistant(_) | Message::System(_) | Message::User(_) => {
            // Streaming deltas already cover assistant text; full assistant
            // messages are not re-emitted to avoid duplication.
            false
        },
        Message::ControlCancelRequest(_) => false,
    }
}

/// Best-effort extraction of token usage from a `result`/assistant usage object.
fn parse_usage(usage: Option<&Value>) -> Usage {
    let Some(u) = usage else {
        return Usage::default();
    };
    let get = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let input = get("input_tokens");
    let output = get("output_tokens");
    let cache_creation = get("cache_creation_input_tokens");
    let cache_read = get("cache_read_input_tokens");
    Usage {
        output_tokens: output,
        context_tokens: input + cache_creation + cache_read,
        cache_creation_tokens: cache_creation,
        cache_read_tokens: cache_read,
    }
}
