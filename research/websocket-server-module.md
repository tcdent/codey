# WebSocket Server Module

## Current Status

**Phase 1: Serialization** ✅ Complete
- Added `Serialize`/`Deserialize` to `AgentStep`, `Usage`, `RequestMode`, `ToolCall`, `ToolDecision`, `Effect`
- Added `ToolEventMessage` with `to_message()` conversion from `ToolEvent`
- Exported new types from `lib.rs` public API

**Phase 2: Workspace Restructure** ✅ Complete (simplified)
- Created workspace with `crates/codey` and `crates/codey-server`
- Kept tool implementations in `codey` with `cli` feature (simpler than separate `codey-tools` crate)
- `codey` crate produces both library and `codey` binary
- `codey-server` depends on `codey` with `cli` feature for full tool access

**Phase 3: codey-server Skeleton** ✅ Complete
- `protocol.rs`: `ClientMessage` and `ServerMessage` enums
- `session.rs`: Per-connection session with event loop
- `server.rs`: WebSocket listener and connection handling
- `main.rs`: CLI entry point

**Phase 4: Full ToolExecutor Integration** ✅ Complete
- `ToolExecutor` integrated for server-side tool execution
- `ToolFilters` loaded from config for auto-approve/deny
- Config file format shared with CLI (~/.config/codey/config.toml)
- Tools matching filters are auto-approved/denied, others bubble to WebSocket

**Phase 5: Integration Testing** 🔲 Planned
- TODO: Add test client
- TODO: End-to-end tests

## Overview

This document outlines the plan to add a WebSocket server module (`codey-server`) that exposes the full agent interaction over WebSocket, enabling automation and integration with external clients while keeping the core CLI unaltered.

## Goals

1. **Daemonized agent access**: Run codey as a background service that clients can connect to
2. **Full tool execution**: Server-side tool execution with approval promotion over WebSocket
3. **Streaming responses**: Real-time streaming of agent output (text, thinking, tool events)
4. **Minimal core changes**: Keep the existing CLI working exactly as-is
5. **Shared primitives**: Reuse `Agent`, `ToolExecutor`, and existing types where possible

## Architecture

### Workspace Structure (Actual Implementation)

```
codey/
├── Cargo.toml                    # workspace root with shared deps
├── crates/
│   ├── codey/                    # core library + CLI binary
│   │   ├── Cargo.toml            # has 'cli' feature for TUI/tools
│   │   └── src/
│   │       ├── lib.rs            # public API exports
│   │       ├── main.rs           # CLI entry point (requires cli feature)
│   │       ├── app.rs            # TUI event loop
│   │       ├── ui/               # TUI components
│   │       ├── llm/              # Agent, AgentStep, Usage
│   │       ├── tools/            # ToolExecutor + implementations
│   │       │   ├── exec.rs       # ToolExecutor, ToolEvent, ToolEventMessage
│   │       │   ├── handlers.rs   # Effect handlers
│   │       │   └── impls/        # Tool implementations
│   │       └── ...
│   │
│   └── codey-server/             # WebSocket server binary
│       ├── Cargo.toml            # depends on codey with cli feature
│       └── src/
│           ├── main.rs           # Server entry point
│           ├── protocol.rs       # ClientMessage/ServerMessage
│           ├── server.rs         # WebSocket listener
│           └── session.rs        # Per-connection session
```

Note: The original plan included a separate `codey-tools` crate, but the
implementation keeps tools in the `codey` crate behind the `cli` feature
for simplicity. This can be refactored later if needed.

### Dependency Graph

```
codey-server ──► codey (with cli feature)
                    │
                    ├── Agent, AgentStep, Usage
                    ├── ToolExecutor, ToolEvent, ToolEventMessage
                    └── Tool implementations (read_file, shell, etc.)

External clients ──► codey-server (WebSocket)
Library users ──────► codey (no cli feature, just core Agent)
```

## Implementation Plan

### Phase 1: Serialization Support

Add `Serialize`/`Deserialize` to core types that will be sent over the wire.

**Files to modify:**

