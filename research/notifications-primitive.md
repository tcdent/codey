# Notifications Primitive

## Problem Statement

The current message flow is rigid:
1. User sends a message
2. Agent responds (streaming + tool calls)
3. Compaction can inject summaries (special case)

External events (file changes, background task completion, IDE diagnostics) have no way to enter the agent's context mid-turn. The message queue is blocked while the agent is working:

```rust
Some(request) = async { self.message_queue.pop_front() },
    if self.input_mode == InputMode::Normal => {  // BLOCKED during agent turn
    self.handle_message(request).await?;
}
```

We need a way to inject **Notifications** into the agent's context without waiting for the turn to complete.

---

## Decisions

### Approach: Tool Result Augmentation

We will use **Tool Result Augmentation** (Approach A below). This is the pattern Anthropic uses in Claude Code with `<system-reminder>` tags. It's proven, simple, and requires no API changes.

### Unified Notification Queue

All user input flows through a single notification queue:

```
User Input
    │
    ▼
┌──────────────────┐
│ Parse input      │
└──────────────────┘
    │
    ├─► Message ────► Notification::Message(content)
    │
    └─► /command ───► Notification::Command { name, turn_id }
    │
    ▼
pending_notifications queue
```

Drain behavior depends on notification type and agent state:

```
pending_notifications
        │
        ▼
┌───────────────────────────────┐
│ Agent idle (Normal mode)?     │
└───────────────────────────────┘
        │
    ┌───┴───┐
    ▼       ▼
   YES      NO (streaming)
    │       │
    ▼       ▼
  Drain   Tool completes?
  all     │
    │     ├─► YES: Drain injectable
    │     │        (Message, BackgroundTask)
    │     │        → inject as XML
    │     │
    │     └─► NO: Keep queued
    ▼
  Handle:
  - Message → send to agent
  - Command → execute
  - BackgroundTask → send to agent
```

### Notification Types

`Notification` is an enum where each variant carries its own data:

```rust
pub enum Notification {
    /// User message to send to agent
    Message(String),
    
    /// Slash command to execute  
    Command {
        name: String,
        turn_id: usize,
    },
    
    /// Background task completed
    BackgroundTask {
        label: String,
        result: String,
    },
}
```

**Injectable vs Deferred:**
- `Message` and `BackgroundTask` can be injected into tool results mid-turn
- `Command` is always deferred until agent is idle (commands execute in app context, not agent context)

```rust
impl Notification {
    fn can_interrupt(&self) -> bool {
        match self {
            Notification::Message(_) | Notification::BackgroundTask { .. } => true,
            Notification::Command { .. } => false,
        }
    }
}
```

### Simplifications

- **No count**: Don't include notification counts
- **No cap**: Include all pending notifications
- **No coalescing**: Defer to the future if it becomes a problem
- **Simple XML**: Just wrap in `<notification source="...">` and append

### Transcript Representation

`NotificationBlock` is **ephemeral** - rendered for display but not persisted:

- User sees the interruption happened in the UI
- Content actually lives in the tool result (which is persisted)
- No need to reconstruct notifications when loading a saved conversation
- Same pattern as sub-agent tool blocks (rendered but not saved)

Example rendering:
```
┌─ shell ─────────────────────────────────
│ cargo build
│ ✓ Compiled successfully
└─────────────────────────────────────────

┌─ notification (user) ────────────────────
│ actually wait, try a different approach
└─────────────────────────────────────────

┌─ edit_file ─────────────────────────────
│ ...
└─────────────────────────────────────────
```

---

## Key Insight: Tool Results Are Unstructured Text

Tool definitions in the Anthropic API use JSON schemas for structured input:
```json
{
  "name": "Edit",
  "parameters": { "type": "object", "properties": { ... } }
}
```

But tool **results** are just text content:
```json
{
  "type": "tool_result",
  "tool_use_id": "toolu_abc123",
  "content": "File updated successfully."
}
```

This means we can append arbitrary content to tool results. The model interprets it semantically based on formatting (like XML tags).

---

## Approach A: Tool Result Augmentation (Recommended)

**Observed in**: Claude Code's `<system-reminder>` pattern for mid-turn user messages.

Append notification content directly to tool results using XML delimiters. Same `call_id`, just richer content.

### How It Works

```
Assistant: [tool_call id="edit_1" name="Edit"]
ToolResult(id="edit_1"): "File updated successfully

<notification source="user">
actually wait, try a different approach
</notification>"
```

The agent sees this as a single tool result and interprets the XML naturally.

### Observed Example (Claude Code)

When a user sends a message while the agent is mid-turn executing tools:

```
ToolResult(id="edit_1"): "<error>String not found...</error>

<system-reminder>
The user sent the following message:
what about this other approach?

Please address this message and continue with your tasks.
</system-reminder>"
```

The notification is concatenated to the tool result. The agent incorporates it without needing a separate turn.

### Implementation

