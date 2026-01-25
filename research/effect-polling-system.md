# Effect Polling System

## Problem Statement

The current effect handling has two issues:

1. **IDE preview deadlock**: When a sub-agent has an IDE preview open and another agent tries to show a preview, the `apply_effect` function blocks in a `while` loop waiting for the IDE slot. This deadlocks because it blocks the same event loop that would process the approval to release the slot.

2. **Duplicate patterns**: `pending_approvals` and `pending_ide_slots` are similar queue structures that track responders waiting for some condition. This suggests a more generic abstraction.

## Current Architecture

### Effects
Effects are delegated operations that the tool pipeline sends to the app layer:
```rust
enum Effect {
    IdeShowPreview { preview: ToolPreview },
    IdeShowDiffPreview { path: PathBuf, edits: Vec<Edit> },
    IdeClosePreview,
    // ... etc
}
```

### Current Flow
1. Tool handler returns `Step::Delegate(Effect::...)`
2. Executor emits `ToolEvent::Delegate { effect, responder, ... }`
3. App calls `apply_effect()` which executes synchronously
4. App immediately sends result via `responder.send(result)`

### Problem with IDE Preview
```rust
// This blocks the event loop!
while !ide.try_claim_preview().await? {
    tokio::time::sleep(Duration::from_secs(1)).await;
}
```

### Approvals (Separate Pattern)
Approvals use a different flow:
1. Tool handler returns `Step::AwaitApproval`
2. Executor emits `ToolEvent::AwaitingApproval { responder, ... }`
3. App queues `(tool_call, responder)` in `pending_approvals`
4. User makes decision → app sends via stored responder

## Proposed Design

### Unified Polling Model

All effects (immediate or deferred) go through the same polling mechanism:

```rust
/// Exclusive resources that only one effect can hold at a time
enum Resource {
    ApprovalSlot,  // Only one approval shown at a time
    IdePreview,    // Only one preview shown at a time
}

/// Result of polling an effect
enum EffectPoll {
    Ready(Result<Option<String>>),  // Effect completed
    Pending,                         // Still waiting, poll again
}

/// An effect waiting to be executed
struct PendingEffect {
    call_id: String,
    agent_id: AgentId,
    effect: Effect,
    responder: oneshot::Sender<EffectResult>,
}
```

### Flow

1. `ToolEvent::Delegate` arrives → create `PendingEffect`, add to queue
2. Main loop polls `pending_effects` each iteration
3. For each pending effect:
   - Check if it needs an exclusive resource
   - If so, only poll if it's first in line for that resource
   - Call `apply_effect(&effect)` → returns `EffectPoll`
   - If `Ready(result)` → send via responder, remove from queue
   - If `Pending` → keep in queue, poll again next iteration

### Resource Exclusivity

Some effects need exclusive access to a resource:

```rust
impl Effect {
    fn resource(&self) -> Option<Resource> {
        match self {
            Effect::AwaitApproval(_) => Some(Resource::ApprovalSlot),
            Effect::IdeShowPreview { .. } |
            Effect::IdeShowDiffPreview { .. } => Some(Resource::IdePreview),
            _ => None,
        }
    }
}
```

When polling:
- Effects with `None` resource → always poll (immediate)
- Effects with `Some(resource)` → only poll if first in queue needing that resource

### apply_effect Changes

```rust
async fn apply_effect(&mut self, effect: &Effect) -> EffectPoll {
    match effect {
        // Immediate effects - just execute
        Effect::IdeReloadBuffer { path } => {
            if let Some(ide) = &self.ide {
                match ide.reload_buffer(&path.to_string_lossy()).await {
                    Ok(_) => EffectPoll::Ready(Ok(None)),
                    Err(e) => EffectPoll::Ready(Err(e)),
                }
            } else {
                EffectPoll::Ready(Ok(None))
            }
        },
        
        // Deferred effects - check condition
        Effect::IdeShowDiffPreview { path, edits } => {
            if let Some(ide) = &self.ide {
                // Resource check happens in polling loop, not here
                // If we're being polled, we have the resource
                match ide.show_diff_preview(&path.to_string_lossy(), edits).await {
                    Ok(_) => EffectPoll::Ready(Ok(None)),
                    Err(e) => EffectPoll::Ready(Err(e)),
                }
            } else {
                EffectPoll::Ready(Ok(None))
            }
        },
        
        // ... etc
    }
}
```

