# Async State Management

## Overview

This document captures findings from analyzing how Codey manages state across its async event loop backends, including bugs discovered and ideas for improvement.

## Current Architecture

The main event loop (`App::run`) uses `tokio::select!` to poll five async sources:

```rust
loop {
    tokio::select! {
        biased;
        Some(term_event) = self.events.next() => { ... }
        Some(ide_event) = async { self.ide.as_mut()?.next().await } => { ... }
        Some((agent_id, agent_step)) = self.agents.next() => { ... }
        Some(tool_event) = self.tool_executor.next() => { ... }
        Some(request) = async { self.message_queue.pop_front() }, if self.input_mode == InputMode::Normal => { ... }
    }
}
```

### Event Sources

| Source | Type | Cancel-safe | Notes |
|--------|------|-------------|-------|
| Terminal events | `EventStream` | ✅ | Crossterm's stream, buffered internally |
| IDE events | `mpsc::Receiver` | ✅ | Messages stay in channel if cancelled |
| Agent registry | `FuturesUnordered` | ✅ | Agent stores state on `self` |
| Tool executor | Custom `next()` | ✅ | Fixed with non-blocking poll pattern |
| Message queue | `VecDeque` | ✅ | Synchronous pop |

## State Tracking Problem

### The Issue: Derived State Tracked Separately

`App` maintains state that should be derived from other sources but is instead tracked independently:

```rust
pub struct App {
    // Primary state (source of truth)
    pending_approval: Option<oneshot::Sender<ToolDecision>>,  // approval channel sender
    
    // Derived state (manually synchronized)
    input_mode: InputMode,  // should mirror executor/agent state
}
```

`InputMode` attempts to reflect:
- `Normal` → agent idle, no tools pending
- `Streaming` → agent streaming OR tool executing
- `ToolApproval` → executor waiting for approval (`pending_approval.is_some()`)

But these are set imperatively at different code points, not derived from the source of truth.

### Bug Discovered: Keystroke Dropping

**Timeline of the bug:**

```
1. ToolEvent::AwaitingApproval received
   ├── pending_approval = Some(responder)    [line 884]
   └── input_mode = ToolApproval             [line 893]

2. User presses 'y' to approve
   └── decide_pending_tool() called
       └── pending_approval.take() → None    [line 819]
       └── (input_mode NOT changed!)         ← BUG

   >>> Window: input_mode=ToolApproval, pending_approval=None <<<

3. Tool executes...
   └── User keystrokes SILENTLY DROPPED (map_key_tool_approval returns None for most keys)

4. ToolEvent::Completed received
   └── input_mode = Streaming                [line 974]
```

**Impact:** Any keystrokes between approval and tool completion were silently ignored.

**Fix applied:** Set `input_mode = Streaming` immediately in `decide_pending_tool()` after consuming the responder (line 828).

## State Inventory

### App State
| Field | Purpose | Source of Truth? |
|-------|---------|------------------|
| `input_mode` | UI keybinding mode | ❌ Derived - mirrors executor/agent |
| `pending_approval` | Approval channel sender | ✅ Primary (half of oneshot) |
| `message_queue` | Pending user messages | ✅ Primary |
| `should_quit` | Exit flag | ✅ Primary |
| `alert` | Status message | ✅ Primary |

### ToolExecutor State
| Field | Purpose | Source of Truth? |
|-------|---------|------------------|
| `active` | Currently running tool | ✅ Primary |
| `active.pending_approval` | Approval channel receiver | ✅ Primary (half of oneshot) |
| `active.pending_effect` | Effect channel receiver | ✅ Primary |
| `pending` | Queued tool calls | ✅ Primary |
| `cancelled` | Cancellation flag | ⚠️ Maybe redundant |

### Agent State
| Field | Purpose | Source of Truth? |
|-------|---------|------------------|
| `state` | Agent FSM (NeedsChatRequest/Streaming/AwaitingToolDecision) | ✅ Primary |
| `active_stream` | Current API response stream | ✅ Primary |
| `streaming_*` | Accumulated content | ✅ Primary |

## Problems with Current Approach

### 1. Manual Synchronization
State must be updated in multiple places, easy to miss one:
- `input_mode` is set in 6+ different locations
- Each location must know the correct value based on context

### 2. Implicit Invariants
The relationship between `input_mode` and `pending_approval` is implicit:
- `ToolApproval` should mean `pending_approval.is_some()`
- Nothing enforces this at the type level

### 3. Temporal Coupling
State updates must happen in the right order:
- Take responder, THEN update mode
- Easy to add code between these that breaks assumptions

