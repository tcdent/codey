# Notifications Primitive

## Problem Statement

The current system has a rigid message flow:
1. User sends a message (terminal input)
2. Agent responds (streaming + tool calls)
3. Compaction can inject summaries (special case)

This model breaks down when we want external events to influence the agent:
- File system changes (watched files modified)
- Background task completion (sub-agent finished, build completed)
- IDE events (cursor moved, file opened, diagnostics changed)
- Timer/scheduled events (reminder, timeout)
- External webhooks (CI status, PR review)

Currently, these events either:
- Get ignored entirely
- Require user to manually ask about them
- Use hacky workarounds (fake user messages)

We need a first-class primitive for **Notifications** - events from outside the user-agent conversation that can be injected into context and optionally activate the agent.

---

## Current Architecture Analysis

### Message Flow Today

```
Terminal Event (user input)
    ↓
MessageRequest::User(content, turn_id)
    ↓
message_queue.push_back(...)
    ↓
handle_message() → agent.send_request()
    ↓
Agent streams response
```

### Context Management

The agent maintains `messages: Vec<ChatMessage>` which maps to LLM API format:
- `ChatRole::System` - system prompt (first message)
- `ChatRole::User` - user messages
- `ChatRole::Assistant` - agent responses (text, tool calls)

The transcript uses `Turn` with `Role::User | Role::Assistant | Role::System` and polymorphic `Block` content.

### Existing "Injection" Patterns

1. **Compaction** (`MessageRequest::Compaction`)
   - Triggered by token threshold
   - Agent responds in `RequestMode::Compaction`
   - Result replaces context via `reset_with_summary()`
   - Appears as `CompactionBlock` in transcript

2. **IDE Events** (`handle_ide_event()`)
   - Currently just updates `selected_text` state
   - No injection into agent context
   - Agent only sees selection if it requests it

3. **Tool Results**
   - Injected as tool response messages
   - Part of the normal request/response flow

---

## Design: Notifications Primitive

### Core Concept

A **Notification** is an event from outside the conversation that:
1. Has a **source** (what system generated it)
2. Has a **priority** (how urgent/important)
3. Has **content** (what happened)
4. Has an optional **action** (what the agent might do)

Notifications differ from user messages in that they:
- May not require immediate response
- Can be batched/coalesced
- Have semantic meaning (type-based handling)
- Can be filtered/prioritized

### Notification Structure

```rust
pub struct Notification {
    pub id: Uuid,
    pub source: NotificationSource,
    pub priority: NotificationPriority,
    pub content: NotificationContent,
    pub timestamp: DateTime<Utc>,
    pub requires_response: bool,
}

pub enum NotificationSource {
    FileSystem,       // File watcher events
    BackgroundTask,   // Sub-agent, build, test completion
    IDE,              // Editor events (diagnostics, navigation)
    Timer,            // Scheduled/timeout events
    External,         // Webhooks, CI, etc.
    System,           // Internal system events
}

pub enum NotificationPriority {
    Low,              // Informational, can be batched
    Normal,           // Standard priority
    High,             // Should interrupt current work
    Critical,         // Must be handled immediately
}

pub enum NotificationContent {
    FileChanged { path: PathBuf, change_type: ChangeType },
    TaskCompleted { task_id: String, result: String },
    DiagnosticsUpdated { uri: String, diagnostics: Vec<Diagnostic> },
    TimerFired { name: String },
    Custom { event_type: String, payload: serde_json::Value },
}
```

### Transcript Integration

New block type for notifications:

```rust
pub struct NotificationBlock {
    pub notification: Notification,
    pub status: Status,
    pub acknowledged: bool,
}

impl Block for NotificationBlock {
    fn kind(&self) -> BlockType { BlockType::Notification }
    // ...
}

pub enum BlockType {
    Text,
    Thinking,
    Tool,
    Compaction,
    Notification,  // NEW
}
```

### Message Queue Integration

Extend the message request enum:

```rust
enum MessageRequest {
    User(String, usize),
    Compaction,
    Command(String, usize),
    Notification(Notification),  // NEW
}
```

### Agent Context Injection

Notifications need to appear in the agent's context. Options:

