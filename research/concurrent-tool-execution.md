# Concurrent Tool Execution

> Design document for enabling concurrent/background tool execution in Codey.

## Status: Phase 4 Complete

**Note on BackgroundStarted timing**: `BackgroundStarted` is emitted AFTER approval (if required), not before. This ensures the agent doesn't receive the "Running in background" placeholder until the tool has actually been approved and started execution.

## Problem Statement

Currently, tools execute sequentially - the agent waits for each tool to complete before continuing. This is inefficient for:
- Long-running shell commands (builds, tests)
- Web fetches
- Sub-agent research tasks
- Any operation where the agent could productively continue while waiting

## Design Goals

1. **Agent ergonomics**: Simple `background: true` parameter to opt-in
2. **Async-first architecture**: Concurrent execution is the default internally; blocking is the special case
3. **Minimal API changes**: Leverage existing streaming/event infrastructure
4. **Clean result handling**: Background results retrievable via dedicated tool

## Agent-Facing API

### Background Parameter

Any tool can accept `background: true`:

```json
{
  "name": "shell",
  "parameters": {
    "command": "cargo build --release",
    "background": true
  }
}
```

### Immediate Response

When `background: true`, agent immediately receives:

```
Running in background (task_id: call_abc123)
```

### Completion Notification

On the next turn after a background task completes, the agent sees a system message:

```
Background task completed: shell (call_abc123)
```

### Retrieving Results

Two tools for managing background tasks:

**`list_background_tasks`** - list all background tasks:
```json
{
  "name": "list_background_tasks",
  "parameters": {}
}
```

Returns:
```
call_abc123 (shell) [Running]
call_def456 (fetch_url) [Complete]
```

**`get_background_task`** - retrieve and remove a result:
```json
{
  "name": "get_background_task",
  "parameters": {
    "task_id": "call_abc123"
  }
}
```

Returns the output and removes the task from tracking.

## Internal Architecture

### Current State

```rust
pub struct ToolExecutor {
    tools: ToolRegistry,
    pending: VecDeque<ToolCall>,
    active: Option<ActivePipeline>,  // Single active pipeline
    cancelled: bool,
}
```

- One tool executes at a time
- `next()` returns events from the single active pipeline
- App waits for `Completed` before resuming agent

### Proposed Changes

Minimal changes to existing types - no new structs needed:

```rust
// ToolCall - add one field
pub struct ToolCall {
    pub agent_id: AgentId,
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub decision: ToolDecision,
    pub background: bool,  // NEW
}

// ActivePipeline - add two fields
struct ActivePipeline {
    agent_id: AgentId,
    call_id: String,
    name: String,
    params: serde_json::Value,
    pipeline: ToolPipeline,
    output: String,
    pending_effect: Option<oneshot::Receiver<EffectResult>>,
    pending_approval: Option<oneshot::Receiver<ToolDecision>>,
    original_decision: ToolDecision,
    background: bool,          // NEW - copied from ToolCall
    status: transcript::Status, // NEW - reuse existing Status enum
}

// Reuse existing enum from src/transcript.rs:
// pub enum Status { Pending, Running, Complete, Error, Denied }

// ToolExecutor - change Option to HashMap
pub struct ToolExecutor {
    tools: ToolRegistry,
    pending: VecDeque<ToolCall>,
    active: HashMap<String, ActivePipeline>,  // CHANGED from Option<ActivePipeline>
    cancelled: bool,
}

// ToolEvent - add two variants
pub enum ToolEvent {
    AwaitingApproval { ... },
    Delegate { ... },
    Delta { ... },
    Completed { ... },
    Error { ... },
    BackgroundStarted { agent_id, call_id, name },  // NEW
    BackgroundCompleted { agent_id, call_id, name },  // NEW
}

// Effect - add one variant
pub enum Effect {
    IdeOpen { ... },
    // ... existing ...
    CheckBackgroundTasks { task_id: Option<String> },  // NEW
}
```

Note: `ActivePipeline.output` is used during execution. When a background task completes, we keep it in `active` - completion is indicated by `pipeline` being empty (no more steps). The `take_result()` method returns `output` and removes the entry.

- Multiple tools execute concurrently
- `next()` polls all active pipelines, returns first ready event
- App decides blocking behavior based on `background` flag

### ToolExecutor Changes

