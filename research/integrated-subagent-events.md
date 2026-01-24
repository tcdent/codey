# Integrated Sub-Agent Event Architecture

## Overview

This document proposes integrating spawned sub-agents into the main application event loop, enabling:
- Tool approval requests to bubble up to the user
- Full visibility into sub-agent activity
- Agent management (list/get) similar to background tasks
- Clean result extraction (last message only)

## Current Architecture

Sub-agents currently bypass the main event loop entirely:

```
┌─────────────────────────────────────────────────────────────┐
│ Main Event Loop (tokio::select!)                            │
│                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────────────┐    │
│  │ term_events │  │ ide_events  │  │ agents.next()    │    │
│  └─────────────┘  └─────────────┘  └──────────────────┘    │
│                                           │                 │
│  ┌─────────────┐  ┌─────────────┐         ▼                │
│  │ tool_events │  │ msg_queue   │    Primary Agent         │
│  └─────────────┘  └─────────────┘         │                │
│         │                                 │                 │
│         ▼                                 ▼                 │
│    ToolExecutor ◄──────────────── AgentStep::ToolRequest   │
│         │                                                   │
│         ▼                                                   │
│  ToolEvent::AwaitingApproval ───► User y/n                 │
│                                                             │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ Separate: run_agent() in background.rs                      │
│                                                             │
│  SpawnAgent tool                                            │
│       │                                                     │
│       ▼                                                     │
│  RunAgent handler                                           │
│       │                                                     │
│       ▼                                                     │
│  run_agent(agent, tools)  ◄── Runs to completion           │
│       │                                                     │
│       ├── execute_tool()  ◄── Auto-approves everything     │
│       │       │                                             │
│       │       └── Step::AwaitApproval → ignored            │
│       │       └── Step::Delegate → ignored                 │
│       │                                                     │
│       └── Returns output string                            │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

**Problems:**
1. Sub-agent tools auto-approve - no user oversight for write operations
2. Delegates ignored - no IDE integration for sub-agents
3. No visibility into sub-agent progress
4. Can't manage/list running sub-agents

## Proposed Architecture

Integrate sub-agents as first-class participants in the event loop:

```
┌─────────────────────────────────────────────────────────────┐
│ Main Event Loop (tokio::select!)                            │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ agents.next() - polls ALL registered agents         │   │
│  │                                                     │   │
│  │   Primary Agent (id=0)                              │   │
│  │   Sub-Agent "research" (id=1)                       │   │
│  │   Sub-Agent "analyze" (id=2)                        │   │
│  │   ...                                               │   │
│  └─────────────────────────────────────────────────────┘   │
│         │                                                   │
│         ▼                                                   │
│  AgentStep::ToolRequest { agent_id, calls }                │
│         │                                                   │
│         ▼                                                   │
│    ToolExecutor.enqueue(calls.with_agent_id(agent_id))     │
│         │                                                   │
│         ▼                                                   │
│  ToolEvent::AwaitingApproval { agent_id, ... }             │
│         │                                                   │
│         ▼                                                   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Approval routing:                                   │   │
│  │   if is_primary || requires_user_approval(tool):   │   │
│  │       show UI, wait for y/n                        │   │
│  │   else:                                            │   │
│  │       auto-approve                                 │   │
│  └─────────────────────────────────────────────────────┘   │
│         │                                                   │
│         ▼                                                   │
│  ToolEvent::Completed { agent_id, call_id, content }       │
│         │                                                   │
│         ▼                                                   │
│  agents.get(agent_id).submit_tool_result(call_id, content) │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## Key Components

### 1. Enhanced AgentRegistry