**Option A: As User Messages (Simple)**
```rust
// In agent.send_request() or restore_from_transcript()
ChatMessage::user(format!(
    "[NOTIFICATION from {source}]: {content}"
))
```

Pros: Works with existing API, no special handling
Cons: Pollutes user message stream, awkward formatting

**Option B: As System Messages (Semantic)**
```rust
// Inject after system prompt, before conversation
ChatMessage::system(format!(
    "Notification ({source}, {priority}): {content}"
))
```

Pros: Semantically correct, separate from user input
Cons: Multiple system messages may confuse models

**Option C: Aggregated Context Block (Recommended)**
```rust
// Single "notifications context" injected before user's message
let notifications_context = format!(
    "<notifications>\n{}\n</notifications>",
    pending_notifications.iter()
        .map(|n| format!("- [{:?}] {}", n.source, n.content))
        .collect::<Vec<_>>()
        .join("\n")
);

// Prepend to user message or inject as separate user message
ChatMessage::user(notifications_context)
```

Pros: Batched, clear delineation, efficient token use
Cons: Still a "fake" user message

**Option D: Extended Thinking Prompt (Cleanest)**
```rust
// Add to system prompt dynamically
let dynamic_context = format!(
    "\n\n## Active Notifications\n{notifications}\n\n\
     Consider these when responding. Not all require action."
);
```

Pros: Natural integration, doesn't pollute conversation
Cons: Requires system prompt regeneration

---

## Activation Modes

Notifications can trigger different behaviors:

### 1. Passive (Accumulate)
Notifications queue up silently. Agent sees them on next user message.

```
User: "fix the bug"
[Notifications silently accumulated]
Agent: (sees notifications in context) "I notice the file changed..."
```

### 2. Prompt (Notify User)
Show notification to user, let them decide to activate agent.

```
┌─────────────────────────────────────────────────┐
│ [!] Build completed with 3 warnings             │
│     Press Enter to discuss, Esc to dismiss     │
└─────────────────────────────────────────────────┘
```

### 3. Active (Auto-Activate)
High-priority notifications automatically trigger agent response.

```
[Critical: Tests failing after file save]
    ↓
Agent automatically activates
    ↓
"I see the tests are now failing. Let me investigate..."
```

### Configuration

```toml
[notifications]
# Global enable/disable
enabled = true

# Per-source configuration
[notifications.file_system]
enabled = true
activation = "passive"
debounce_ms = 500

[notifications.background_task]
enabled = true
activation = "prompt"

[notifications.ide.diagnostics]
enabled = true
activation = "active"
min_priority = "high"
```

---

## Event Flow

### Passive Flow
```
External Event
    ↓
NotificationSource generates Notification
    ↓
notification_queue.push(notification)
    ↓
[User sends message]
    ↓
handle_message():
    - Drain notification_queue
    - Inject into context
    - Send to agent
    ↓
Agent responds (aware of notifications)
```

### Active Flow
```
External Event
    ↓
NotificationSource generates Notification (priority: High)
    ↓
notification_queue.push(notification)
    ↓
check_auto_activation():
    - Priority >= threshold?
    - Agent idle?
    - Activation mode == "active"?
    ↓
MessageRequest::Notification(notification)
    ↓
handle_message():
    - Create synthetic context
    - Begin assistant turn
    - Agent responds proactively
```

---

## Implementation Components

### 1. NotificationManager

Central hub for notification handling:

```rust
pub struct NotificationManager {
    queue: VecDeque<Notification>,
    config: NotificationConfig,
    coalescing_window: Duration,
}

impl NotificationManager {
    pub fn push(&mut self, notification: Notification);
    pub fn drain(&mut self) -> Vec<Notification>;
    pub fn drain_for_context(&mut self) -> Option<String>;
    pub fn has_pending(&self) -> bool;
    pub fn should_auto_activate(&self) -> bool;
}
```

### 2. NotificationSource Trait

Allow pluggable notification sources:

```rust
#[async_trait]
pub trait NotificationSource {
    fn source_type(&self) -> NotificationSourceType;
    async fn next(&mut self) -> Option<Notification>;
}

// Implementations
pub struct FileWatcher { ... }
pub struct IdeNotifications { ... }
pub struct BackgroundTaskMonitor { ... }
```