```rust
impl Notification {
    /// Format as XML for injection into tool results
    /// Returns None for notifications that shouldn't be injected (e.g., Commands)
    fn to_xml(&self) -> Option<String> {
        match self {
            Notification::Message(content) => Some(format!(
                "<notification source=\"user\">\n{}\n</notification>",
                content
            )),
            Notification::BackgroundTask { label, result } => Some(format!(
                "<notification source=\"background_task\" label=\"{}\">\n{}\n</notification>",
                label, result
            )),
            Notification::Command { .. } => None,
        }
    }
}

// In ToolEvent::Completed handler:
let injectable: Vec<_> = self.pending_notifications
    .drain(..)
    .filter(|n| n.can_interrupt())
    .collect();

let content = if injectable.is_empty() {
    content
} else {
    let xml = injectable.iter()
        .filter_map(|n| n.to_xml())
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{}\n\n{}", content, xml)
};

agent.submit_tool_result(&call_id, content);
```
```

### System Prompt Addition

```
You may see <notification> tags in tool results. These are messages from the user
or background task completions that occurred while you were working. Consider them 
when deciding your next action.
```

### Why This Works

1. **API compatible**: Tool results are unstructured text
2. **No ID management**: Reuses existing tool call ID
3. **Proven**: Claude Code uses this pattern in production
4. **Clear boundaries**: XML delimits tool output vs notification
5. **Zero overhead**: Just string formatting

---

## Approach B: Synthetic Tool Injection (Alternative)

For cases where notifications should appear as distinct events in the message history rather than embedded in tool results.

### How It Works

Inject a synthetic tool call + result pair:

```
Assistant: [tool_call id="edit_1" name="Edit"]
ToolResult(id="edit_1"): "Success"
Assistant: [tool_call id="notif_1" name="_system_notification"]  ← Synthetic
ToolResult(id="notif_1"): "File src/lib.rs modified externally"  ← Synthetic
```

### Implementation

```rust
pub const NOTIFICATION_TOOL: &str = "_system_notification";

fn inject_notification(&mut self, notification: Notification) {
    let call_id = format!("notif_{}", Uuid::new_v4());

    // Synthetic tool call
    self.messages.push(ChatMessage {
        role: ChatRole::Assistant,
        content: MessageContent::default()
            .append(ContentPart::ToolCall(GenaiToolCall {
                id: call_id.clone(),
                name: NOTIFICATION_TOOL.to_string(),
                arguments: "{}".to_string(),
            })),
        options: None,
    });

    // Synthetic tool result
    self.messages.push(ChatMessage::tool_response(
        &call_id,
        &notification.to_message(),
    ));
}
```

Would also need a tool definition:
```rust
Tool {
    name: "_system_notification",
    description: "System-generated notifications. You do not call this tool;
                  the system uses it to inform you of external events.",
}
```

---

## Comparison

| Aspect | Tool Result Augmentation | Synthetic Tool Injection |
|--------|-------------------------|-------------------------|
| Complexity | Low | Medium |
| API changes | None | Tool definition needed |
| Message count | Same | +2 per notification |
| Transcript clarity | Embedded in tool | Distinct events |
| Proven | Yes (Claude Code) | Theoretical |
| Token overhead | Minimal | Higher |

---

## Recommendation

**Decision: Use Approach A** (Tool Result Augmentation):
- Proven in production (Claude Code uses this)
- Simplest implementation
- No schema changes

---

## Injection Timing

The natural injection point is **after any tool completes**, but only for injectable notifications:

```rust
ToolEvent::Completed { agent_id, call_id, content } => {
    // Drain only injectable notifications (Message, BackgroundTask)
    // Commands stay queued for when agent becomes idle
    let (injectable, deferred): (Vec<_>, Vec<_>) = self.pending_notifications
        .drain(..)
        .partition(|n| n.can_interrupt());
    
    // Put deferred notifications back
    self.pending_notifications.extend(deferred);
    
    // Append injectable notifications as XML
    let content = if injectable.is_empty() {
        content
    } else {
        let xml = injectable.iter()
            .filter_map(|n| n.to_xml())
            .collect::<Vec<_>>()
            .join("\n\n");
        format!("{}\n\n{}", content, xml)
    };
    
    agent.submit_tool_result(&call_id, content);
}
```

This ensures:
- Notifications arrive between tool calls (natural pause point)
- Agent sees them before deciding next action
- No interruption of streaming or tool execution

---

## Handling Multiple Queued Notifications

**Decision**: Keep it simple - append all notifications as separate XML blocks:

```
ToolResult(id="edit_1"): "File updated successfully

<notification source="user">
actually wait, try a different approach
</notification>

<notification source="background_task" label="build-check">
Build completed: 2 warnings
</notification>"
```

No counting, no capping, no coalescing. If this becomes a problem (e.g., file watcher storms), we can add coalescing later.

---

## Implementation Progress

### ✅ Completed

**Ephemeral Block Support** (`src/transcript.rs`):
- Added `is_ephemeral()` method to `Block` trait (default `false`)
- `NotificationBlock` struct that returns `is_ephemeral() -> true`
- Custom `Serialize` for `Turn` that filters out ephemeral blocks
- Notification rendering with yellow styling and ⚡ icon

### 🔲 In Progress

**Data Structures** (`src/app.rs`):
- `Notification` enum with variants: `Message`, `Command`, `BackgroundTask`
- `can_interrupt()` method to determine if notification can be injected mid-turn
- `to_xml()` method for injectable notifications
- `pending_notifications: VecDeque<Notification>` on `App` struct

### 🔲 Remaining

**Wiring** (in `src/app.rs`):
1. Modify `queue_message()` (line ~586) - always create Notification (Message or Command)
2. Add event loop branch to drain notifications when idle (InputMode::Normal)
3. Modify `ToolEvent::Completed` handler (line ~1135) - drain injectable notifications and append XML
4. Wire `BackgroundTask` notifications when background tasks complete

**System Prompt**:
- Add explanation of `<notification>` tags (see "System Prompt Addition" section above)

**Testing**:
- Test notification injection into tool results
- Test ephemeral block filtering during serialization
- Test `queue_message()` routing (Message vs Command)
- Test Command deferral (not injected, waits for idle)

---

## Open Questions

1. ~~**Activation modes**~~: Decided - unified notification queue (see Decisions above)
2. **Coalescing**: Deferred - solve if it becomes a problem
3. ~~**Transcript representation**~~: Decided - ephemeral `NotificationBlock` (see Decisions above)
4. **Rate limiting**: Deferred - solve if it becomes a problem
5. ~~**Command handling**~~: Decided - Commands always deferred until idle