```rust
pub struct AgentRegistry {
    agents: HashMap<AgentId, Mutex<Agent>>,
    metadata: HashMap<AgentId, AgentMetadata>,
    next_id: AgentId,
    primary: Option<AgentId>,
}

pub struct AgentMetadata {
    /// Short label for UI display (e.g., "research codebase")
    pub label: String,
    /// Parent agent that spawned this one (None for primary)
    pub parent_id: Option<AgentId>,
    /// Channel to send result back to parent's tool pipeline
    pub result_sender: Option<oneshot::Sender<String>>,
    /// When the agent was spawned
    pub created_at: Instant,
    /// Current status
    pub status: AgentStatus,
}

pub enum AgentStatus {
    Running,
    Finished,
    Error(String),
}

impl AgentRegistry {
    /// Register a spawned sub-agent
    pub fn register_spawned(
        &mut self,
        agent: Agent,
        label: String,
        parent_id: AgentId,
        result_sender: oneshot::Sender<String>,
    ) -> AgentId {
        let id = self.next_id;
        self.next_id += 1;

        self.agents.insert(id, Mutex::new(agent));
        self.metadata.insert(id, AgentMetadata {
            label,
            parent_id: Some(parent_id),
            result_sender: Some(result_sender),
            created_at: Instant::now(),
            status: AgentStatus::Running,
        });

        id
    }

    /// Get metadata for an agent
    pub fn metadata(&self, id: AgentId) -> Option<&AgentMetadata> {
        self.metadata.get(&id)
    }

    /// Mark agent as finished and extract result sender
    pub fn finish(&mut self, id: AgentId) -> Option<oneshot::Sender<String>> {
        if let Some(meta) = self.metadata.get_mut(&id) {
            meta.status = AgentStatus::Finished;
            meta.result_sender.take()
        } else {
            None
        }
    }
}
```

### 2. SpawnAgent via Effect

Instead of running the agent directly, delegate to App:

```rust
// In spawn_agent.rs
pub enum Effect {
    // ... existing effects ...

    /// Spawn a sub-agent, returning its ID
    SpawnAgent {
        agent: Agent,
        label: String,
        /// Sender for the result when agent completes
        result_sender: oneshot::Sender<String>,
    },
}

struct RunAgent {
    task: String,
    task_context: Option<String>,
}

#[async_trait]
impl EffectHandler for RunAgent {
    async fn call(self: Box<Self>) -> Step {
        let ctx = agent_context()?;
        let oauth = ctx.oauth.read().await.clone();

        let system_prompt = build_prompt(&self.task_context);
        let tools = ToolRegistry::subagent_with_edit(); // Now includes edit_file

        let mut agent = Agent::new(
            ctx.runtime_config.clone(),
            &system_prompt,
            oauth,
            tools,
        );
        agent.send_request(&self.task, RequestMode::Normal);

        // Create channel for result
        let (tx, rx) = oneshot::channel();

        // Delegate to App to register the agent
        Step::Delegate(Effect::SpawnAgent {
            agent,
            label: truncate(&self.task, 30),
            result_sender: tx,
        })

        // After delegation, wait for result
        // (This requires pipeline support for async continuation)
    }
}
```

### 3. App Handling

```rust
// In app.rs apply_effect()
Effect::SpawnAgent { agent, label, result_sender } => {
    let parent_id = agent_id; // The agent that called spawn_agent
    let child_id = self.agents.register_spawned(
        agent,
        label,
        parent_id,
        result_sender,
    );

    tracing::info!("Spawned sub-agent {} with label '{}'", child_id, label);
    Ok(format!("agent:{}", child_id))
}
```

### 4. Agent Completion Handling

```rust
// In handle_agent_step()
AgentStep::Finished { usage } => {
    let is_primary = self.agents.primary_id() == Some(agent_id);

    if !is_primary {
        // Sub-agent finished - extract result and notify parent
        let result = {
            let agent = self.agents.get(agent_id).unwrap();
            agent.lock().await.last_message().unwrap_or_default()
        };

        if let Some(sender) = self.agents.finish(agent_id) {
            let _ = sender.send(result);
        }

        // Optionally remove from registry after short delay
        // (allows list_agents to show recently finished)
    }
}
```

### 5. Approval Routing

```rust
// In handle_tool_event()
ToolEvent::AwaitingApproval { agent_id, name, params, .. } => {
    let is_primary = self.agents.primary_id() == Some(agent_id);

    // Tools that always require user approval, even from sub-agents
    let requires_approval = matches!(name.as_str(),
        "mcp_edit_file" | "mcp_write_file" | "mcp_shell"
    );

    if is_primary || requires_approval {
        // Show approval UI with agent context
        let label = self.agents.metadata(agent_id)
            .map(|m| m.label.clone());

        let tool_call = ToolCall {
            agent_id,
            agent_label: label,  // New field
            // ...
        };

        self.pending_approvals.push_back((tool_call, responder));
        // ... show UI
    } else {
        // Auto-approve read-only tools from sub-agents
        let _ = responder.send(ToolDecision::Approve);
    }
}
```

