//! Tool execution engine
//!
//! Provides the ToolExecutor for running tools and handling the
//! approval -> execute -> effects lifecycle.

use crate::llm::AgentId;
use crate::tools::pipeline::{Effect, EffectContext, ToolPipeline};
use crate::tools::ToolOutput;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::collections::VecDeque;
use std::fs;

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
    AwaitingApproval(ToolCall),
    /// Streaming output from execution
    OutputDelta {
        agent_id: AgentId,
        call_id: String,
        delta: String,
    },
    /// Tool execution completed
    Completed {
        agent_id: AgentId,
        call_id: String,
        content: String,
        is_error: bool,
        effects: Vec<Effect>,
    },
}

/// Active tool execution state
struct ActiveExecution {
    agent_id: AgentId,
    call_id: String,
    stream: BoxStream<'static, ToolOutput>,
    collected_output: String,
}

/// Executes tools with approval flow and streaming output
pub struct ToolExecutor {
    tools: crate::tools::ToolRegistry,
    pending: VecDeque<ToolCall>,
    active: Option<ActiveExecution>,
}

impl ToolExecutor {
    pub fn new(tools: crate::tools::ToolRegistry) -> Self {
        Self {
            tools,
            pending: VecDeque::new(),
            active: None,
        }
    }

    pub fn tools(&self) -> &crate::tools::ToolRegistry {
        &self.tools
    }

    pub fn enqueue(&mut self, tool_calls: Vec<ToolCall>) {
        self.pending.extend(tool_calls);
    }

    pub fn front(&self) -> Option<&ToolCall> {
        self.pending.front()
    }

    pub fn decide(&mut self, call_id: &str, decision: ToolDecision) {
        if let Some(tool) = self.pending.iter_mut().find(|t| t.call_id == call_id) {
            tool.decision = decision;
        }
    }

    pub async fn next(&mut self) -> Option<ToolEvent> {
        loop {
            if let Some(active) = &mut self.active {
                match active.stream.next().await {
                    Some(ToolOutput::Delta(delta)) => {
                        active.collected_output.push_str(&delta);
                        return Some(ToolEvent::OutputDelta {
                            agent_id: active.agent_id,
                            call_id: active.call_id.clone(),
                            delta,
                        });
                    }
                    Some(ToolOutput::Done(result)) => {
                        let active = self.active.take().unwrap();
                        return Some(ToolEvent::Completed {
                            agent_id: active.agent_id,
                            call_id: active.call_id,
                            content: result.content,
                            is_error: result.is_error,
                            effects: result.effects,
                        });
                    }
                    None => {
                        let active = self.active.take().unwrap();
                        return Some(ToolEvent::Completed {
                            agent_id: active.agent_id,
                            call_id: active.call_id,
                            content: active.collected_output,
                            is_error: false,
                            effects: vec![],
                        });
                    }
                }
            }

            let tool_call = self.pending.front_mut()?;
            match tool_call.decision {
                ToolDecision::Pending => {
                    tool_call.decision = ToolDecision::Requested;
                    return Some(ToolEvent::AwaitingApproval(tool_call.clone()));
                }
                ToolDecision::Requested => return None,
                ToolDecision::Deny => {
                    let tool_call = self.pending.pop_front().unwrap();
                    return Some(ToolEvent::Completed {
                        agent_id: tool_call.agent_id,
                        call_id: tool_call.call_id,
                        content: "Denied by user".to_string(),
                        is_error: true,
                        effects: vec![],
                    });
                }
                ToolDecision::Approve => {
                    let tool_call = self.pending.pop_front().unwrap();
                    let tool = self.tools.get_arc(&tool_call.name);
                    let stream = tool.execute(tool_call.params.clone());
                    self.active = Some(ActiveExecution {
                        agent_id: tool_call.agent_id,
                        call_id: tool_call.call_id,
                        stream,
                        collected_output: String::new(),
                    });
                    continue;
                }
            }
        }
    }
}

// ============================================================================
// Pipeline Execution
// ============================================================================

/// Execution phase for a pipeline
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelinePhase {
    Running,
    AwaitingApproval,
    Done,
}

/// Events emitted during pipeline execution
#[derive(Debug)]
pub enum PipelineEvent {
    /// An effect needs to be applied
    Effect(Effect),
    /// Pipeline needs user approval
    AwaitingApproval,
    /// Streaming output
    OutputDelta { content: String },
    /// Pipeline completed
    Completed {
        content: String,
        is_error: bool,
        /// Effects that follow the last executed effect (for post-completion handling)
        remaining_effects: Vec<Effect>,
    },
}