### 3. App Integration

```rust
// In App struct
notification_manager: NotificationManager,
notification_sources: Vec<Box<dyn NotificationSource>>,

// In event loop
loop {
    tokio::select! {
        // ... existing branches ...

        // Notification sources
        Some(notification) = poll_notification_sources() => {
            self.notification_manager.push(notification);
            if self.notification_manager.should_auto_activate() {
                self.message_queue.push_back(
                    MessageRequest::Notification(notification)
                );
            }
        }
    }
}
```

### 4. UI Integration

Status bar indicator for pending notifications:

```
┌─────────────────────────────────────────────────────────┐
│ codey v0.1.0  │  tokens: 12.4k  │  [3 notifications]   │
└─────────────────────────────────────────────────────────┘
```

Notification panel (optional):

```
┌─ Notifications ─────────────────────────────────────────┐
│ [fs] src/main.rs modified                    2s ago    │
│ [bg] Build completed successfully            5s ago    │
│ [ide] 2 new diagnostics in lib.rs           10s ago    │
└─────────────────────────────────────────────────────────┘
```

---

## Comparison with Existing Patterns

| Aspect | User Message | Compaction | Notification |
|--------|--------------|------------|--------------|
| Source | User input | Token threshold | External events |
| Trigger | Explicit | Automatic | Configurable |
| Urgency | Immediate | Delayed | Varies |
| Context impact | Full turn | Replaces context | Injected |
| User visibility | Full | Summary | Optional |
| Agent response | Required | Required | Optional |

---

## Use Cases

### 1. File Watcher Integration
```
User: "I'm going to edit the config file manually"
Agent: "Sure, I'll wait"
[User edits file externally]
[Notification: config.toml modified]
Agent: (on next message, aware of change)
  "I see you updated config.toml. The new timeout value looks good."
```

### 2. Background Task Completion
```
User: "Run the full test suite in the background"
Agent: (spawns background task)
[User continues chatting about other things]
[Notification: test suite completed - 3 failures]
[UI shows notification badge]
User: (presses Enter to discuss)
Agent: "The test suite finished. 3 tests failed in the auth module..."
```

### 3. IDE Diagnostics
```
[Notification: New error in main.rs:45]
[Auto-activation triggered]
Agent: "I notice a new type error appeared on line 45.
        This is likely from the change I just made. Let me fix it."
```

### 4. CI/CD Integration
```
[Notification: PR #123 checks failed]
Agent: "The CI checks failed on your PR. The linting step
        found 2 issues. Would you like me to fix them?"
```

---

## Mid-Stream Injection

### The Current Limitation

Today's event loop has a strict ordering:

```rust
// message_queue only drains when agent is idle
Some(request) = async { self.message_queue.pop_front() },
    if self.input_mode == InputMode::Normal => {  // <-- BLOCKED during agent turn
    self.handle_message(request).await?;
}
```

This means:
- While agent is streaming: no new messages processed
- While agent is thinking: no new messages processed
- While tools execute: no new messages processed
- While awaiting approval: no new messages processed

Notifications must wait until the entire turn completes.

### Why Mid-Stream Injection Matters

Consider these scenarios:

**Scenario 1: Long-Running Tool**
```
Agent: "Let me run the full test suite..."
[Tool executing: 45 seconds]
[File changes detected - user saved a fix]
[Notification queued... waiting... waiting...]
[Tests finish with old code]
Agent: "Tests failed"
[NOW notification delivered - too late!]
```

**Scenario 2: Streaming Response**
```
Agent: (streaming) "Based on my analysis of the codebase..."
[IDE: new diagnostic - type error on line 45]
[Agent continues for 30 more seconds, unaware]
Agent: "...and that's my recommendation"
[NOW notification delivered]
User: "But there's a type error now"
```

**Scenario 3: Multi-Tool Turn**
```
Agent: Calls tool A, then tool B, then tool C
[Between tool A and B: critical notification arrives]
[Agent continues with stale understanding]
```

### Injection Points

Where could we inject notifications mid-stream?

