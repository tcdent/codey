//! Tool execution engine
//!
//! Executes tool pipelines with approval flow and streaming output.

use std::collections::VecDeque;

use tokio::sync::oneshot;

use crate::llm::AgentId;
use crate::tools::pipeline::{Effect, Step, ToolPipeline};
use crate::tools::ToolRegistry;

/// Result of executing a delegated effect
pub type EffectResult = Result<(), String>;

/// A tool call pending execution
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub agent_id: AgentId,
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub decision: ToolDecision,
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
    /// Tool execution completed
    Completed {
        agent_id: AgentId,
        call_id: String,
        content: String,
        is_error: bool,
    },
}

impl ToolEvent {
    fn completed(active: ActivePipeline) -> Self {
        Self::Completed {
            agent_id: active.agent_id,
            call_id: active.call_id,
            content: active.output,
            is_error: false,
        }
    }

    fn error(active: ActivePipeline, content: impl Into<String>) -> Self {
        Self::Completed {
            agent_id: active.agent_id,
            call_id: active.call_id,
            content: content.into(),
            is_error: true,
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
}

impl ActivePipeline {
    fn new(tool_call: ToolCall, pipeline: ToolPipeline) -> Self {
        Self {
            agent_id: tool_call.agent_id,
            call_id: tool_call.call_id,
            name: tool_call.name,
            params: tool_call.params,
            pipeline,
            output: String::new(),
            pending_effect: None,
            pending_approval: None,
        }
    }
}

/// Executes tools with approval flow and streaming output.
///
/// TODO: Currently executes tools sequentially with a single active pipeline.
/// This executor is shared between all agents, so parallel tool execution
/// is not supported. Consider per-agent executors or a concurrent model
/// if parallel execution is needed.
pub struct ToolExecutor {
    tools: ToolRegistry,
    pending: VecDeque<ToolCall>,
    active: Option<ActivePipeline>,
}

impl ToolExecutor {
    pub fn new(tools: ToolRegistry) -> Self {
        Self {
            tools,
            pending: VecDeque::new(),
            active: None,
        }
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn enqueue(&mut self, tool_calls: Vec<ToolCall>) {
        self.pending.extend(tool_calls);
    }

    pub async fn next(&mut self) -> Option<ToolEvent> {
        // Check if active pipeline is waiting for effect or approval
        if let Some(event) = self.check_pending_effect().await {
            return Some(event);
        }
        if let Some(event) = self.check_pending_approval().await {
            return Some(event);
        }

        // If no active pipeline, start next pending tool
        if self.active.is_none() {
            let tool_call = match self.pending.front() {
                Some(t) if t.decision == ToolDecision::Pending => self.pending.pop_front().unwrap(),
                _ => return None,
            };

            let tool = self.tools.get(&tool_call.name);
            let pipeline = tool.compose(tool_call.params.clone());
            self.active = Some(ActivePipeline::new(tool_call, pipeline));
        }

        // Execute pipeline steps until an event
        loop {
            if let Some(event) = self.execute_step().await {
                return Some(event);
            }
        }
    }

    /// Await pending effect result, return error event if it failed
    async fn check_pending_effect(&mut self) -> Option<ToolEvent> {
        let active = self.active.as_mut()?;
        let rx = active.pending_effect.take()?;

        match rx.await {
            Ok(Ok(())) => None,
            Ok(Err(msg)) => {
                let active = self.active.take().unwrap();
                Some(ToolEvent::error(active, msg))
            },
            Err(_) => {
                let active = self.active.take().unwrap();
                Some(ToolEvent::error(active, "Effect channel dropped"))
            },
        }
    }

    /// Await pending approval decision, return error event if denied
    async fn check_pending_approval(&mut self) -> Option<ToolEvent> {
        let active = self.active.as_mut()?;
        let rx = active.pending_approval.take()?;

        match rx.await {
            Ok(ToolDecision::Approve) => None,
            Ok(ToolDecision::Deny) => {
                let active = self.active.take().unwrap();
                Some(ToolEvent::error(active, "Denied by user"))
            },
            Ok(_) => None, // Pending/Requested shouldn't happen, treat as continue
            Err(_) => {
                let active = self.active.take().unwrap();
                Some(ToolEvent::error(active, "Approval channel dropped"))
            },
        }
    }

    /// Execute next handler in active pipeline
    async fn execute_step(&mut self) -> Option<ToolEvent> {
        let active = self.active.as_mut().unwrap();

        // Pop next handler
        let handler = match active.pipeline.pop() {
            Some(h) => h,
            None => {
                // Pipeline complete
                let active = self.active.take().unwrap();
                return Some(ToolEvent::completed(active));
            },
        };

        // Execute handler
        let step = handler.call().await;

        // Re-borrow after await
        let active = self.active.as_mut().unwrap();
        match step {
            Step::Continue => None,
            Step::Output(content) => {
                active.output = content;
                None
            },
            Step::Delta(content) => {
                Some(ToolEvent::delta(active, content)) //
            },
            Step::Delegate(effect) => {
                let (event, rx) = ToolEvent::delegate(active, effect);
                active.pending_effect = Some(rx);
                Some(event)
            },
            Step::AwaitApproval => {
                let (event, rx) = ToolEvent::awaiting_approval(active);
                active.pending_approval = Some(rx);
                Some(event)
            },
            Step::Error(msg) => {
                let active = self.active.take().unwrap();
                Some(ToolEvent::error(active, msg))
            },
        }
    }
}