### 6. Agent Management Tools

```rust
// list_agents tool
pub struct ListAgentsTool;

impl Tool for ListAgentsTool {
    fn name(&self) -> &'static str { "mcp_list_agents" }

    fn description(&self) -> &'static str {
        "List all spawned sub-agents and their status"
    }

    fn compose(&self, _params: Value) -> ToolPipeline {
        ToolPipeline::new().then(ListAgentsHandler)
    }
}

struct ListAgentsHandler;

#[async_trait]
impl EffectHandler for ListAgentsHandler {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::ListAgents)
    }
}

// get_agent tool
pub struct GetAgentTool;

impl Tool for GetAgentTool {
    fn name(&self) -> &'static str { "mcp_get_agent" }

    fn description(&self) -> &'static str {
        "Get the result from a finished sub-agent"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "integer",
                    "description": "The agent ID returned from spawn_agent"
                }
            },
            "required": ["agent_id"]
        })
    }
}
```

### 7. Result Extraction (Last Message Only)

```rust
impl Agent {
    /// Extract just the final assistant message for returning to parent
    pub fn last_message(&self) -> Option<String> {
        // Walk messages in reverse, find last assistant content
        for msg in self.messages.iter().rev() {
            if let Message::Assistant { content, .. } = msg {
                // Filter to just text blocks, skip tool_use
                let text: String = content.iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}
```

## UI Considerations

### Block Rendering with Agent Label

```rust
impl ToolBlock {
    fn render(&self, width: u16) -> Vec<Line<'_>> {
        let mut spans = vec![self.render_status()];

        // Add agent label for non-primary agents
        if let Some(label) = &self.agent_label {
            spans.push(Span::styled(
                format!("[{}] ", label),
                Style::default().fg(Color::Cyan),
            ));
        }

        spans.push(Span::styled(&self.tool_name, Style::default().fg(Color::Magenta)));
        // ... rest of rendering
    }
}
```

Example output:
```
◐ [research codebase] edit_file(src/lib.rs)
  - old_string: "fn old()"
  + new_string: "fn new()"
  [y]es [n]o
```

### Agent Status in Status Bar

```
╭─────────────────────────────────────────────────────────────╮
│ Agents: 1 primary, 2 running, 1 finished                    │
╰─────────────────────────────────────────────────────────────╯
```

## Event Flow Example

```
1. Primary agent calls spawn_agent(task: "research auth system")

2. SpawnAgent tool emits Effect::SpawnAgent { agent, label, result_sender }

3. App handles effect:
   - Registers agent with id=1, label="research auth system"
   - Stores result_sender for later
   - Returns "agent:1" to complete the spawn_agent tool

4. Primary agent continues, sub-agent now polled in agents.next()

5. Sub-agent requests edit_file tool

6. ToolExecutor emits ToolEvent::AwaitingApproval { agent_id: 1, ... }

7. App sees agent_id != primary AND tool is edit_file:
   - Creates ToolCall with agent_label: Some("research auth system")
   - Shows approval UI: "[research auth system] edit_file(...)"

8. User approves (y)

9. Tool executes, ToolEvent::Completed routed to agent 1

10. Sub-agent finishes (AgentStep::Finished)

11. App:
    - Calls agent.last_message() to get final response
    - Sends result through stored result_sender
    - Marks agent as Finished in registry

12. Primary agent's spawn_agent tool receives result, continues
```

## Migration Path

1. **Phase 1**: Add AgentMetadata to registry, no behavior change
2. **Phase 2**: Add Effect::SpawnAgent, keep existing run_agent() as fallback
3. **Phase 3**: Route sub-agent tools through ToolExecutor with auto-approve
4. **Phase 4**: Add approval routing for write tools
5. **Phase 5**: Add agent management tools (list/get)
6. **Phase 6**: Add UI labels and status bar

## Open Questions

1. **Timeout for sub-agents?** Should there be a maximum runtime?

2. **Cancellation?** If user cancels primary, cancel all sub-agents?

3. **Resource limits?** Maximum concurrent sub-agents?

4. **Sub-sub-agents?** Allow spawned agents to spawn? (Probably no initially)

5. **Result size?** Truncate/summarize large last_message results?

6. **Approval batching?** If sub-agent requests 5 edits, show all at once?
