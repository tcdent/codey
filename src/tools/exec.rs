//! Tool execution engine
//!
//! Executes tool pipelines with approval flow and streaming output.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use tokio::sync::oneshot;

use crate::effect::EffectResult;
use crate::llm::AgentId;
use crate::transcript::Status;
use crate::tools::pipeline::{Effect, Step, ToolPipeline};
use crate::tools::ToolRegistry;

// =============================================================================
// Polling helpers
// =============================================================================

/// Noop waker for manual polling
struct NoopWaker;
impl Wake for NoopWaker {
    fn wake(self: Arc<Self>) {}
}

/// Poll a oneshot receiver without blocking
fn poll_receiver<T>(rx: &mut oneshot::Receiver<T>) -> Poll<Result<T, oneshot::error::RecvError>> {
    let waker = Waker::from(Arc::new(NoopWaker));
    let mut cx = Context::from_waker(&waker);
    Pin::new(rx).poll(&mut cx)
}

// =============================================================================
// Types
// =============================================================================

/// What an active pipeline is waiting for (mutually exclusive states)
enum WaitingFor {
    /// Not waiting - ready to execute next step
    Nothing,
    /// Waiting for a handler to complete (spawned in separate task)
    Handler(oneshot::Receiver<Step>),
    /// Waiting for delegated effect to complete
    Effect(oneshot::Receiver<EffectResult>),
    /// Waiting for user approval (Ok = approved, Err = denied)
    Approval(oneshot::Receiver<EffectResult>),
}

/// A tool call pending execution
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub agent_id: AgentId,
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub decision: ToolDecision,
    /// If true, execute in background and return immediately
    pub background: bool,
}

impl ToolCall {
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = agent_id;
        self
    }
}

/// Decision state for a pending tool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolDecision {
    #[default]
    Pending,
    Requested,
    Approve,
    Deny,
}

/// Events emitted by the tool executor
#[derive(Debug)]
pub enum ToolEvent {
    /// Effect delegated to app (IDE, agents, approvals, etc)
    Delegate {
        agent_id: AgentId,
        call_id: String,
        effect: Effect,
        /// Send result back to executor to continue/error the pipeline
        responder: oneshot::Sender<EffectResult>,
    },
    /// Streaming output from execution
    Delta {
        agent_id: AgentId,
        call_id: String,
        content: String,
    },
    /// Tool execution completed successfully
    Completed {
        agent_id: AgentId,
        call_id: String,
        content: String,
    },
    /// Tool execution failed
    Error {
        agent_id: AgentId,
        call_id: String,
        content: String,
    },
    /// Background tool started - placeholder sent to agent
    BackgroundStarted {
        agent_id: AgentId,
        call_id: String,
        name: String,
    },
    /// Background tool completed - notification for agent
    BackgroundCompleted {
        agent_id: AgentId,
        call_id: String,
        name: String,
    },
}

impl ToolEvent {
    fn completed(active: ActivePipeline) -> Self {
        Self::Completed {
            agent_id: active.agent_id,
            call_id: active.call_id,
            content: active.output,
        }
    }

    fn error(active: ActivePipeline, content: impl Into<String>) -> Self {
        Self::Error {
            agent_id: active.agent_id,
            call_id: active.call_id,
            content: content.into(),
        }
    }

    fn delta(active: &ActivePipeline, content: String) -> Self {
        Self::Delta {
            agent_id: active.agent_id,
            call_id: active.call_id.clone(),
            content,
        }
    }

    fn delegate(
        active: &ActivePipeline,
        effect: Effect,
    ) -> (Self, oneshot::Receiver<EffectResult>) {
        let (tx, rx) = oneshot::channel();
        (
            Self::Delegate {
                agent_id: active.agent_id,
                call_id: active.call_id.clone(),
                effect,
                responder: tx,
            },
            rx,
        )
    }
}

/// Active pipeline execution state
struct ActivePipeline {
    agent_id: AgentId,
    call_id: String,
    name: String,
    params: serde_json::Value,
    pipeline: ToolPipeline,
    output: String,
    /// What we're currently waiting for (if anything)
    waiting: WaitingFor,
    /// Original decision from tool call (for pre-approval)
    original_decision: ToolDecision,
    /// If true, this is a background task
    background: bool,
    /// Execution status
    status: Status,
}