```
Agent Turn Lifecycle:
    │
    ├─► send_request()
    │       │
    │       ├─► LLM streaming begins ──────────────► [Injection Point 1]
    │       │       │                                 Between chunks?
    │       │       ├─► text delta                    Risky: mid-thought
    │       │       ├─► text delta
    │       │       └─► tool_call
    │       │
    │       ├─► Tool execution ────────────────────► [Injection Point 2]
    │       │       │                                 Before tool runs?
    │       │       ├─► awaiting approval
    │       │       ├─► tool running
    │       │       └─► tool complete
    │       │
    │       ├─► submit_tool_result() ──────────────► [Injection Point 3]
    │       │       │                                 With tool result?
    │       │       └─► next iteration
    │       │
    │       └─► Finished
    │
    └─► Turn complete ─────────────────────────────► [Injection Point 4]
                                                      Current behavior
```

### Injection Point Analysis

#### Point 1: During LLM Streaming
**Feasibility**: Very difficult
- Can't modify an in-flight API request
- Would need to cancel and restart with new context
- Loses streaming progress, poor UX

**When useful**: Critical notifications that invalidate current response

#### Point 2: Before Tool Execution
**Feasibility**: Possible
- Tool hasn't run yet
- Could prepend notification context to tool result
- Or cancel tool and restart turn

**Implementation**:
```rust
// In tool execution flow
async fn execute_tool(&mut self, tool_call: ToolCall) -> ToolResult {
    // Check for critical notifications before running
    if let Some(notification) = self.check_critical_notifications() {
        return ToolResult::Interrupted {
            reason: notification,
            should_restart: true,
        };
    }

    // Proceed with tool execution
    self.run_tool(tool_call).await
}
```

#### Point 3: With Tool Result (Recommended)
**Feasibility**: Good
- Natural injection point in the request/response cycle
- Tool result is already being assembled
- Agent will immediately see notification in next iteration

**Implementation**:
```rust
// When building tool result message
fn build_tool_result(&self, call_id: &str, result: &str) -> ChatMessage {
    let notifications = self.notification_manager.drain_for_injection();

    let content = if let Some(notifs) = notifications {
        format!(
            "{result}\n\n<notifications>\n{notifs}\n</notifications>"
        )
    } else {
        result.to_string()
    };

    ChatMessage::tool_response(call_id, content)
}
```

**Pros**:
- Clean integration with existing flow
- Agent sees notification before next action
- Doesn't break streaming or tool execution

**Cons**:
- Notification bundled with unrelated tool result
- May confuse the model

#### Point 4: Turn Boundary (Current)
**Feasibility**: Implemented (current behavior)
**When useful**: Non-urgent notifications, passive mode

### Approach A: Tool Result Augmentation (Recommended)

**Observed in**: Claude Code's background agent system (`<system-reminder>` tags)

The simplest approach: append notification content directly to existing tool results using XML delimiters. No synthetic tool calls, no new message types - just string concatenation with semantic markup.

#### How It Works

Tool results in the Anthropic API are text content, not parsed JSON. This means we can append arbitrary content to them:

```
Assistant: [tool_call id="edit_1" name="Edit"]
ToolResult(id="edit_1"): "File updated successfully

<notification source=\"file_watcher\">
src/lib.rs was modified externally
</notification>"
```

The agent receives this as a single tool result and interprets the XML semantically. Same `call_id` - just richer content.

#### Implementation

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

#### System Prompt Addition

```
You may see <notification> tags in tool results. These are external events
(file changes, background task completions, etc.) that occurred while you
were working. Consider them when deciding your next action.
```

#### Observed Example (Claude Code)

When a user sends a message while the agent is mid-turn, it appears appended to tool results:

```
ToolResult(id="edit_1"): "<error>String not found...</error>

<system-reminder>
The user sent the following message:
what about this other approach?

Please address this message and continue with your tasks.
</system-reminder>"
```

The agent sees this naturally and incorporates the message without needing a separate turn.

#### Why This Works

1. **API compatible**: Tool results are unstructured text - append anything
2. **No ID management**: Reuses existing tool call ID
3. **Proven pattern**: Claude Code uses this for mid-turn user message injection
4. **Clear boundaries**: XML makes tool output vs notification unambiguous
5. **Zero overhead**: Just string formatting