1. `src/llm/agent.rs`:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub enum AgentStep { ... }

   #[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
   pub struct Usage { ... }
   ```

2. `src/tools/exec.rs`:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct ToolCall { ... }

   #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
   pub enum ToolDecision { ... }

   // NEW: Serializable version of ToolEvent
   /// Serializable version of [`ToolEvent`] for wire protocols (WebSocket, IPC, etc.)
   ///
   /// This mirrors `ToolEvent` but omits the `oneshot::Sender` responder channels
   /// which cannot be serialized. The internal `ToolEvent` uses channels to implement
   /// the approval flow within a single process, while this type is used for
   /// cross-process or network communication.
   ///
   /// # Why the duplication?
   ///
   /// `ToolEvent` contains `oneshot::Sender<ToolDecision>` for the approval flow -
   /// when a tool needs approval, the executor sends the event with a channel, and
   /// the receiver (TUI or WebSocket server) sends the decision back through that
   /// channel. This is elegant for in-process use but channels can't cross the wire.
   ///
   /// TODO: Consider whether we could restructure to have a single event type with
   /// the responder as an external concern (e.g., keyed by call_id in a separate map).
   /// For now, the duplication is minimal and the conversion is straightforward.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(tag = "type")]
   pub enum ToolEventMessage { ... }

   impl ToolEvent {
       pub fn to_message(&self) -> ToolEventMessage { ... }
   }
   ```

3. `src/tools/pipeline.rs` (if `Effect` needs to be serialized for `Delegate` events):
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub enum Effect { ... }
   ```

4. `src/transcript.rs` (for `Status` enum if included in messages):
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
   pub enum Status { ... }
   ```

### Phase 2: Workspace Restructure

Convert from single crate to workspace with multiple crates.

**Step 2.1: Create workspace root Cargo.toml**

```toml
[workspace]
resolver = "2"
members = [
    "crates/codey",
    "crates/codey-tools",
    "crates/codey-cli",
    "crates/codey-server",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Codey Contributors"]
license = "MIT"
repository = "https://github.com/tcdent/codey"

[workspace.dependencies]
# Shared dependencies with versions pinned at workspace level
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
thiserror = "2"
tracing = "0.1"
# ... etc
```

**Step 2.2: Create crates/codey (core library)**

Move core functionality:
- `src/lib.rs` → `crates/codey/src/lib.rs`
- `src/llm/` → `crates/codey/src/llm/`
- `src/tools/{mod.rs, exec.rs, pipeline.rs, io.rs}` → `crates/codey/src/tools/`
- `src/config.rs` → `crates/codey/src/config.rs`
- `src/auth.rs` → `crates/codey/src/auth.rs`
- `src/transcript.rs` → `crates/codey/src/transcript.rs`
- `src/prompts.rs` → `crates/codey/src/prompts.rs`
- `src/tool_filter.rs` → `crates/codey/src/tool_filter.rs`

**Step 2.3: Create crates/codey-tools**

Move tool implementations:
- `src/tools/impls/*.rs` → `crates/codey-tools/src/`
- `src/tools/handlers.rs` → `crates/codey-tools/src/handlers.rs` (or split)

```rust
// crates/codey-tools/src/lib.rs
pub use read_file::ReadFileTool;
pub use write_file::WriteFileTool;
// ... etc

/// Create a ToolRegistry with all available tools
pub fn full_registry() -> codey::ToolRegistry {
    let mut registry = codey::ToolRegistry::empty();
    registry.register(Arc::new(ReadFileTool));
    registry.register(Arc::new(WriteFileTool));
    // ... etc
    registry
}
```

**Step 2.4: Create crates/codey-cli**

Move TUI-specific code:
- `src/main.rs` → `crates/codey-cli/src/main.rs`
- `src/app.rs` → `crates/codey-cli/src/app.rs`
- `src/ui/` → `crates/codey-cli/src/ui/`
- `src/ide/` → `crates/codey-cli/src/ide/`
- `src/commands.rs` → `crates/codey-cli/src/commands.rs`
- `src/compaction.rs` → `crates/codey-cli/src/compaction.rs`

```toml
# crates/codey-cli/Cargo.toml
[package]
name = "codey-cli"
version.workspace = true

[[bin]]
name = "codey"
path = "src/main.rs"

[dependencies]
codey = { path = "../codey" }
codey-tools = { path = "../codey-tools" }
ratatui = { version = "0.30.0-beta.0", features = ["scrolling-regions"] }
crossterm = { version = "0.28", features = ["event-stream"] }
clap = { version = "4", features = ["derive", "env"] }
nvim-rs = { version = "0.9", features = ["use_tokio"] }
# ... etc
```

**Step 2.5: Create crates/codey-server (stub)**

Initial skeleton for WebSocket server.