impl ActivePipeline {
    fn new(tool_call: ToolCall, pipeline: ToolPipeline) -> Self {
        Self {
            agent_id: tool_call.agent_id,
            call_id: tool_call.call_id,
            name: tool_call.name,
            original_decision: tool_call.decision,
            params: tool_call.params,
            background: tool_call.background,
            pipeline,
            output: String::new(),
            waiting: WaitingFor::Nothing,
            status: Status::Running,
        }
    }
    
    /// Check if pipeline is waiting for something
    fn is_waiting(&self) -> bool {
        !matches!(self.waiting, WaitingFor::Nothing)
    }
    
    /// Check if pipeline is waiting for a handler (spawned task)
    fn is_waiting_for_handler(&self) -> bool {
        matches!(self.waiting, WaitingFor::Handler(_))
    }
    
    /// Check if pipeline is complete (no more steps)
    fn is_complete(&self) -> bool {
        self.pipeline.is_empty() && !self.is_waiting()
    }
    
    // -- State transitions --
    
    /// Transition to error state
    fn set_error(&mut self, msg: impl Into<String>) {
        self.status = Status::Error;
        self.output = msg.into();
        self.waiting = WaitingFor::Nothing;
    }
    
    /// Transition to complete state
    fn set_complete(&mut self) {
        self.status = Status::Complete;
        self.waiting = WaitingFor::Nothing;
    }
    
    /// Transition to denied state
    fn set_denied(&mut self) {
        self.status = Status::Denied;
        self.waiting = WaitingFor::Nothing;
    }
}

/// Executes tools with approval flow and streaming output.
///
/// Supports concurrent execution of multiple tools. Blocking tools emit
/// Completed/Error events and are removed from tracking. Background tools
/// emit BackgroundStarted immediately and BackgroundCompleted when done,
/// staying in active map until results are retrieved via take_result().
pub struct ToolExecutor {
    tools: ToolRegistry,
    pending: VecDeque<ToolCall>,
    /// All active pipelines, keyed by call_id
    active: HashMap<String, ActivePipeline>,
    /// Flag to signal cancellation
    cancelled: bool,
}