---

### Approach B: Synthetic Tool Injection (Alternative)

For cases where notifications should appear as distinct "events" in the message history.

#### How It Works

Inject a synthetic tool call + result pair:

```
Assistant: [tool_call id="edit_1" name="Edit"]
ToolResult(id="edit_1"): "Success"
Assistant: [tool_call id="notif_1" name="_system_notification"]  ← Synthetic
ToolResult(id="notif_1"): "File src/lib.rs modified externally"  ← Synthetic
```

#### Implementation

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

#### Trade-offs

| Aspect | Tool Result Augmentation | Synthetic Tool Injection |
|--------|-------------------------|-------------------------|
| Complexity | Low | Medium |
| API changes | None | Tool definition needed |
| Message count | Same | +2 per notification |
| Transcript clarity | Embedded in tool | Distinct events |
| Proven | Yes (Claude Code) | Theoretical |
| Token overhead | Minimal | Higher |

---

### Recommendation

**Use Approach A** (Tool Result Augmentation) as primary:
- Proven in production (Claude Code)
- Simplest implementation
- No schema changes needed

**Consider Approach B** if:
- Notifications need distinct transcript entries
- Agent should explicitly acknowledge notifications

---

### Other Considered Approaches

#### Turn Interruption (for critical notifications)

```rust
pub enum TurnInterrupt {
    InjectAndContinue { notification: Notification },
    CancelAndRestart { notification: Notification },
    QueueForNext { notification: Notification },
}

// In agent streaming loop
match self.check_interrupt() {
    Some(TurnInterrupt::CancelAndRestart { notification }) => {
        // Stop current streaming
        self.cancel_current_request();

        // Inject notification into context
        self.inject_notification(notification);

        // Restart the turn
        self.send_request(self.last_prompt, mode);
    }
    Some(TurnInterrupt::InjectAndContinue { notification }) => {
        // Will appear in next tool result
        self.pending_injection = Some(notification);
    }
    None => {
        // Continue normally
    }
}
```

### Streaming Context Window

A more sophisticated approach: maintain a "context window" that can be updated:

```
┌─────────────────────────────────────────────────────────┐
│ System Prompt                                           │
├─────────────────────────────────────────────────────────┤
│ [Dynamic Context Window]          ◄── Can be updated    │
│ - Current notifications                                 │
│ - Recent file changes                                   │
│ - IDE state                                             │
├─────────────────────────────────────────────────────────┤
│ Conversation History                                    │
│ User: ...                                               │
│ Assistant: ...                                          │
├─────────────────────────────────────────────────────────┤
│ Current Turn                                            │
│ User: "fix the bug"                                     │
│ Assistant: (streaming...)                               │
└─────────────────────────────────────────────────────────┘
```

The "Dynamic Context Window" could be:
- Updated between tool calls
- Refreshed on turn restart
- Limited size (token budget)

### API Considerations

Current Anthropic API doesn't support:
- Modifying in-flight requests
- Injecting content mid-stream
- Multiple system messages (cleanly)

Workarounds:
1. **Tool result injection**: Append notification to tool results
2. **Turn restart**: Cancel and re-send with new context
3. **System prompt refresh**: Update system prompt between turns

Future API features that would help:
- Server-sent events for context updates
- Interruptible streaming
- Dynamic system context

### State Machine View

```
                    ┌─────────────────────┐
                    │       IDLE          │
                    │  (accepts messages) │
                    └──────────┬──────────┘
                               │ user message
                               ▼
                    ┌─────────────────────┐
         ┌─────────│     STREAMING       │─────────┐
         │         │  (LLM generating)   │         │
         │         └──────────┬──────────┘         │
         │                    │                    │
    critical              tool_call            finished
    notification              │                    │
         │                    ▼                    │
         │         ┌─────────────────────┐         │
         │         │   TOOL_PENDING      │         │
         │         │  (awaiting tool)    │         │
         │         └──────────┬──────────┘         │
         │                    │                    │
         │    ┌───────────────┼───────────────┐    │
         │    │               │               │    │
         │ critical      tool_done        high │    │
         │ notif             │            notif│    │
         │    │              ▼               │    │
         │    │    ┌─────────────────────┐   │    │
         │    │    │  TOOL_COMPLETE      │   │    │
         │    │    │ (result ready)      │───┘    │
         │    │    └──────────┬──────────┘        │
         │    │               │                   │
         │    │          inject with              │
         │    │          tool result              │
         │    │               │                   │
         │    │               ▼                   │
         │    │    ┌─────────────────────┐        │
         │    └───►│   CONTINUING        │◄───────┘
         │         │  (next iteration)   │
         │         └──────────┬──────────┘
         │                    │
         │                    └──────────┐
         │                               │
         ▼                               ▼
┌─────────────────────┐       ┌─────────────────────┐
│  INTERRUPTED        │       │       IDLE          │
│  (restart turn)     │──────►│  (turn complete)    │
└─────────────────────┘       └─────────────────────┘
```