### Polling Loop (in main event loop)

```rust
fn poll_pending_effects(&mut self) {
    let mut completed = vec![];
    
    // Track which resources have an active effect
    let mut active_resources: HashSet<Resource> = HashSet::new();
    
    for (idx, pending) in self.pending_effects.iter_mut().enumerate() {
        // Check resource exclusivity
        if let Some(resource) = pending.effect.resource() {
            if active_resources.contains(&resource) {
                continue; // Not our turn yet
            }
            active_resources.insert(resource);
        }
        
        // Poll the effect
        match self.apply_effect(&pending.effect).await {
            EffectPoll::Ready(result) => {
                let effect_result = result.map_err(|e| e.to_string());
                let _ = pending.responder.send(effect_result);
                completed.push(idx);
            },
            EffectPoll::Pending => {
                // Keep in queue
            },
        }
    }
    
    // Remove completed effects (in reverse order to preserve indices)
    for idx in completed.into_iter().rev() {
        self.pending_effects.remove(idx);
    }
}
```

## Converting Approvals to Effects

### Phase 2: Unify Approvals

Add approval as an effect type:

```rust
enum Effect {
    // ... existing effects ...
    
    /// Request user approval for a tool call
    AwaitApproval { tool_call: ToolCall },
}
```

The approval flow becomes:
1. Handler returns `Step::Delegate(Effect::AwaitApproval { tool_call })`
2. Effect goes into `pending_effects` queue
3. Polling loop checks if it's first for `Resource::ApprovalSlot`
4. If first, show approval UI
5. User decision stored in `approval_decisions: HashMap<String, ToolDecision>`
6. Next poll: check if decision exists → `Ready` or `Pending`

```rust
Effect::AwaitApproval { tool_call } => {
    // Check if user has made a decision
    if let Some(decision) = self.approval_decisions.remove(&call_id) {
        match decision {
            ToolDecision::Approve => EffectPoll::Ready(Ok(None)),
            ToolDecision::Deny => EffectPoll::Ready(Err(anyhow!("Denied by user"))),
            // ... handle other decisions
        }
    } else {
        // Still waiting for user
        EffectPoll::Pending
    }
}
```

### Benefits of Unification

1. **Single code path**: All deferred operations use the same pattern
2. **Easier reasoning**: No special cases for approvals vs IDE slots
3. **Extensible**: Easy to add new resource types or deferrable effects
4. **No deadlocks**: Polling model never blocks the event loop

## Implementation Plan

### Phase 1: IDE Effects (Current Focus)
1. Add `PendingEffect`, `EffectPoll`, `Resource` types
2. Add `pending_effects: VecDeque<PendingEffect>` to App
3. Change `ToolEvent::Delegate` handling to queue effects
4. Implement `poll_pending_effects()` in main loop
5. Update `apply_effect` to return `EffectPoll`
6. Remove `IdeClaimSlot` effect (resource check is implicit)
7. Remove blocking while loops from IDE effects

### Phase 2: Unify Approvals
1. Add `Effect::AwaitApproval { tool_call: ToolCall }`
2. Add `approval_decisions: HashMap<String, ToolDecision>` to App
3. Convert `Step::AwaitApproval` to use `Step::Delegate(Effect::AwaitApproval {...})`
4. Remove `ToolEvent::AwaitingApproval`
5. Remove `pending_approvals` field
6. Update approval UI logic to work with polling model

## Open Questions

1. **Polling frequency**: How often should we poll? Every main loop iteration, or with some throttling?

2. **Error handling**: If an effect errors, should we retry or fail immediately?

3. **Cancellation**: What happens if a tool is cancelled while its effect is pending?

4. **Ordering**: Within a resource, should effects be strictly FIFO, or could priority matter?

5. **Multiple resources**: Could an effect need multiple resources? (Probably not, but worth considering)

## Related Files

- `src/app.rs` - Main app struct and event loop
- `src/tools/pipeline.rs` - Effect enum definition
- `src/tools/exec.rs` - Tool executor, WaitingFor states
- `src/tools/handlers.rs` - Effect handlers (AwaitIdeSlot, etc.)