### Phase 3: WebSocket Protocol

Define the wire protocol for client-server communication.

**File: `crates/codey-server/src/protocol.rs`**

```rust
use codey::{AgentStep, ToolEventMessage, Usage};
use serde::{Deserialize, Serialize};

// ============================================================================
// Client → Server
// ============================================================================

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
}

// ============================================================================
// Server → Client
// ============================================================================

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Session established, provides session info
    Connected {
        session_id: String,
        /// Resume token for reconnection (optional feature)
        resume_token: Option<String>,
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

    /// Error occurred
    Error {
        message: String,
        /// If true, the session is no longer usable
        fatal: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallInfo {
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub background: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryMessage {
    pub role: String,  // "user", "assistant", "tool"
    pub content: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub id: u32,
    pub name: Option<String>,
    pub is_streaming: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingApproval {
    pub agent_id: u32,
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
}
```

### Phase 4: Session Management

Implement per-connection session handling.

**File: `crates/codey-server/src/session.rs`**

```rust
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};
use codey::{
    Agent, AgentRuntimeConfig, AgentStep, RequestMode,
    ToolCall, ToolDecision, ToolEvent, ToolExecutor,
};
use codey_tools::full_registry;

use crate::protocol::{ClientMessage, ServerMessage};

pub struct Session {
    /// Unique session identifier
    id: String,

    /// The primary agent
    agent: Agent,

    /// Tool executor for server-side tool execution
    tool_executor: ToolExecutor,

    /// Auto-approval filters from config
    filters: ToolFilters,

    /// Pending approvals: call_id -> responder channel
    pending_approvals: HashMap<String, oneshot::Sender<ToolDecision>>,

    /// Channel to send messages to WebSocket writer task
    ws_tx: mpsc::UnboundedSender<ServerMessage>,

    /// Channel to receive messages from WebSocket reader task
    ws_rx: mpsc::UnboundedReceiver<ClientMessage>,
}

impl Session {
    pub fn new(
        config: AgentRuntimeConfig,
        system_prompt: &str,
        oauth: Option<OAuthCredentials>,
        ws_tx: mpsc::UnboundedSender<ServerMessage>,
        ws_rx: mpsc::UnboundedReceiver<ClientMessage>,
    ) -> Self {
        let tools = full_registry();
        let tool_executor = ToolExecutor::new(tools.clone());
        let agent = Agent::new(config, system_prompt, oauth, tools);

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            agent,
            tool_executor,
            filters: ToolFilters::default(),  // TODO: load from config
            pending_approvals: HashMap::new(),
            ws_tx,
            ws_rx,
        }
    }

    /// Main event loop - mirrors app.rs structure
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Send connected message
        self.ws_tx.send(ServerMessage::Connected {
            session_id: self.id.clone(),
            resume_token: None,
        })?;

        loop {
            tokio::select! {
                // Priority 1: WebSocket messages from client
                Some(msg) = self.ws_rx.recv() => {
                    if self.handle_client_message(msg).await? {
                        break; // Client requested disconnect
                    }
                }

                // Priority 2: Agent steps (streaming, tool requests)
                Some(step) = self.agent.next() => {
                    self.handle_agent_step(step).await?;
                }

                // Priority 3: Tool executor events
                Some(event) = self.tool_executor.next() => {
                    self.handle_tool_event(event).await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_client_message(&mut self, msg: ClientMessage) -> anyhow::Result<bool> {
        match msg {
            ClientMessage::SendMessage { content, .. } => {
                self.agent.send_request(&content, RequestMode::Normal);
            }

            ClientMessage::ToolDecision { call_id, approved } => {
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
                self.agent.cancel();
                self.tool_executor.cancel();
            }

            ClientMessage::GetHistory => {
                // TODO: implement history retrieval
            }

            ClientMessage::GetState => {
                // TODO: implement state retrieval
            }
        }

        Ok(false)
    }

    async fn handle_agent_step(&mut self, step: AgentStep) -> anyhow::Result<()> {
        let agent_id = 0; // Primary agent

        match step {
            AgentStep::TextDelta(content) => {
                self.ws_tx.send(ServerMessage::TextDelta { agent_id, content })?;
            }

            AgentStep::ThinkingDelta(content) => {
                self.ws_tx.send(ServerMessage::ThinkingDelta { agent_id, content })?;
            }

            AgentStep::ToolRequest(calls) => {
                // Send tool request notification
                let call_infos: Vec<_> = calls.iter().map(|c| ToolCallInfo {
                    call_id: c.call_id.clone(),
                    name: c.name.clone(),
                    params: c.params.clone(),
                    background: c.background,
                }).collect();

                self.ws_tx.send(ServerMessage::ToolRequest {
                    agent_id,
                    calls: call_infos,
                })?;

                // Enqueue for execution
                self.tool_executor.enqueue(calls);
            }

            AgentStep::Finished { usage } => {
                self.ws_tx.send(ServerMessage::Finished { agent_id, usage })?;
            }

            AgentStep::Retrying { attempt, error } => {
                self.ws_tx.send(ServerMessage::Retrying { agent_id, attempt, error })?;
            }

            AgentStep::Error(message) => {
                self.ws_tx.send(ServerMessage::Error { message, fatal: false })?;
            }

            AgentStep::CompactionDelta(_) => {
                // TODO: decide if we want to expose compaction to clients
            }
        }

        Ok(())
    }

    async fn handle_tool_event(&mut self, event: ToolEvent) -> anyhow::Result<()> {
        match event {
            ToolEvent::AwaitingApproval {
                agent_id,
                call_id,
                name,
                params,
                background,
                responder,
            } => {
                // Check auto-approval filters first
                if self.filters.should_approve(&name, &params) {
                    let _ = responder.send(ToolDecision::Approve);
                    self.ws_tx.send(ServerMessage::ToolStarted {
                        agent_id,
                        call_id,
                        name,
                    })?;
                } else {
                    // Promote to WebSocket for user decision
                    self.pending_approvals.insert(call_id.clone(), responder);
                    self.ws_tx.send(ServerMessage::ToolAwaitingApproval {
                        agent_id,
                        call_id,
                        name,
                        params,
                        background,
                    })?;
                }
            }

            ToolEvent::Delta { agent_id, call_id, content } => {
                self.ws_tx.send(ServerMessage::ToolDelta {
                    agent_id,
                    call_id,
                    content,
                })?;
            }

            ToolEvent::Completed { agent_id, call_id, content } => {
                // Submit result back to agent
                self.agent.submit_tool_result(&call_id, content.clone());

                self.ws_tx.send(ServerMessage::ToolCompleted {
                    agent_id,
                    call_id,
                    content,
                })?;
            }

            ToolEvent::Error { agent_id, call_id, content } => {
                // Submit error back to agent
                self.agent.submit_tool_result(&call_id, format!("Error: {}", content));

                self.ws_tx.send(ServerMessage::ToolError {
                    agent_id,
                    call_id,
                    error: content,
                })?;
            }

            ToolEvent::Delegate { responder, .. } => {
                // For now, reject delegated effects (IDE integration, sub-agents)
                // TODO: implement delegation over WebSocket
                let _ = responder.send(Err("Delegation not supported over WebSocket".to_string()));
            }

            ToolEvent::BackgroundStarted { agent_id, call_id, name } => {
                self.ws_tx.send(ServerMessage::ToolStarted {
                    agent_id,
                    call_id,
                    name,
                })?;
            }

            ToolEvent::BackgroundCompleted { agent_id, call_id, .. } => {
                // Retrieve result and submit to agent
                if let Some((name, output, status)) = self.tool_executor.take_result(&call_id) {
                    self.agent.submit_tool_result(&call_id, output.clone());
                    self.ws_tx.send(ServerMessage::ToolCompleted {
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
```

