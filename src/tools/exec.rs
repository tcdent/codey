//! Tool execution engine
//!
//! Executes tool pipelines with approval flow and streaming output.

use std::collections::{HashMap, VecDeque};

use tokio::sync::oneshot;

use crate::llm::AgentId;
use crate::transcript::Status;
use crate::tools::pipeline::{Effect, Step, ToolPipeline};
use crate::tools::ToolRegistry;

/// Result of executing a delegated effect
/// Ok(None) = success, continue pipeline
/// Ok(Some(output)) = success, set this as pipeline output
/// Err(msg) = failure, abort pipeline
pub type EffectResult = Result<Option<String>, String>;

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
    /// Tool needs user approval
    AwaitingApproval {
        agent_id: AgentId,
        call_id: String,
        name: String,
        params: serde_json::Value,
        /// If true, tool will run in background after approval
        background: bool,
        /// Send decision back to executor
        responder: oneshot::Sender<ToolDecision>,
    },
    /// Effect delegated to app (IDE, agents, etc)
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

    fn awaiting_approval(active: &ActivePipeline) -> (Self, oneshot::Receiver<ToolDecision>) {
        let (tx, rx) = oneshot::channel();
        (
            Self::AwaitingApproval {
                agent_id: active.agent_id,
                call_id: active.call_id.clone(),
                name: active.name.clone(),
                params: active.params.clone(),
                background: active.background,
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
    /// Waiting for effect result from app layer
    pending_effect: Option<oneshot::Receiver<EffectResult>>,
    /// Waiting for approval decision from app layer
    pending_approval: Option<oneshot::Receiver<ToolDecision>>,
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
            pending_effect: None,
            pending_approval: None,
            status: Status::Running,
        }
    }
    
    /// Check if pipeline is waiting for something
    fn is_waiting(&self) -> bool {
        self.pending_effect.is_some() || self.pending_approval.is_some()
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
        self.pending_effect = None;
        self.pending_approval = None;
    }
    
    /// Transition to complete state
    fn set_complete(&mut self) {
        self.status = Status::Complete;
        self.pending_effect = None;
        self.pending_approval = None;
    }
    
    /// Transition to denied state
    fn set_denied(&mut self) {
        self.status = Status::Denied;
        self.pending_effect = None;
        self.pending_approval = None;
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

    pub async fn next(&mut self) -> Option<ToolEvent> {
        // Check for cancellation
        if self.cancelled {
            self.cancelled = false;
            return None;
        }

        // Start ALL pending tools (don't emit BackgroundStarted yet - wait for approval)
        while let Some(tool_call) = self.pending.front() {
            if tool_call.decision != ToolDecision::Pending && tool_call.decision != ToolDecision::Approve {
                break;
            }
            let tool_call = self.pending.pop_front().unwrap();
            let call_id = tool_call.call_id.clone();
            
            let tool = self.tools.get(&tool_call.name);
            let pipeline = tool.compose(tool_call.params.clone());
            self.active.insert(call_id.clone(), ActivePipeline::new(tool_call, pipeline));
        }

        // Poll all active pipelines for pending effects/approvals
        let call_ids: Vec<String> = self.active.keys().cloned().collect();
        for call_id in &call_ids {
            // Check pending effect
            if let Some(event) = self.check_pending_effect(call_id) {
                return Some(event);
            }
            // Check pending approval
            if let Some(event) = self.check_pending_approval(call_id) {
                return Some(event);
            }
        }

        // Execute steps on non-waiting pipelines until we get an event
        // This mirrors the original behavior where we'd loop until an event was produced
        loop {
            let call_ids: Vec<String> = self.active.keys().cloned().collect();
            let mut any_stepped = false;
            
            for call_id in call_ids {
                let should_step = {
                    match self.active.get(&call_id) {
                        Some(active) => !active.is_waiting() && active.status == Status::Running,
                        None => false,
                    }
                };
                
                if should_step {
                    any_stepped = true;
                    if let Some(event) = self.execute_step(&call_id).await {
                        return Some(event);
                    }
                }
            }
            
            // If no pipelines were stepped (all waiting or complete), we're done
            if !any_stepped {
                break;
            }
        }
        
        None
    }

    /// Check pending effect for a specific pipeline
    fn check_pending_effect(&mut self, call_id: &str) -> Option<ToolEvent> {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, Wake, Waker};
        use std::sync::Arc;
        
        // Noop waker for polling
        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }
        
        let active = self.active.get_mut(call_id)?;
        let rx = active.pending_effect.as_mut()?;
        
        let waker = Waker::from(Arc::new(NoopWake));
        let mut cx = Context::from_waker(&waker);
        
        match Pin::new(rx).poll(&mut cx) {
            Poll::Ready(Ok(Ok(None))) => {
                // Effect completed, no output - continue pipeline
                active.pending_effect = None;
                None
            },
            Poll::Ready(Ok(Ok(Some(output)))) => {
                // Effect completed with output - inject into pipeline
                active.pending_effect = None;
                active.output = output;
                None
            },
            Poll::Ready(Ok(Err(msg))) => {
                let mut active = self.active.remove(call_id).unwrap();
                if active.background {
                    // Background task error - keep for retrieval
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
            Poll::Pending => None,
        }
    }

    /// Check pending approval for a specific pipeline
    fn check_pending_approval(&mut self, call_id: &str) -> Option<ToolEvent> {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll, Wake, Waker};
        use std::sync::Arc;
        
        // Noop waker for polling
        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }
        
        let active = self.active.get_mut(call_id)?;
        let rx = active.pending_approval.as_mut()?;
        
        let waker = Waker::from(Arc::new(NoopWake));
        let mut cx = Context::from_waker(&waker);
        
        let poll_result = Pin::new(rx).poll(&mut cx);
        tracing::debug!("check_pending_approval: poll_result={:?}", poll_result);
        
        match poll_result {
            Poll::Ready(Ok(ToolDecision::Approve)) => {
                tracing::debug!("check_pending_approval: Approved");
                active.pending_approval = None;
                // For background tools, emit BackgroundStarted now that approval is granted
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
            Poll::Ready(Ok(ToolDecision::Deny)) => {
                tracing::debug!("check_pending_approval: Denied - emitting error and continuing to finally effects");
                active.pipeline.skip_to_finally();
                active.set_denied();
                // Emit error but keep pipeline active to drain finally effects
                Some(ToolEvent::Error {
                    agent_id: active.agent_id,
                    call_id: active.call_id.clone(),
                    content: "Denied by user".to_string(),
                })
            },
            Poll::Ready(Ok(_)) => {
                // Pending/Requested shouldn't happen
                tracing::debug!("check_pending_approval: unexpected decision");
                active.pending_approval = None;
                None
            },
            Poll::Ready(Err(_)) => {
                tracing::debug!("check_pending_approval: channel error");
                // Sender dropped - treat as cancellation
                let active = self.active.remove(call_id).unwrap();
                Some(ToolEvent::error(active, "Approval cancelled"))
            },
            Poll::Pending => {
                tracing::debug!("check_pending_approval: Pending");
                None
            },
        }
    }

    /// Execute next handler in a specific pipeline
    async fn execute_step(&mut self, call_id: &str) -> Option<ToolEvent> {
        // Get the handler to execute
        let handler = {
            let active = self.active.get_mut(call_id)?;
            match active.pipeline.pop() {
                Some(h) => h,
                None => {
                    // Pipeline complete
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

        // Execute handler (outside borrow)
        let step = handler.call().await;

        // Re-borrow after await
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
                active.pending_effect = Some(rx);
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
                    let (event, rx) = ToolEvent::awaiting_approval(active);
                    active.pending_approval = Some(rx);
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