```rust
impl ToolExecutor {
    pub async fn next(&mut self) -> Option<ToolEvent> {
        // Check cancellation
        if self.cancelled {
            self.cancelled = false;
            return None;
        }

        // Start ALL pending tools (not just one)
        while let Some(tool_call) = self.pending.pop_front() {
            let pipeline = self.tools.get(&tool_call.name).compose(tool_call.params.clone());
            let background = tool_call.background;
            self.active.insert(
                tool_call.call_id.clone(),
                ActivePipeline::new(tool_call, pipeline)
            );
            
            // Emit BackgroundStarted immediately for background tools
            if background {
                return Some(ToolEvent::BackgroundStarted { ... });
            }
        }

        // Poll all active pipelines
        for (call_id, pipeline) in &mut self.active {
            if let Some(event) = self.poll_pipeline(pipeline) {
                match &event {
                    ToolEvent::Completed { .. } | ToolEvent::Error { .. } => {
                        if pipeline.background {
                            // Background: update status, keep in active
                            // Emit BackgroundCompleted instead
                            pipeline.status = Status::Complete; // or Error
                            return Some(ToolEvent::BackgroundCompleted { ... });
                        } else {
                            // Blocking: remove from active
                            self.active.remove(call_id);
                            return Some(event);
                        }
                    }
                    _ => return Some(event),
                }
            }
        }
        
        None
    }
}
```

### App-Level Logic (Minimal)

App is purely reactive - just handles events, no state tracking:

```rust
match tool_event {
    ToolEvent::BackgroundStarted { call_id, name, .. } => {
        // Feed placeholder to agent immediately
        agent.add_tool_result(&call_id, format!(
            "Running in background (task_id: {})", 
            call_id
        ));
    }
    
    ToolEvent::Completed { call_id, content, .. } => {
        // Blocking tool finished - feed result to agent
        agent.add_tool_result(&call_id, content);
    }
    
    ToolEvent::BackgroundCompleted { call_id, name, .. } => {
        // Add notification to chat - agent sees it and can call check_tasks
        agent.add_system_message(format!(
            "Background task completed: {} ({})",
            name, call_id
        ));
    }
}
```

### ToolExecutor State & Queries

Only two methods needed for `check_tasks` tool:

```rust
use crate::transcript::Status;

impl ToolExecutor {
    /// List all background tasks: (call_id, tool_name, status)
    pub fn list_tasks(&self) -> Vec<(&str, &str, Status)> {
        self.active.values()
            .filter(|p| p.background)
            .map(|p| (p.call_id.as_str(), p.name.as_str(), p.status))
            .collect()
    }
    
    /// Take a completed/failed background result by call_id (removes from tracking)
    pub fn take_result(&mut self, call_id: &str) -> Option<(String, String, Status)> {
        match self.active.get(call_id) {
            Some(p) if p.background && p.status != Status::Running => {
                let p = self.active.remove(call_id).unwrap();
                Some((p.name, p.output, p.status))
            }
            _ => None,
        }
    }
}
```

### Background Task Tools

Two tools, each with its own Effect:

```rust
// list_background_tasks - no parameters
impl Tool for ListBackgroundTasks {
    fn compose(&self, _params: Value) -> ToolPipeline {
        ToolPipeline::new()
            .effect(Effect::ListBackgroundTasks)
    }
}

// get_background_task - requires task_id
impl Tool for GetBackgroundTask {
    fn compose(&self, params: Value) -> ToolPipeline {
        let task_id: String = /* parse from params */;
        ToolPipeline::new()
            .effect(Effect::GetBackgroundTask { task_id })
    }
}

// Effect variants
pub enum Effect {
    // ... existing ...
    ListBackgroundTasks,
    GetBackgroundTask { task_id: String },
}

// In App, handle the effects:
Effect::ListBackgroundTasks => {
    let tasks = executor.list_tasks();
    if tasks.is_empty() {
        Ok(Some("No background tasks".to_string()))
    } else {
        let output = tasks.iter()
            .map(|(call_id, name, status)| format!("{} ({}) [{:?}]", call_id, name, status))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Some(output))
    }
}

Effect::GetBackgroundTask { task_id } => {
    match executor.take_result(&task_id) {
        Some((tool_name, output, status)) => {
            Ok(Some(format!("Task {} ({}) [{:?}]:\n{}", task_id, tool_name, status, output)))
        }
        None => Ok(Some(format!("Task {} not found or still running", task_id))),
    }
}
```

## Event Flow Diagrams

### Blocking Tool (default)

```
Agent                    App                     ToolExecutor
  |                       |                           |
  |-- tool_use(shell) --> |                           |
  |                       |-- enqueue(shell) -------> |
  |                       |                           |-- execute
  |                       | <-- Delta(output) --------|
  |                       | <-- Completed(result) ----|
  | <-- tool_result ------|                           |
  |                       |                           |
  |-- continue response ->|                           |
```