/// Executes a pipeline of effects
pub struct PipelineExecution {
    effects: Vec<Effect>,
    index: usize,
    phase: PipelinePhase,
    context: EffectContext,
    output: String,
    is_error: bool,
}

impl PipelineExecution {
    pub fn new(pipeline: ToolPipeline, params: serde_json::Value) -> Self {
        Self {
            effects: pipeline.effects,
            index: 0,
            phase: PipelinePhase::Running,
            context: EffectContext::new(params),
            output: String::new(),
            is_error: false,
        }
    }

    pub fn phase(&self) -> PipelinePhase {
        self.phase
    }

    pub fn approve(&mut self) {
        if self.phase == PipelinePhase::AwaitingApproval {
            self.phase = PipelinePhase::Running;
        }
    }

    pub fn deny(&mut self) {
        self.output = "Denied by user".to_string();
        self.is_error = true;
        self.phase = PipelinePhase::Done;
    }

    pub fn step(&mut self) -> Option<PipelineEvent> {
        if self.phase == PipelinePhase::AwaitingApproval {
            return None;
        }
        if self.phase == PipelinePhase::Done {
            return None;
        }

        while self.index < self.effects.len() {
            let effect = self.effects[self.index].clone();
            self.index += 1;

            match self.interpret(effect) {
                Interpretation::Continue => continue,
                Interpretation::Emit(event) => return Some(event),
                Interpretation::Suspend => {
                    self.phase = PipelinePhase::AwaitingApproval;
                    return Some(PipelineEvent::AwaitingApproval);
                }
                Interpretation::Error(msg) => {
                    self.output = msg;
                    self.is_error = true;
                    self.phase = PipelinePhase::Done;
                    return Some(PipelineEvent::Completed {
                        content: std::mem::take(&mut self.output),
                        is_error: true,
                        remaining_effects: vec![],
                    });
                }
            }
        }

        // All effects processed
        self.phase = PipelinePhase::Done;
        Some(PipelineEvent::Completed {
            content: std::mem::take(&mut self.output),
            is_error: self.is_error,
            remaining_effects: vec![], // Could collect remaining if we want
        })
    }

    fn interpret(&mut self, effect: Effect) -> Interpretation {
        match effect {
            // Validation
            Effect::ValidateParams { error } => {
                error.map_or(Interpretation::Continue, Interpretation::Error)
            }
            Effect::ValidateFileExists { ref path } => {
                if path.exists() {
                    Interpretation::Continue
                } else {
                    Interpretation::Error(format!("File not found: {}", path.display()))
                }
            }
            Effect::ValidateFileReadable { ref path } => match fs::metadata(path) {
                Ok(m) if m.is_file() => Interpretation::Continue,
                Ok(_) => Interpretation::Error(format!("Not a file: {}", path.display())),
                Err(e) => Interpretation::Error(format!("Cannot read {}: {}", path.display(), e)),
            },
            Effect::Validate { ok, error } => {
                if ok { Interpretation::Continue } else { Interpretation::Error(error) }
            }

            // IDE effects - emit for app to handle
            Effect::IdeOpen { .. }
            | Effect::IdeShowPreview { .. }
            | Effect::IdeReloadBuffer { .. }
            | Effect::IdeClosePreview => Interpretation::Emit(PipelineEvent::Effect(effect)),

            // Control flow
            Effect::AwaitApproval => Interpretation::Suspend,
            Effect::StreamDelta { content } => {
                self.output.push_str(&content);
                Interpretation::Emit(PipelineEvent::OutputDelta { content })
            }
            Effect::Output { content } => {
                self.output = content;
                Interpretation::Continue
            }
            Effect::Error { message } => Interpretation::Error(message),

            // File system
            Effect::ReadFile { ref path, ref context_key } => match fs::read_to_string(path) {
                Ok(content) => {
                    self.context.store(context_key.clone(), content);
                    Interpretation::Continue
                }
                Err(e) => Interpretation::Error(format!("Failed to read {}: {}", path.display(), e)),
            },
            Effect::WriteFile { ref path, ref content } => match fs::write(path, content) {
                Ok(()) => Interpretation::Continue,
                Err(e) => Interpretation::Error(format!("Failed to write {}: {}", path.display(), e)),
            },

            // Agent/notification effects - emit for app to handle
            Effect::SpawnAgent { .. } | Effect::Notify { .. } => {
                Interpretation::Emit(PipelineEvent::Effect(effect))
            }
        }
    }
}

enum Interpretation {
    Continue,
    Emit(PipelineEvent),
    Suspend,
    Error(String),
}