### Implementation Complexity

| Injection Strategy | Complexity | UX Impact | Use Case |
|-------------------|------------|-----------|----------|
| Turn boundary | Low | None | Default, non-urgent |
| Tool result | Medium | Minimal | High priority |
| Before tool | Medium | Tool cancelled | Critical |
| Mid-stream | High | Response restart | Emergency only |
| Context window | High | Seamless | Ideal future |

### Recommendation

**Phase 1**: Tool result injection for High priority
- Lowest risk
- Natural integration point
- Agent sees notification before next action

**Phase 2**: Before-tool interruption for Critical
- Can cancel unnecessary work
- Restart with fresh context
- Clear UX: "Interrupted by notification"

**Phase 3**: Explore context window approach
- Requires more architectural changes
- Best long-term UX
- May need API evolution

---

## Open Questions

1. **Notification Persistence**
   - Should notifications persist across sessions?
   - Save to transcript vs. separate notification log?

2. **Coalescing Strategy**
   - How to merge rapid file changes?
   - Window-based vs. semantic deduplication?

3. **Priority Inference**
   - Can we auto-detect priority from content?
   - ML-based importance scoring?

4. **Agent Notification Requests**
   - Should agents be able to request notifications?
   - "Notify me when the build finishes"

5. **Notification Actions**
   - Pre-defined actions agents can take?
   - "Acknowledge", "Investigate", "Dismiss"?

6. **Rate Limiting**
   - Prevent notification storms
   - Per-source rate limits?

7. **Context Budget**
   - How many notification tokens to allow?
   - Summarization for old notifications?

---

## Phased Implementation

### Phase 1: Foundation
- [ ] Define `Notification` types and structures
- [ ] Add `NotificationBlock` to transcript
- [ ] Create `NotificationManager` with basic queue
- [ ] Add `MessageRequest::Notification` variant

### Phase 2: Integration
- [ ] Inject notifications into agent context
- [ ] Add passive accumulation mode
- [ ] Status bar notification indicator
- [ ] Configuration options

### Phase 3: Sources
- [ ] File watcher notification source
- [ ] Background task completion notifications
- [ ] IDE diagnostic notifications

### Phase 4: Activation
- [ ] Prompt mode with UI
- [ ] Auto-activation for high priority
- [ ] Notification panel UI

### Phase 5: Advanced
- [ ] Coalescing and deduplication
- [ ] Notification persistence
- [ ] Agent-requested notifications
- [ ] External webhook integration

---

## Relationship to Sub-Agent Architecture

The Notifications primitive complements the sub-agent work:

- **Sub-agents** spawn and run to completion, returning results as tool output
- **Notifications** signal when background work completes
- Combined: Sub-agent spawns in background → Notification when done → Agent can discuss results

```
Primary Agent
    │
    ├─► spawn_background_task("run tests")
    │       └──────────────────────► Background Runner
    │                                       │
    ├─► continues conversation              ├─► running...
    │   with user                           │
    │                                       └─► complete
    │                                       │
    ◄───────── Notification ────────────────┘
    │   "Tests completed: 2 failures"
    │
    ├─► [Prompt mode] User sees notification
    │   OR
    ├─► [Active mode] Agent auto-responds
```

This creates a complete async work model where:
1. Sub-agents handle the actual background work
2. Notifications handle the signaling/awareness
3. The primary conversation remains responsive
