# Sub-Agent Architecture Options

## Problem Statement

The primary agent sometimes needs to delegate work to sub-agents for:
- Research tasks (exploring codebase, reading docs)
- Parallel investigation of multiple approaches
- Long-running background work while staying responsive

Current implementation has issues:
- Sub-agents registered in `AgentRegistry` try to stream to transcript (crash)
- No mechanism to return sub-agent results to primary agent
- Tool execution shares the same `ToolExecutor` with approval flow designed for UI

## Option 1: Sub-Agents as Tools (Current Direction)

Sub-agent runs to completion within a tool call, returns result as tool output.

```
Primary Agent                    Sub-Agent
     │                               
     ├─► tool_call(task)             
     │       │                       
     │       └──────────────────► spawn
     │                               │
     │   (primary blocked/waiting)   ├─► thinking...
     │                               ├─► tool calls...
     │                               ├─► more thinking...
     │                               └─► done
     │                               │
     ◄───────────────────────────────┘ result string
     │
     ├─► continues with result
```

### Pros
- Simple mental model: tool in, result out
- Result becomes part of primary agent's context
- No UI complexity - sub-agent is invisible to user
- Clean separation - sub-agent has own runner, no registry

### Cons  
- Primary agent "blocked" while sub-agent runs (though UI stays responsive)
- No visibility into sub-agent progress
- Result must fit in context window
- Can't interact with sub-agent mid-task

### Implementation
- `BackgroundRunner` module with `run_agent()` function
- Simple tool execution without approval flow
- `SpawnAgent` handler awaits completion, returns output

---

## Option 2: Streamed Sub-Agent Output

Sub-agent output streams into primary agent's context in real-time.

```
Primary Agent                    Sub-Agent
     │                               
     ├─► spawn_task(task)            
     │       │                       
     │       └──────────────────► spawn
     │                               │
     ├─► continues...                ├─► thinking...
     │                               │
     ◄───────────── stream ──────────┤ "Found 3 files..."
     │   (injected as context)       │
     ├─► responds to user            ├─► tool calls...
     │                               │
     ◄───────────── stream ──────────┤ "Analysis complete..."
     │                               │
     ├─► incorporates findings       └─► done
```

### Pros
- Real-time visibility into sub-agent work
- Primary can react to partial results
- More collaborative feel

### Cons
- Complex: how does streamed content appear in primary's context?
- Token usage: sub-agent output consumes primary's context
- Ordering issues: what if primary responds before sub-agent finishes?
- API limitations: can't inject content mid-stream

### Implementation Challenges
- Would need synthetic "user messages" injecting sub-agent updates
- Or a special message role for sub-agent content
- Anthropic API doesn't support this natively

---

## Option 3: Parallel Agents with "Swipe" UI

Multiple agents run independently, user can switch between them.

```
┌─────────────────────────────────────────┐
│  Agent 1 (primary)  │  Agent 2 (task)   │
│  ◄─── swipe ───►    │                   │
├─────────────────────────────────────────┤
│                                         │
│  Currently viewing: Agent 1             │
│                                         │
│  > Working on the main feature...       │
│                                         │
│  [Agent 2 working in background ···]    │
│                                         │
└─────────────────────────────────────────┘
```

### Pros
- Full visibility into all agent work
- User can interact with any agent
- Natural for truly parallel tasks
- Each agent has own context/transcript

### Cons
- UI complexity: tabs, indicators, notifications
- User cognitive load: tracking multiple conversations
- When/how do agents share information?
- Resource usage: multiple active streams

### Implementation
- `AgentRegistry` already supports multiple agents
- UI needs tab bar or swipe gestures
- Need "bring result to primary" action
- Background indicator in status bar

---

## Option 4: Hybrid - Tool with Progress Updates

Sub-agent runs as tool but emits progress to UI without affecting primary context.

```
Primary Agent                    Sub-Agent
     │                               
     ├─► tool_call(task)             
     │       │                       
     │       └──────────────────► spawn
     │                               │
     │   ┌─────────────────────────────┐
     │   │ Task: analyzing codebase    │  ◄── UI overlay
     │   │ Progress: reading files...  │
     │   └─────────────────────────────┘
     │                               │
     │   (UI shows progress,         ├─► working...
     │    primary waits)             │
     │                               └─► done
     │                               │
     ◄───────────────────────────────┘ result
     │
     ├─► continues with result
```

### Pros
- User sees sub-agent is working
- Progress visible without cluttering transcript
- Still simple tool model for agent
- Could show sub-agent thinking/tool calls in collapsible panel

### Cons
- More UI complexity
- Still can't interact with sub-agent
- Need to design progress display

### Implementation
- Sub-agent streams to a separate UI component (not transcript)
- `TaskBlock` in transcript shows summary + expandable details
- Progress updates via channel to UI layer

---

## Comparison Matrix

| Aspect | Tool (Opt 1) | Streamed (Opt 2) | Parallel (Opt 3) | Hybrid (Opt 4) |
|--------|--------------|------------------|------------------|----------------|
| Implementation complexity | Low | High | Medium | Medium |
| User visibility | None | Full | Full | Partial |
| User interaction | None | None | Full | None |
| Context efficiency | Good | Poor | Good | Good |
| API compatibility | ✓ | ✗ | ✓ | ✓ |
| Progress feedback | None | Real-time | Real-time | Real-time |

---

## Recommendation

**Start with Option 1** (tool-based) as the foundation:
- Simplest to implement correctly
- Unblocks the current crash
- Clean architecture with `BackgroundRunner`

**Then consider Option 4** (hybrid with progress) as enhancement:
- Add progress UI to show sub-agent is working
- Expandable task block to see sub-agent details
- Doesn't change the fundamental model

**Option 3** (parallel swipe) could be a separate feature:
- For power users who want multiple agents
- Independent of sub-agent-as-tool pattern
- Could be triggered by explicit `/spawn` command vs automatic `task` tool

---

## Open Questions

1. **Should sub-agents be able to spawn sub-sub-agents?**
   - Probably not initially (prevent recursion bombs)
   - Could add with depth limit later

2. **How to handle sub-agent errors?**
   - Return error as tool result
   - Primary agent can decide to retry or report to user

3. **Should sub-agent tools be configurable?**
   - Currently: read-only tools for background agents
   - Could allow full tools with user confirmation

4. **Token limits for sub-agent results?**
   - Truncate long results?
   - Summarize before returning?
   - Let sub-agent decide what's important?

5. **Caching/reuse of sub-agent results?**
   - Same task = same result?
   - Useful for repeated research queries