### 4. Debugging Difficulty
When state gets out of sync, it's hard to trace:
- Which code path set `input_mode`?
- When was `pending_approval` consumed?

## Proposed Solutions

### Option A: Combine Correlated State (Recommended)

Put the responder inside the enum:

```rust
enum InputMode {
    Normal,
    Streaming,
    ToolApproval {
        responder: oneshot::Sender<ToolDecision>,
    },
}
```

**Pros:**
- Can't be in `ToolApproval` without responder
- Consuming responder forces state transition
- Invalid states unrepresentable

**Cons:**
- Moderate refactor
- `InputMode` no longer `Copy`

**Historical note:** This was previously implemented but removed because the responder appeared to "disappear." The root cause was actually a cancel-safety bug on the *receiver* side (see below). Now that the receiver is cancel-safe, this approach is viable again.

### Option B: Derive State Instead of Storing

```rust
impl App {
    fn input_mode(&self) -> InputMode {
        if self.pending_approval.is_some() {
            InputMode::ToolApproval
        } else if self.is_streaming() {
            InputMode::Streaming
        } else {
            InputMode::Normal
        }
    }
    
    fn is_streaming(&self) -> bool {
        // Check agent state, tool executor state, etc.
    }
}
```

**Pros:**
- Always consistent with source of truth
- No synchronization needed

**Cons:**
- More expensive (checks on every call)
- `is_streaming()` needs access to agent state (async mutex)

### Option C: State Machine with Explicit Transitions

```rust
enum AppState {
    Idle,
    Streaming { agent_id: AgentId },
    AwaitingApproval { 
        agent_id: AgentId,
        responder: oneshot::Sender<ToolDecision>,
    },
    ToolExecuting { agent_id: AgentId },
}

impl AppState {
    fn transition(&mut self, event: StateEvent) -> Result<(), InvalidTransition> {
        // Enforce valid transitions
    }
}
```

**Pros:**
- Explicit state machine
- Invalid transitions are errors
- Easy to log/debug transitions

**Cons:**
- Larger refactor
- More complex API

## Related Issues

### Cancel-Safety in tokio::select!

The tool executor had a bug where `oneshot::Receiver.await` was used directly in the select loop. When cancelled, the receiver was dropped, losing the response channel.

**The bug manifested as the responder "disappearing":**

1. Responder (sender) was stored in `InputMode::ToolApproval` or `App.pending_approval`
2. Receiver was in `ToolExecutor.active.pending_approval`
3. Old code: `let rx = active.pending_approval.take()?; rx.await`
4. When `select!` cancelled `tool_executor.next()`, the receiver was dropped
5. Sending approval would fail - looked like the responder vanished

**The fix (commit `2906a65`):** Non-blocking polling that doesn't take ownership:

```rust
// Old (cancel-unsafe) - receiver taken, lost on cancel
let rx = active.pending_approval.take()?;
rx.await

// New (cancel-safe) - receiver borrowed, stays in place
let rx = active.pending_approval.as_mut()?;
let poll_result = Pin::new(rx).poll(&mut cx);
match poll_result {
    Poll::Ready(Ok(decision)) => {
        active.pending_approval = None;  // Only clear after receiving
        // handle decision
    },
    Poll::Pending => None,  // Still waiting, receiver preserved
}
```

**Key insight:** The responder (sender) was never the problem - it was the receiver being dropped that made sends fail. With the receiver now cancel-safe, Option A (responder inside `InputMode`) is viable again.

### Redundant State After Fix

Line 974 now sets `input_mode = Streaming` redundantly (already set in `decide_pending_tool()`). Harmless but indicates the scattered nature of state management.

## Action Items

1. **Short term (done):** Fix keystroke dropping bug by setting `input_mode` when responder is consumed

2. **Medium term:** Implement Option A - combine `InputMode::ToolApproval` with responder

3. **Long term:** Consider Option C for more complex state (multiple agents, parallel tools)

4. **Documentation:** Add comments explaining state invariants until refactored

## Files Involved

| File | Relevant State |
|------|----------------|
| `src/app.rs` | `input_mode`, `pending_approval`, main event loop |
| `src/tools/exec.rs` | `active`, `pending_approval` (receiver), `pending_effect` |
| `src/llm/agent.rs` | `state`, `active_stream` |
| `src/llm/registry.rs` | Agent aggregation |

## References

- [Tokio select! documentation](https://tokio.rs/tokio/tutorial/select) - cancel safety
- [Making Invalid States Unrepresentable](https://blog.janestreet.com/effective-ml-revisited/) - type-driven design