impl ToolExecutor {
    pub fn new(tools: ToolRegistry) -> Self {
        Self {
            tools,
            pending: VecDeque::new(),
            active: HashMap::new(),
            cancelled: false,
        }
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }
    
    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }

    /// Cancel any active or pending tool execution
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.pending.clear();
        // Only clear non-background tasks
        self.active.retain(|_, p| p.background && p.status != Status::Running);
    }

    pub fn enqueue(&mut self, tool_calls: Vec<ToolCall>) {
        self.pending.extend(tool_calls);
    }
    
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
    
    /// Get output from a background task without removing it (for testing)
    #[cfg(test)]
    pub fn get_background_output(&self, call_id: &str) -> Option<String> {
        self.active.get(call_id)
            .filter(|p| p.background)
            .map(|p| p.output.clone())
    }
    
    /// Check if a tool call is ready to start (not denied/requested)
    fn is_ready(tool_call: &ToolCall) -> bool {
        matches!(tool_call.decision, ToolDecision::Pending | ToolDecision::Approve)
    }
    
    /// Check if a foreground tool is currently running for a specific agent
    fn has_running_foreground_for_agent(&self, agent_id: AgentId) -> bool {
        self.active.values().any(|p| 
            p.agent_id == agent_id && !p.background && p.status == Status::Running
        )
    }
    
    /// Count running background tasks
    pub fn running_background_count(&self) -> usize {
        self.active.values().filter(|p| p.background && p.status == Status::Running).count()
    }
    
    /// Start a tool by composing its pipeline and adding to active
    fn start_tool(&mut self, tool_call: ToolCall) {
        let call_id = tool_call.call_id.clone();
        let tool = self.tools.get(&tool_call.name);
        let pipeline = tool.compose(tool_call.params.clone());
        self.active.insert(call_id, ActivePipeline::new(tool_call, pipeline));
    }

    pub async fn next(&mut self) -> Option<ToolEvent> {
        // Check for cancellation
        if self.cancelled {
            self.cancelled = false;
            return None;
        }

        // Start pending tools with these rules:
        // - Foreground tools execute strictly in order (FIFO), one at a time per agent
        // - Background tools can start anytime, order not guaranteed
        
        // Start one foreground tool per agent that doesn't already have one running
        let fg_agent_ids: HashSet<_> = self.pending.iter()
            .filter(|t| !t.background && Self::is_ready(t))
            .map(|t| t.agent_id)
            .collect();
        
        for agent_id in fg_agent_ids {
            if !self.has_running_foreground_for_agent(agent_id) {
                if let Some(idx) = self.pending.iter().position(|t| 
                    t.agent_id == agent_id && !t.background && Self::is_ready(t)
                ) {
                    let tool_call = self.pending.remove(idx).unwrap();
                    self.start_tool(tool_call);
                }
            }
        }
        
        // Start all ready background tools
        // (Collect indices first, then remove in reverse to preserve indices)
        let bg_indices: Vec<_> = self.pending.iter()
            .enumerate()
            .filter(|(_, t)| t.background && Self::is_ready(t))
            .map(|(i, _)| i)
            .collect();
        for idx in bg_indices.into_iter().rev() {
            let tool_call = self.pending.remove(idx).unwrap();
            self.start_tool(tool_call);
        }

        // Main execution loop - keeps going while there's work to do
        loop {
            // Poll all active pipelines for pending results (handlers, effects, approvals)
            let call_ids: Vec<String> = self.active.keys().cloned().collect();
            for call_id in &call_ids {
                if let Some(event) = self.poll_waiting(call_id) {
                    return Some(event);
                }
            }

            // Execute steps on non-waiting pipelines
            let call_ids: Vec<String> = self.active.keys().cloned().collect();
            let mut any_stepped = false;
            let mut any_waiting_for_handler = false;
            
            for call_id in &call_ids {
                let (should_step, is_handler_wait) = {
                    match self.active.get(call_id) {
                        Some(active) => (
                            // Step if not waiting and either:
                            // - Running (including when empty, to trigger completion)
                            // - Denied/Error with finally handlers still remaining
                            !active.is_waiting() && 
                                (active.status == Status::Running || !active.pipeline.is_empty()),
                            active.is_waiting_for_handler(),
                        ),
                        None => (false, false),
                    }
                };
                
                if is_handler_wait {
                    any_waiting_for_handler = true;
                }
                
                if should_step {
                    any_stepped = true;
                    if let Some(event) = self.execute_step(call_id) {
                        return Some(event);
                    }
                }
            }
            
            // If we stepped something, continue the loop (execute_step may have spawned a handler)
            if any_stepped {
                continue;
            }
            
            // If any pipelines are waiting for spawned handlers, yield to let them run
            if any_waiting_for_handler {
                tokio::task::yield_now().await;
                continue;
            }
            
            // Check if we're blocked waiting for user input (approvals) or effects
            let any_waiting_for_external = self.active.values().any(|p| {
                matches!(p.waiting, WaitingFor::Approval(_) | WaitingFor::Effect(_))
            });
            
            if any_waiting_for_external {
                // Sleep briefly to avoid busy-polling while waiting for user input
                // The main app loop handles keyboard events separately
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
            
            // Nothing to step and nothing waiting - we're done for now
            break;
        }
        
        None
    }

    /// Poll the waiting state for a specific pipeline
    fn poll_waiting(&mut self, call_id: &str) -> Option<ToolEvent> {
        let active = self.active.get_mut(call_id)?;
        
        // Take ownership of waiting state to poll it
        let waiting = std::mem::replace(&mut active.waiting, WaitingFor::Nothing);
        
        match waiting {
            WaitingFor::Nothing => None,
            
            WaitingFor::Handler(mut rx) => {
                match poll_receiver(&mut rx) {
                    Poll::Ready(Ok(step)) => {
                        // Handler completed - process the step result
                        self.process_step(call_id, step)
                    },
                    Poll::Ready(Err(_)) => {
                        // Channel dropped - handler panicked or was cancelled
                        let active = self.active.get_mut(call_id)?;
                        if active.background {
                            active.set_error("Handler channel dropped");
                            Some(ToolEvent::BackgroundCompleted {
                                agent_id: active.agent_id,
                                call_id: active.call_id.clone(),
                                name: active.name.clone(),
                            })
                        } else {
                            let active = self.active.remove(call_id).unwrap();
                            Some(ToolEvent::error(active, "Handler channel dropped"))
                        }
                    },
                    Poll::Pending => {
                        // Still running - put it back
                        self.active.get_mut(call_id).unwrap().waiting = WaitingFor::Handler(rx);
                        None
                    },
                }
            },
            
            WaitingFor::Effect(mut rx) => {
                match poll_receiver(&mut rx) {
                    Poll::Ready(Ok(Ok(None))) => {
                        // Effect completed, no output - continue pipeline
                        None
                    },
                    Poll::Ready(Ok(Ok(Some(output)))) => {
                        // Effect completed with output - inject into pipeline
                        active.output = output;
                        None
                    },
                    Poll::Ready(Ok(Err(msg))) => {
                        let mut active = self.active.remove(call_id).unwrap();
                        if active.background {
                            active.set_error(msg);
                            let event = ToolEvent::BackgroundCompleted {
                                agent_id: active.agent_id,
                                call_id: active.call_id.clone(),
                                name: active.name.clone(),
                            };
                            self.active.insert(active.call_id.clone(), active);
                            Some(event)
                        } else {
                            Some(ToolEvent::error(active, msg))
                        }
                    },
                    Poll::Ready(Err(_)) => {
                        let mut active = self.active.remove(call_id).unwrap();
                        if active.background {
                            active.set_error("Effect channel dropped");
                            let event = ToolEvent::BackgroundCompleted {
                                agent_id: active.agent_id,
                                call_id: active.call_id.clone(),
                                name: active.name.clone(),
                            };
                            self.active.insert(active.call_id.clone(), active);
                            Some(event)
                        } else {
                            Some(ToolEvent::error(active, "Effect channel dropped"))
                        }
                    },
                    Poll::Pending => {
                        // Put it back - still waiting
                        self.active.get_mut(call_id).unwrap().waiting = WaitingFor::Effect(rx);
                        None
                    },
                }
            },
            
            WaitingFor::Approval(mut rx) => {
                let poll_result = poll_receiver(&mut rx);
                
                match poll_result {
                    Poll::Ready(Ok(Ok(_))) => {
                        // Approved - continue pipeline
                        // For background tools, emit BackgroundStarted now
                        if active.background {
                            Some(ToolEvent::BackgroundStarted {
                                agent_id: active.agent_id,
                                call_id: active.call_id.clone(),
                                name: active.name.clone(),
                            })
                        } else {
                            None
                        }
                    },
                    Poll::Ready(Ok(Err(reason))) => {
                        // Denied - skip to finally
                        active.pipeline.skip_to_finally();
                        active.set_denied();
                        Some(ToolEvent::Error {
                            agent_id: active.agent_id,
                            call_id: active.call_id.clone(),
                            content: reason,
                        })
                    },
                    Poll::Ready(Err(_)) => {
                        tracing::warn!("poll_waiting: approval channel dropped");
                        let active = self.active.remove(call_id).unwrap();
                        Some(ToolEvent::error(active, "Approval cancelled"))
                    },
                    Poll::Pending => {
                        tracing::trace!("poll_waiting: Pending (waiting for user approval)");
                        // Put it back - still waiting
                        self.active.get_mut(call_id).unwrap().waiting = WaitingFor::Approval(rx);
                        None
                    },
                }
            },
        }
    }

    /// Execute next handler in a specific pipeline.
    /// Spawns the handler in a separate task to avoid losing state if dropped.
    fn execute_step(&mut self, call_id: &str) -> Option<ToolEvent> {
        // Get the handler to execute
        let handler = {
            let active = self.active.get_mut(call_id)?;
            match active.pipeline.pop() {
                Some(h) => h,
                None => {
                    // Pipeline complete - finally handlers have run
                    // For denied/errored pipelines, we already emitted the event, just cleanup
                    if active.status == Status::Denied || active.status == Status::Error {
                        self.active.remove(call_id);
                        return None;
                    }
                    
                    if active.background {
                        active.set_complete();
                        return Some(ToolEvent::BackgroundCompleted {
                            agent_id: active.agent_id,
                            call_id: active.call_id.clone(),
                            name: active.name.clone(),
                        });
                    } else {
                        let active = self.active.remove(call_id).unwrap();
                        return Some(ToolEvent::completed(active));
                    }
                },
            }
        };

        // Spawn handler in separate task so it won't be lost if our future is dropped.
        // The result will be polled via WaitingFor::Handler in poll_waiting().
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let step = handler.call().await;
            let _ = tx.send(step);
        });
        
        let active = self.active.get_mut(call_id)?;
        active.waiting = WaitingFor::Handler(rx);
        None
    }
    
    /// Process a Step result from a completed handler.
    /// Factored out since it's used by poll_waiting when Handler completes.
    fn process_step(&mut self, call_id: &str, step: Step) -> Option<ToolEvent> {
        let active = self.active.get_mut(call_id)?;
        match step {
            Step::Continue => None,
            Step::Output(content) => {
                active.output = content;
                None
            },
            Step::Delta(content) => {
                Some(ToolEvent::delta(active, content))
            },
            Step::Delegate(effect) => {
                let (event, rx) = ToolEvent::delegate(active, effect);
                active.waiting = WaitingFor::Effect(rx);
                Some(event)
            },
            Step::AwaitApproval => {
                // Skip approval if tool was pre-approved
                if active.original_decision == ToolDecision::Approve {
                    // For background tools, emit BackgroundStarted now
                    if active.background {
                        Some(ToolEvent::BackgroundStarted {
                            agent_id: active.agent_id,
                            call_id: active.call_id.clone(),
                            name: active.name.clone(),
                        })
                    } else {
                        None  // Continue to next step
                    }
                } else {
                    // Delegate approval to app via Effect::AwaitApproval
                    let effect = Effect::AwaitApproval {
                        name: active.name.clone(),
                        params: active.params.clone(),
                        background: active.background,
                    };
                    let (event, rx) = ToolEvent::delegate(active, effect);
                    active.waiting = WaitingFor::Approval(rx);
                    Some(event)
                }
            },
            Step::Error(msg) => {
                if active.background {
                    active.set_error(&msg);
                    Some(ToolEvent::BackgroundCompleted {
                        agent_id: active.agent_id,
                        call_id: active.call_id.clone(),
                        name: active.name.clone(),
                    })
                } else {
                    let active = self.active.remove(call_id).unwrap();
                    Some(ToolEvent::error(active, msg))
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::impls::ShellTool;
    use std::collections::HashSet;

    /// Helper to collect all events until no more are produced
    async fn collect_events(executor: &mut ToolExecutor) -> Vec<ToolEvent> {
        let mut events = vec![];
        while let Some(event) = executor.next().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn test_single_tool_completes() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test1".to_string(),
            name: "mcp_shell".to_string(),
            params: serde_json::json!({ "command": "echo hello" }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        let events = collect_events(&mut executor).await;
        
        assert_eq!(events.len(), 1);
        match &events[0] {
            ToolEvent::Completed { content, .. } => {
                assert!(content.contains("hello"));
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_multiple_tools_sequential() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Enqueue two tools
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "test1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo first" }),
                decision: ToolDecision::Approve,
                background: false,
            },
            ToolCall {
                agent_id: 0,
                call_id: "test2".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo second" }),
                decision: ToolDecision::Approve,
                background: false,
            },
        ]);

        let events = collect_events(&mut executor).await;
        
        // Both should complete
        let completed: Vec<_> = events.iter().filter(|e| matches!(e, ToolEvent::Completed { .. })).collect();
        assert_eq!(completed.len(), 2, "Expected 2 Completed events, got {}", completed.len());
        
        // Verify both outputs are present
        let outputs: Vec<_> = completed.iter().map(|e| {
            if let ToolEvent::Completed { content, .. } = e { content.clone() } else { String::new() }
        }).collect();
        assert!(outputs.iter().any(|o| o.contains("first")), "Missing 'first' output");
        assert!(outputs.iter().any(|o| o.contains("second")), "Missing 'second' output");
    }

    #[tokio::test]
    async fn test_background_tools_concurrent() {
        // This test verifies that concurrent background tools don't lose output
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Enqueue multiple background tools
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "bg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo background_one" }),
                decision: ToolDecision::Approve,
                background: true,
            },
            ToolCall {
                agent_id: 0,
                call_id: "bg2".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo background_two" }),
                decision: ToolDecision::Approve,
                background: true,
            },
            ToolCall {
                agent_id: 0,
                call_id: "bg3".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo background_three" }),
                decision: ToolDecision::Approve,
                background: true,
            },
        ]);

        let events = collect_events(&mut executor).await;
        
        // Should have BackgroundStarted and BackgroundCompleted for each
        let started: HashSet<_> = events.iter().filter_map(|e| {
            if let ToolEvent::BackgroundStarted { call_id, .. } = e { Some(call_id.clone()) } else { None }
        }).collect();
        let completed: HashSet<_> = events.iter().filter_map(|e| {
            if let ToolEvent::BackgroundCompleted { call_id, .. } = e { Some(call_id.clone()) } else { None }
        }).collect();
        
        assert_eq!(started.len(), 3, "Expected 3 BackgroundStarted events");
        assert_eq!(completed.len(), 3, "Expected 3 BackgroundCompleted events");
        assert!(started.contains("bg1"));
        assert!(started.contains("bg2"));
        assert!(started.contains("bg3"));
        assert!(completed.contains("bg1"));
        assert!(completed.contains("bg2"));
        assert!(completed.contains("bg3"));
        
        // Verify outputs are stored (not lost!)
        let output1 = executor.get_background_output("bg1");
        let output2 = executor.get_background_output("bg2");
        let output3 = executor.get_background_output("bg3");
        
        assert!(output1.is_some(), "bg1 output was lost!");
        assert!(output2.is_some(), "bg2 output was lost!");
        assert!(output3.is_some(), "bg3 output was lost!");
        
        assert!(output1.unwrap().contains("background_one"), "bg1 has wrong output");
        assert!(output2.unwrap().contains("background_two"), "bg2 has wrong output");
        assert!(output3.unwrap().contains("background_three"), "bg3 has wrong output");
    }

    #[tokio::test]
    async fn test_mixed_foreground_background() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Mix of foreground and background tools
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "fg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo foreground" }),
                decision: ToolDecision::Approve,
                background: false,
            },
            ToolCall {
                agent_id: 0,
                call_id: "bg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo background" }),
                decision: ToolDecision::Approve,
                background: true,
            },
        ]);

        let events = collect_events(&mut executor).await;
        
        // Foreground should have Completed
        let fg_completed = events.iter().any(|e| {
            matches!(e, ToolEvent::Completed { call_id, .. } if call_id == "fg1")
        });
        assert!(fg_completed, "Foreground tool should complete normally");
        
        // Background should have BackgroundCompleted
        let bg_completed = events.iter().any(|e| {
            matches!(e, ToolEvent::BackgroundCompleted { call_id, .. } if call_id == "bg1")
        });
        assert!(bg_completed, "Background tool should complete");
        
        // Verify background output is stored
        let bg_output = executor.get_background_output("bg1");
        assert!(bg_output.is_some(), "Background output was lost");
        assert!(bg_output.unwrap().contains("background"));
    }

    #[tokio::test]
    async fn test_handler_spawn_doesnt_block() {
        // This test verifies that spawning handlers doesn't block - multiple tools
        // can have their handlers running concurrently
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Use sleep to make tools take some time
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "slow1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "sleep 0.1 && echo slow1_done" }),
                decision: ToolDecision::Approve,
                background: true,
            },
            ToolCall {
                agent_id: 0,
                call_id: "slow2".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "sleep 0.1 && echo slow2_done" }),
                decision: ToolDecision::Approve,
                background: true,
            },
        ]);

        let start = std::time::Instant::now();
        let events = collect_events(&mut executor).await;
        let elapsed = start.elapsed();
        
        // If running concurrently, should take ~0.1s. If sequential, ~0.2s
        // Allow some margin for test flakiness
        assert!(elapsed.as_millis() < 180, "Tools should run concurrently, took {:?}", elapsed);
        
        // Both should complete
        let completed: HashSet<_> = events.iter().filter_map(|e| {
            if let ToolEvent::BackgroundCompleted { call_id, .. } = e { Some(call_id.clone()) } else { None }
        }).collect();
        assert_eq!(completed.len(), 2);
        
        // Both outputs should be present
        assert!(executor.get_background_output("slow1").unwrap().contains("slow1_done"));
        assert!(executor.get_background_output("slow2").unwrap().contains("slow2_done"));
    }

    #[tokio::test]
    async fn test_foreground_fifo_order() {
        // Foreground tools should execute in strict FIFO order,
        // even if all are auto-approved
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Enqueue three foreground tools - second one sleeps to test ordering
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "fg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "sleep 0.05 && echo first" }),
                decision: ToolDecision::Approve,
                background: false,
            },
            ToolCall {
                agent_id: 0,
                call_id: "fg2".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo second" }),
                decision: ToolDecision::Approve,
                background: false,
            },
        ]);

        let events = collect_events(&mut executor).await;
        
        // Get completion order by extracting call_ids from Completed events
        let completion_order: Vec<_> = events.iter().filter_map(|e| {
            if let ToolEvent::Completed { call_id, .. } = e { Some(call_id.clone()) } else { None }
        }).collect();
        
        assert_eq!(completion_order, vec!["fg1", "fg2"], 
            "Foreground tools should complete in FIFO order, got {:?}", completion_order);
    }

    #[tokio::test]
    async fn test_background_doesnt_block_foreground() {
        // A running background tool should not block foreground tools from starting
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Background tool that takes a while, followed by fast foreground
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "bg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "sleep 0.1 && echo background" }),
                decision: ToolDecision::Approve,
                background: true,
            },
            ToolCall {
                agent_id: 0,
                call_id: "fg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo foreground" }),
                decision: ToolDecision::Approve,
                background: false,
            },
        ]);

        // Collect events and track completion order
        let mut completion_order: Vec<String> = vec![];
        
        while let Some(event) = executor.next().await {
            match &event {
                ToolEvent::Completed { call_id, .. } => {
                    completion_order.push(call_id.clone());
                }
                ToolEvent::BackgroundCompleted { call_id, .. } => {
                    completion_order.push(call_id.clone());
                }
                _ => {}
            }
        }
        
        // Foreground (instant echo) should complete before background (0.1s sleep)
        assert_eq!(completion_order.len(), 2, "Both tools should complete");
        assert_eq!(completion_order[0], "fg1", "Foreground should complete first");
        assert_eq!(completion_order[1], "bg1", "Background should complete second");
    }

    #[tokio::test]
    async fn test_foreground_per_agent_lanes() {
        // Foreground tools from different agents should run concurrently.
        // Agent 0's slow foreground tool should NOT block agent 1's fast foreground tool.
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Agent 0: slow foreground tool (sleeps 0.1s)
        // Agent 1: fast foreground tool (instant echo)
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "agent0_slow".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "sleep 0.1 && echo agent0" }),
                decision: ToolDecision::Approve,
                background: false,
            },
            ToolCall {
                agent_id: 1,
                call_id: "agent1_fast".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo agent1" }),
                decision: ToolDecision::Approve,
                background: false,
            },
        ]);

        // Collect events and track completion order
        let mut completion_order: Vec<String> = vec![];
        
        while let Some(event) = executor.next().await {
            if let ToolEvent::Completed { call_id, .. } = &event {
                completion_order.push(call_id.clone());
            }
        }
        
        // Agent 1's fast tool should complete before agent 0's slow tool
        assert_eq!(completion_order.len(), 2, "Both tools should complete");
        assert_eq!(completion_order[0], "agent1_fast", 
            "Agent 1's fast foreground should complete first (not blocked by agent 0)");
        assert_eq!(completion_order[1], "agent0_slow", 
            "Agent 0's slow foreground should complete second");
    }

    #[tokio::test]
    async fn test_foreground_fifo_within_agent() {
        // Foreground tools within the SAME agent should still be FIFO.
        // This ensures per-agent lanes don't break intra-agent ordering.
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Agent 0: two foreground tools, first one sleeps
        // Agent 1: one foreground tool (to prove cross-agent concurrency still works)
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "agent0_first".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "sleep 0.05 && echo first" }),
                decision: ToolDecision::Approve,
                background: false,
            },
            ToolCall {
                agent_id: 0,
                call_id: "agent0_second".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo second" }),
                decision: ToolDecision::Approve,
                background: false,
            },
            ToolCall {
                agent_id: 1,
                call_id: "agent1_tool".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo agent1" }),
                decision: ToolDecision::Approve,
                background: false,
            },
        ]);

        let mut completion_order: Vec<String> = vec![];
        
        while let Some(event) = executor.next().await {
            if let ToolEvent::Completed { call_id, .. } = &event {
                completion_order.push(call_id.clone());
            }
        }
        
        assert_eq!(completion_order.len(), 3, "All three tools should complete");
        
        // Agent 1 should complete first (instant, runs concurrently)
        assert_eq!(completion_order[0], "agent1_tool",
            "Agent 1's tool should complete first (concurrent with agent 0)");
        
        // Agent 0's tools should complete in FIFO order (first before second)
        let agent0_order: Vec<_> = completion_order.iter()
            .filter(|id| id.starts_with("agent0"))
            .collect();
        assert_eq!(agent0_order, vec!["agent0_first", "agent0_second"],
            "Agent 0's foreground tools should complete in FIFO order");
    }

    #[tokio::test]
    async fn test_foreground_approval_blocks_subsequent() {
        // A foreground tool waiting for approval should block subsequent foreground tools
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        // Two foreground tools, first needs approval
        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "fg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo first" }),
                decision: ToolDecision::Pending, // Needs approval
                background: false,
            },
            ToolCall {
                agent_id: 0,
                call_id: "fg2".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo second" }),
                decision: ToolDecision::Approve, // Auto-approved
                background: false,
            },
        ]);

        // First event should be Delegate with AwaitApproval for fg1
        let event1 = executor.next().await.unwrap();
        let responder = match event1 {
            ToolEvent::Delegate { call_id, effect: Effect::AwaitApproval { .. }, responder, .. } => {
                assert_eq!(call_id, "fg1");
                responder
            }
            other => panic!("Expected Delegate/AwaitApproval for fg1, got {:?}", other),
        };

        // fg2 should NOT have started yet (no events available without blocking)
        // Approve fg1
        responder.send(Ok(None)).unwrap();  // Approve

        // Now collect remaining events
        let events = collect_events(&mut executor).await;
        
        // Should see fg1 complete, then fg2 complete (in order)
        let completion_order: Vec<_> = events.iter().filter_map(|e| {
            if let ToolEvent::Completed { call_id, .. } = e { Some(call_id.clone()) } else { None }
        }).collect();
        
        assert_eq!(completion_order, vec!["fg1", "fg2"],
            "After approval, tools should complete in order");
    }

    #[tokio::test]
    async fn test_foreground_denial_unblocks_next() {
        // Denying a foreground tool should allow the next foreground tool to start
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "fg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo first" }),
                decision: ToolDecision::Pending,
                background: false,
            },
            ToolCall {
                agent_id: 0,
                call_id: "fg2".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo second" }),
                decision: ToolDecision::Pending,
                background: false,
            },
        ]);

        // Get approval request for fg1
        let event1 = executor.next().await.unwrap();
        let responder1 = match event1 {
            ToolEvent::Delegate { call_id, effect: Effect::AwaitApproval { .. }, responder, .. } => {
                assert_eq!(call_id, "fg1");
                responder
            }
            other => panic!("Expected Delegate/AwaitApproval for fg1, got {:?}", other),
        };

        // Deny fg1
        responder1.send(Err("Denied by user".to_string())).unwrap();

        // Should get Error for fg1, then Delegate/AwaitApproval for fg2
        let mut saw_fg1_error = false;
        let mut saw_fg2_approval = false;
        let mut responder2 = None;
        
        while let Some(event) = executor.next().await {
            match event {
                ToolEvent::Error { call_id, .. } if call_id == "fg1" => {
                    saw_fg1_error = true;
                }
                ToolEvent::Delegate { call_id, effect: Effect::AwaitApproval { .. }, responder, .. } if call_id == "fg2" => {
                    saw_fg2_approval = true;
                    responder2 = Some(responder);
                    break; // Got what we need
                }
                _ => {}
            }
        }
        
        assert!(saw_fg1_error, "Should see error for denied fg1");
        assert!(saw_fg2_approval, "fg2 should get approval request after fg1 denied");
        
        // Approve fg2 and verify it completes
        responder2.unwrap().send(Ok(None)).unwrap();  // Approve
        let events = collect_events(&mut executor).await;
        
        let fg2_completed = events.iter().any(|e| {
            matches!(e, ToolEvent::Completed { call_id, .. } if call_id == "fg2")
        });
        assert!(fg2_completed, "fg2 should complete after approval");
    }

    #[tokio::test]
    async fn test_background_with_pending_foreground() {
        // Background tools should start even when a foreground tool is waiting for approval
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![
            ToolCall {
                agent_id: 0,
                call_id: "fg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo foreground" }),
                decision: ToolDecision::Pending,
                background: false,
            },
            ToolCall {
                agent_id: 0,
                call_id: "bg1".to_string(),
                name: "mcp_shell".to_string(),
                params: serde_json::json!({ "command": "echo background" }),
                decision: ToolDecision::Approve,
                background: true,
            },
        ]);

        // Should get both: Delegate/AwaitApproval for fg1 AND BackgroundStarted for bg1
        let mut saw_fg1_approval = false;
        let mut saw_bg1_started = false;
        let mut responder = None;
        
        // Collect a few events
        for _ in 0..5 {
            if let Some(event) = executor.next().await {
                match event {
                    ToolEvent::Delegate { call_id, effect: Effect::AwaitApproval { .. }, responder: r, .. } if call_id == "fg1" => {
                        saw_fg1_approval = true;
                        responder = Some(r);
                    }
                    ToolEvent::BackgroundStarted { call_id, .. } if call_id == "bg1" => {
                        saw_bg1_started = true;
                    }
                    ToolEvent::BackgroundCompleted { .. } => break,
                    _ => {}
                }
            }
        }
        
        assert!(saw_fg1_approval, "Should see approval request for fg1");
        assert!(saw_bg1_started, "Background should start even with pending foreground approval");
        
        // Cleanup: approve fg1 so test exits cleanly
        if let Some(r) = responder {
            let _ = r.send(Ok(None));  // Approve
        }
    }
}