### Phase 5: WebSocket Server

Implement the server listener and connection handling.

**File: `crates/codey-server/src/server.rs`**

```rust
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use futures::{SinkExt, StreamExt};

use crate::protocol::{ClientMessage, ServerMessage};
use crate::session::Session;

pub struct Server {
    addr: SocketAddr,
    config: ServerConfig,
}

pub struct ServerConfig {
    pub system_prompt: String,
    pub agent_config: AgentRuntimeConfig,
    pub oauth: Option<OAuthCredentials>,
}

impl Server {
    pub fn new(addr: SocketAddr, config: ServerConfig) -> Self {
        Self { addr, config }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(&self.addr).await?;
        tracing::info!("WebSocket server listening on {}", self.addr);

        while let Ok((stream, peer_addr)) = listener.accept().await {
            tracing::info!("New connection from {}", peer_addr);
            let config = self.config.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, config).await {
                    tracing::error!("Connection error: {}", e);
                }
            });
        }

        Ok(())
    }
}

async fn handle_connection(stream: TcpStream, config: ServerConfig) -> anyhow::Result<()> {
    let ws_stream = accept_async(stream).await?;
    let (mut ws_sink, mut ws_source) = ws_stream.split();

    // Channels for session <-> WebSocket communication
    let (tx_to_ws, mut rx_to_ws) = mpsc::unbounded_channel::<ServerMessage>();
    let (tx_from_ws, rx_from_ws) = mpsc::unbounded_channel::<ClientMessage>();

    // Spawn WebSocket writer task
    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = rx_to_ws.recv().await {
            let json = serde_json::to_string(&msg)?;
            ws_sink.send(Message::Text(json)).await?;
        }
        Ok::<_, anyhow::Error>(())
    });

    // Spawn WebSocket reader task
    let reader_handle = tokio::spawn(async move {
        while let Some(msg) = ws_source.next().await {
            match msg? {
                Message::Text(text) => {
                    let client_msg: ClientMessage = serde_json::from_str(&text)?;
                    tx_from_ws.send(client_msg)?;
                }
                Message::Close(_) => break,
                _ => {} // Ignore binary, ping, pong
            }
        }
        Ok::<_, anyhow::Error>(())
    });

    // Create and run session
    let mut session = Session::new(
        config.agent_config,
        &config.system_prompt,
        config.oauth,
        tx_to_ws,
        rx_from_ws,
    );

    session.run().await?;

    // Clean up
    writer_handle.abort();
    reader_handle.abort();

    Ok(())
}
```

