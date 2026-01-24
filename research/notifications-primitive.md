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

<notification source="file_watcher">
src/lib.rs was modified externally
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
fn submit_tool_result_with_notifications(
    &mut self,
    call_id: &str,
    result: &str,
    notifications: &[Notification],
) {
    let content = if notifications.is_empty() {
        result.to_string()
    } else {
        let notif_xml = notifications.iter()
            .map(|n| format!(
                "<notification source=\"{:?}\">\n{}\n</notification>",
                n.source, n.to_message()
            ))
            .collect::<Vec<_>>()
            .join("\n");

        format!("{result}\n\n{notif_xml}")
    };

    self.messages.push(ChatMessage::tool_response(call_id, &content));
}
```

### System Prompt Addition

```
You may see <notification> tags in tool results. These are external events
(file changes, background task completions, etc.) that occurred while you
were working. Consider them when deciding your next action.
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

**Use Approach A** (Tool Result Augmentation):
- Proven in production
- Simplest implementation
- No schema changes

**Consider Approach B** if:
- Notifications need distinct transcript entries
- Agent should explicitly acknowledge notifications

---

## Injection Timing

The natural injection point is **after any tool completes**:

```rust
ToolEvent::Completed { agent_id, call_id, content } => {
    // Drain pending notifications
    let notifications = self.notification_manager.drain();

    // Submit result with notifications appended
    agent.submit_tool_result_with_notifications(
        &call_id,
        &content,
        &notifications,
    );
}
```

This ensures:
- Notifications arrive between tool calls (natural pause point)
- Agent sees them before deciding next action
- No interruption of streaming or tool execution

---

## Handling Multiple Queued Notifications

When several notifications arrive before a tool completes, we need a strategy for injection.

### Option 1: Append All

Simply append all pending notifications to the tool result:

```
ToolResult(id="edit_1"): "File updated successfully

<notification source="file_watcher">
src/lib.rs modified externally
</notification>

<notification source="file_watcher">
src/main.rs modified externally
</notification>

<notification source="background_task">
Build completed: 2 warnings
</notification>"
```

**Pros**: Simple, complete information
**Cons**: Can bloat tool results, token cost scales linearly

### Option 2: Batch into Single Block

Group notifications into one XML block:

```
ToolResult(id="edit_1"): "File updated successfully

<notifications count="3">
- [file_watcher] src/lib.rs modified externally
- [file_watcher] src/main.rs modified externally
- [background_task] Build completed: 2 warnings
</notifications>"
```

**Pros**: Compact, clear count signals "catch up"
**Cons**: Less structured for agent parsing

### Option 3: Coalesce by Source

Merge similar notifications:

```
ToolResult(id="edit_1"): "File updated successfully

<notification source="file_watcher">
Multiple files modified: src/lib.rs, src/main.rs
</notification>

<notification source="background_task">
Build completed: 2 warnings
</notification>"
```

**Pros**: Reduces noise from rapid file changes
**Cons**: Loses individual event detail

### Option 4: Cap with Overflow Indicator

Limit injected notifications, indicate overflow:

```
ToolResult(id="edit_1"): "File updated successfully

<notifications showing="3" total="7">
- [file_watcher] src/lib.rs modified
- [file_watcher] src/main.rs modified
- [background_task] Build completed
(4 more notifications pending)
</notifications>"
```

**Pros**: Bounds token cost, agent knows there's more
**Cons**: Agent may miss important notifications

### Recommendation

Combine approaches:

1. **Coalesce** rapid same-source notifications (e.g., file watcher debounce)
2. **Batch** into single `<notifications>` block with count
3. **Cap** at reasonable limit (e.g., 5-10) with overflow indicator
4. **Prioritize** if capping - show higher priority first

```rust
fn format_notifications(notifications: &[Notification], max: usize) -> String {
    // Coalesce same-source notifications within time window
    let coalesced = coalesce_by_source(notifications);

    let total = coalesced.len();
    let showing: Vec<_> = coalesced.into_iter().take(max).collect();

    let mut result = format!("<notifications count=\"{}\">", total);
    for n in &showing {
        result.push_str(&format!("\n- [{}] {}", n.source, n.message));
    }
    if total > max {
        result.push_str(&format!("\n({} more pending)", total - max));
    }
    result.push_str("\n</notifications>");
    result
}
```

---

## Open Questions

1. **Activation modes**: Should some notifications auto-trigger agent response vs. passive accumulation?
2. **Coalescing**: How to batch rapid file changes?
3. **Transcript representation**: Should notifications appear as a distinct block type?
4. **Rate limiting**: Prevent notification storms from overwhelming context?