### Background Tool

```
Agent                    App                     ToolExecutor
  |                       |                           |
  |-- tool_use(shell,    |                           |
  |    background=true) ->|                           |
  |                       |-- enqueue(shell) -------> |
  |                       | <-- BackgroundStarted ----|-- add to `active` (background=true)
  | <-- tool_result ------|                           |
  |    "Running in bg"    |                           |
  |                       |                           |
  |-- continue response ->|                           |
  |                       |                           |
  |                       | <-- BackgroundCompleted --|-- pipeline done, stays in active
  | <-- system_message ---|                           |
  |    "Task completed"   |                           |
  |                       |                           |
  |-- check_tasks(id) --->|                           |
  |                       |-- Effect::CheckBgTasks -->|
  |                       | <-- result ---------------|-- take_result() removes from active
  | <-- tool_result ------|                           |
  |    (actual output)    |                           |
```

## Implementation Plan

### Phase 1: ToolExecutor Concurrency ✅
- [x] Add `background: bool` and `status: Status` to `ActivePipeline` (reuse `transcript::Status`)
- [x] Add `background: bool` to `ToolCall`
- [x] Change `active: Option<ActivePipeline>` to `HashMap<String, ActivePipeline>`
- [x] Update `next()` to start all pending tools
- [x] Update `next()` to poll all active pipelines
- [x] On completion: set status, blocking removes from active, background keeps
- [x] Add `BackgroundStarted` / `BackgroundCompleted` variants to `ToolEvent`
- [x] Add `list_tasks()`, `take_result()` methods
- [x] Update `cancel()` to handle multiple pipelines
- [ ] Add tests for concurrent execution

### Phase 2: Background Flag Plumbing ✅
- [x] Extract `background` from tool params when creating ToolCall (in agent.rs From impl)
- [ ] Update tool schemas to document `background` parameter (optional on all tools)

### Phase 3: App Event Handling ✅
- [x] Handle `BackgroundStarted` - send placeholder to agent
- [x] Handle `BackgroundCompleted` - add notification via alert

### Phase 4: Background Task Tools ✅
- [x] Add `ListBackgroundTasks` and `GetBackgroundTask { task_id }` variants to `Effect`
- [x] Implement `list_background_tasks` tool
- [x] Implement `get_background_task` tool
- [x] Add both to tool registry
- [x] Handle effects in App.apply_effect()

### Phase 5: Polish & Testing
- [ ] Handle edge cases (cancellation, errors, timeouts)
- [ ] Add integration tests
- [ ] Update documentation
- [ ] Consider timeout/expiry for uncollected results

## Open Questions (Resolved)

1. **Approval flow for background tools**: Should background tools still require approval?
   - *Decision*: Yes, approval bubbles up naturally. The pipeline emits `AwaitingApproval` like normal - App handles it the same way. Background just affects when we return placeholder vs wait for result.

2. **Error handling**: How do we surface errors from background tools?
   - *Decision*: Errors stored in `output` field, same as results. `list_tasks()` shows status (Running/Completed/Failed). Agent calls `check_tasks(task_id)` to retrieve error message.

3. **Resource limits**: Should we limit concurrent background tasks?
   - *Decision*: Ignore for now.

4. **Result expiry**: Should uncollected results expire?
   - *Decision*: Ignore for now.

5. **Sub-agent interaction**: The `task` tool spawns sub-agents. Should it use this same background mechanism?
   - *Decision*: Future consideration. (Sub-agents spawning sub-agents is a fun rabbit hole.)

## Related Files

- `src/tools/exec.rs` - ToolExecutor implementation
- `src/app.rs` - App event loop, tool result handling
- `src/tools/impls/` - Individual tool implementations
- `src/tools/impls/background_tasks.rs` - ListBackgroundTasksTool, GetBackgroundTaskTool
- `src/tools/handlers.rs` - Effect handlers including ListBackgroundTasks, GetBackgroundTask
- `src/tools/pipeline.rs` - Effect enum with ListBackgroundTasks, GetBackgroundTask variants
- `src/llm/agent.rs` - Agent tool result handling, background param extraction

## Changelog

- **2025-01-11**: Phases 2-4 complete - background flag extraction, event handling, and background task tools
- **2024-XX-XX**: Phase 1 complete - ToolExecutor concurrency infrastructure
- **2024-XX-XX**: Initial design document created