**File: `crates/codey-server/src/main.rs`**

```rust
use std::net::SocketAddr;
use clap::Parser;

mod protocol;
mod server;
mod session;

#[derive(Parser)]
#[command(name = "codey-server")]
#[command(about = "WebSocket server for codey agent")]
struct Args {
    /// Address to listen on
    #[arg(short, long, default_value = "127.0.0.1:9999")]
    listen: SocketAddr,

    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Run in foreground (don't daemonize)
    #[arg(long)]
    foreground: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::init();

    let args = Args::parse();

    // Load config
    let config = load_config(args.config)?;

    // TODO: daemonization support

    let server = server::Server::new(args.listen, config);
    server.run().await
}
```

## Implementation Order

1. **Phase 1: Serialization** (can be done without restructure)
   - Add `Serialize`/`Deserialize` to `AgentStep`, `Usage`, `ToolCall`, `ToolDecision`
   - Add `ToolEventMessage` with conversion from `ToolEvent`
   - Test serialization roundtrips

2. **Phase 2: Workspace Restructure**
   - Create workspace Cargo.toml
   - Create `crates/codey/` with core library
   - Create `crates/codey-tools/` with tool implementations
   - Create `crates/codey-cli/` with TUI (verify existing behavior works)
   - Update CI/CD for workspace builds

3. **Phase 3: codey-server Skeleton**
   - Create `crates/codey-server/` structure
   - Implement protocol types
   - Implement basic WebSocket accept loop
   - Test connection establishment

4. **Phase 4: Session Implementation**
   - Implement Session struct with event loop
   - Wire up Agent and ToolExecutor
   - Handle ClientMessage routing
   - Handle AgentStep → ServerMessage conversion
   - Handle ToolEvent → ServerMessage conversion + approval flow

5. **Phase 5: Integration & Testing**
   - End-to-end testing with sample client
   - Error handling and reconnection logic
   - Configuration loading (filters, OAuth, etc.)
   - Documentation

## Future Considerations

### Multi-Agent Support
The protocol includes `agent_id` fields to support multi-agent sessions. This mirrors the existing `AgentRegistry` in the CLI.

### Session Persistence
- Save/restore session state for server restarts
- Resume tokens for client reconnection

### Authentication
- API key authentication for server access
- Per-session OAuth forwarding

### Sub-Agent Delegation
Currently `ToolEvent::Delegate` is rejected over WebSocket. Could potentially:
- Proxy delegation requests to client
- Handle sub-agent spawning server-side

### IDE Integration
The WebSocket protocol could potentially support IDE effects (selections, open files) by forwarding them to the client. This would enable VS Code extension integration.

## References

- Current tool executor: `src/tools/exec.rs`
- Current app event loop: `src/app.rs`
- Library documentation: `LIBRARY.md`
- Sub-agent architecture: `research/sub-agent-architecture.md`
