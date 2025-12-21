//! Streamable tool executor
//!
//! This module provides two execution models:
//! 1. ToolExecutor - for traditional streaming tools
//! 2. PipelineExecutor - for effect-composed tools

use crate::ide::ToolPreview;
use crate::llm::AgentId;
use crate::tools::pipeline::{Effect, EffectContext, ToolPipeline};
use crate::tools::ToolOutput;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;

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
    /// Set the agent_id for this tool call
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = agent_id;
        self
    }
}

/// Decision state for a pending tool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolDecision {
    /// Initial state, not yet requested
    #[default]
    Pending,
    /// Approval has been requested from the user
    Requested,
    /// User approved execution
    Approve,
    /// User denied execution
    Deny,
}

/// Effects that tools can request
/// These are processed by App after tool completion
#[derive(Debug, Clone)]
pub enum ToolEffect {
    /// Spawn a background agent with a task
    SpawnAgent {
        task: String,
        context: Option<String>,
    },
    /// Reload a buffer in the IDE (after file modification)
    IdeReloadBuffer {
        path: PathBuf,
    },
    /// Open/navigate to a file in the IDE
    IdeOpen {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// Show a notification to the user
    #[allow(dead_code)]
    Notify {
        message: String,
    },
}

/// Events emitted by the tool executor
#[derive(Debug)]
pub enum ToolEvent {
    /// A tool needs user approval
    AwaitingApproval(ToolCall),
    /// Streaming output from tool execution
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
        effects: Vec<ToolEffect>,
    },
}

/// Active tool execution state
struct ActiveExecution {
    agent_id: AgentId,
    call_id: String,
    stream: BoxStream<'static, ToolOutput>,
    collected_output: String,
}

/// Streamable tool executor
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

    /// Get a reference to the tool registry
    pub fn tools(&self) -> &crate::tools::ToolRegistry {
        &self.tools
    }

    /// Add tool calls to the pending queue
    pub fn enqueue(&mut self, tool_calls: Vec<ToolCall>) {
        self.pending.extend(tool_calls);
    }

    /// Get front pending tool
    pub fn front(&self) -> Option<&ToolCall> {
        self.pending.front()
    }

    /// Mark a pending tool with a decision
    pub fn decide(&mut self, call_id: &str, decision: ToolDecision) {
        tracing::debug!("ToolExecutor decision for {}: {:?}", call_id, decision);
        if let Some(tool) = self.pending.iter_mut().find(|t| t.call_id == call_id) {
            tool.decision = decision;
        }
    }

    /// Poll for the next event
    pub async fn next(&mut self) -> Option<ToolEvent> {
        loop {
            // If executing, poll the stream
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
                        // Stream ended without Done - use collected output
                        // TODO this should be unreachable
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

            // Check front of pending queue
            let tool_call = self.pending.front_mut()?;
            match tool_call.decision {
                ToolDecision::Pending => {
                    // Send approval request
                    tracing::debug!("ToolExecutor requesting approval for {}", tool_call.call_id);
                    tool_call.decision = ToolDecision::Requested;
                    return Some(ToolEvent::AwaitingApproval(tool_call.clone()));
                }
                ToolDecision::Requested => {
                    // Already sent approval request, waiting for decide()
                    return None;
                }
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
                    tracing::debug!("ToolExecutor executing approved tool {}", tool_call.call_id);
                    let tool_call = self.pending.pop_front().unwrap();
                    let tool = self.tools.get_arc(&tool_call.name);
                    let stream = tool.execute(tool_call.params.clone());

                    self.active = Some(ActiveExecution {
                        agent_id: tool_call.agent_id,
                        call_id: tool_call.call_id,
                        stream,
                        collected_output: String::new(),
                    });
                    // Loop back to poll the new stream
                    continue;
                }
            }
        }
    }
}

// ============================================================================
// Effect Pipeline Executor
// ============================================================================

/// Execution phase for a pipeline
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelinePhase {
    /// Running pre-effects (validation, preview)
    Pre,
    /// Awaiting user approval
    AwaitingApproval,
    /// Running execution effects
    Execute,
    /// Running post-effects (cleanup)
    Post,
    /// Pipeline completed
    Done,
}

/// Events emitted by the pipeline executor
#[derive(Debug)]
pub enum PipelineEvent {
    /// Pre-effects are running (IDE should show preview)
    PreEffectsStarted {
        preview: Option<ToolPreview>,
    },

    /// An IDE effect needs to be applied
    IdeEffect(IdeEffect),

    /// Pipeline needs user approval to continue
    AwaitingApproval,

    /// Streaming output from pipeline execution
    OutputDelta {
        content: String,
    },

    /// Pipeline completed (success or error)
    Completed {
        content: String,
        is_error: bool,
        post_effects: Vec<ToolEffect>,
    },
}

/// IDE-specific effects that need to be applied by the app
#[derive(Debug, Clone)]
pub enum IdeEffect {
    /// Open a file in the IDE
    Open {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },

    /// Show a preview (diff, content)
    ShowPreview {
        preview: ToolPreview,
    },

    /// Reload a buffer
    ReloadBuffer {
        path: PathBuf,
    },

    /// Close/dismiss preview
    ClosePreview,
}

/// State of an executing pipeline
pub struct PipelineExecution {
    /// The pipeline being executed
    pipeline: ToolPipeline,

    /// Current execution phase
    phase: PipelinePhase,

    /// Index into the current phase's effects
    effect_index: usize,

    /// Context for the execution
    context: EffectContext,

    /// Collected output
    output: String,

    /// Whether an error occurred
    is_error: bool,

    /// Post-effects to emit on completion (converted to ToolEffect)
    post_tool_effects: Vec<ToolEffect>,
}

impl PipelineExecution {
    /// Create a new pipeline execution
    pub fn new(pipeline: ToolPipeline, params: serde_json::Value) -> Self {
        Self {
            pipeline,
            phase: PipelinePhase::Pre,
            effect_index: 0,
            context: EffectContext::new(params),
            output: String::new(),
            is_error: false,
            post_tool_effects: vec![],
        }
    }

    /// Get the current phase
    pub fn phase(&self) -> PipelinePhase {
        self.phase
    }

    /// Resume execution after approval
    pub fn approve(&mut self) {
        if self.phase == PipelinePhase::AwaitingApproval {
            self.phase = PipelinePhase::Execute;
            self.effect_index = 0;
        }
    }

    /// Deny execution
    pub fn deny(&mut self) {
        self.output = "Denied by user".to_string();
        self.is_error = true;
        self.phase = PipelinePhase::Done;
    }

    /// Execute the next step in the pipeline
    ///
    /// Returns the next event, or None if the pipeline needs to wait
    /// (e.g., for approval) or is complete.
    pub fn step(&mut self) -> Option<PipelineEvent> {
        loop {
            match self.phase {
                PipelinePhase::Pre => {
                    if let Some(event) = self.run_pre_effects() {
                        return Some(event);
                    }
                    // Pre-effects done, move to approval
                    if self.pipeline.requires_approval {
                        self.phase = PipelinePhase::AwaitingApproval;
                        return Some(PipelineEvent::AwaitingApproval);
                    } else {
                        self.phase = PipelinePhase::Execute;
                        self.effect_index = 0;
                        continue;
                    }
                }

                PipelinePhase::AwaitingApproval => {
                    // Waiting for approve() or deny() to be called
                    return None;
                }

                PipelinePhase::Execute => {
                    if let Some(event) = self.run_execute_effects() {
                        return Some(event);
                    }
                    // Execute effects done, move to post
                    self.phase = PipelinePhase::Post;
                    self.effect_index = 0;
                    continue;
                }

                PipelinePhase::Post => {
                    if let Some(event) = self.run_post_effects() {
                        return Some(event);
                    }
                    // All done
                    self.phase = PipelinePhase::Done;
                    return Some(PipelineEvent::Completed {
                        content: std::mem::take(&mut self.output),
                        is_error: self.is_error,
                        post_effects: std::mem::take(&mut self.post_tool_effects),
                    });
                }

                PipelinePhase::Done => {
                    return None;
                }
            }
        }
    }

    /// Run pre-effects, returning an event if one should be emitted
    fn run_pre_effects(&mut self) -> Option<PipelineEvent> {
        while self.effect_index < self.pipeline.pre.len() {
            let effect = &self.pipeline.pre[self.effect_index];
            self.effect_index += 1;

            match self.interpret_effect(effect.clone()) {
                EffectInterpretation::Continue => continue,
                EffectInterpretation::Emit(event) => return Some(event),
                EffectInterpretation::Error(msg) => {
                    self.output = msg;
                    self.is_error = true;
                    self.phase = PipelinePhase::Done;
                    return Some(PipelineEvent::Completed {
                        content: std::mem::take(&mut self.output),
                        is_error: true,
                        post_effects: vec![],
                    });
                }
            }
        }
        None
    }

    /// Run execute effects, returning an event if one should be emitted
    fn run_execute_effects(&mut self) -> Option<PipelineEvent> {
        while self.effect_index < self.pipeline.execute.len() {
            let effect = &self.pipeline.execute[self.effect_index];
            self.effect_index += 1;

            match self.interpret_effect(effect.clone()) {
                EffectInterpretation::Continue => continue,
                EffectInterpretation::Emit(event) => return Some(event),
                EffectInterpretation::Error(msg) => {
                    self.output = msg;
                    self.is_error = true;
                    self.phase = PipelinePhase::Done;
                    return Some(PipelineEvent::Completed {
                        content: std::mem::take(&mut self.output),
                        is_error: true,
                        post_effects: vec![],
                    });
                }
            }
        }
        None
    }

    /// Run post effects, returning an event if one should be emitted
    fn run_post_effects(&mut self) -> Option<PipelineEvent> {
        while self.effect_index < self.pipeline.post.len() {
            let effect = &self.pipeline.post[self.effect_index];
            self.effect_index += 1;

            match self.interpret_effect(effect.clone()) {
                EffectInterpretation::Continue => continue,
                EffectInterpretation::Emit(event) => return Some(event),
                EffectInterpretation::Error(msg) => {
                    // Errors in post-effects are logged but don't fail the tool
                    tracing::warn!("Post-effect error: {}", msg);
                    continue;
                }
            }
        }
        None
    }

    /// Interpret a single effect
    fn interpret_effect(&mut self, effect: Effect) -> EffectInterpretation {
        match effect {
            // Validation effects
            Effect::ValidateParams { error } => {
                if let Some(msg) = error {
                    EffectInterpretation::Error(msg)
                } else {
                    EffectInterpretation::Continue
                }
            }

            Effect::ValidateFileExists { path } => {
                if path.exists() {
                    EffectInterpretation::Continue
                } else {
                    EffectInterpretation::Error(format!(
                        "File not found: {}",
                        path.display()
                    ))
                }
            }

            Effect::ValidateFileReadable { path } => {
                match fs::metadata(&path) {
                    Ok(m) if m.is_file() => EffectInterpretation::Continue,
                    Ok(_) => EffectInterpretation::Error(format!(
                        "Not a file: {}",
                        path.display()
                    )),
                    Err(e) => EffectInterpretation::Error(format!(
                        "Cannot read file {}: {}",
                        path.display(),
                        e
                    )),
                }
            }

            Effect::Validate { ok, error } => {
                if ok {
                    EffectInterpretation::Continue
                } else {
                    EffectInterpretation::Error(error)
                }
            }

            // IDE effects
            Effect::IdeOpen { path, line, column } => {
                EffectInterpretation::Emit(PipelineEvent::IdeEffect(IdeEffect::Open {
                    path,
                    line,
                    column,
                }))
            }

            Effect::IdeShowPreview { preview } => {
                EffectInterpretation::Emit(PipelineEvent::IdeEffect(IdeEffect::ShowPreview {
                    preview,
                }))
            }

            Effect::IdeReloadBuffer { path } => {
                // Convert to post tool effect
                self.post_tool_effects.push(ToolEffect::IdeReloadBuffer { path: path.clone() });
                EffectInterpretation::Emit(PipelineEvent::IdeEffect(IdeEffect::ReloadBuffer {
                    path,
                }))
            }

            Effect::IdeClosePreview => {
                EffectInterpretation::Emit(PipelineEvent::IdeEffect(IdeEffect::ClosePreview))
            }

            // Control flow
            Effect::AwaitApproval => {
                // This shouldn't happen in interpret - it's handled at the phase level
                EffectInterpretation::Continue
            }

            Effect::StreamDelta { content } => {
                self.output.push_str(&content);
                EffectInterpretation::Emit(PipelineEvent::OutputDelta { content })
            }

            Effect::Output { content } => {
                self.output = content;
                EffectInterpretation::Continue
            }

            Effect::Error { message } => {
                EffectInterpretation::Error(message)
            }

            // File system effects
            Effect::ReadFile { path, context_key } => {
                match fs::read_to_string(&path) {
                    Ok(content) => {
                        self.context.store(context_key, content);
                        EffectInterpretation::Continue
                    }
                    Err(e) => EffectInterpretation::Error(format!(
                        "Failed to read {}: {}",
                        path.display(),
                        e
                    )),
                }
            }

            Effect::WriteFile { path, content } => {
                match fs::write(&path, &content) {
                    Ok(()) => EffectInterpretation::Continue,
                    Err(e) => EffectInterpretation::Error(format!(
                        "Failed to write {}: {}",
                        path.display(),
                        e
                    )),
                }
            }

            // Agent effects
            Effect::SpawnAgent { task, context } => {
                self.post_tool_effects.push(ToolEffect::SpawnAgent { task, context });
                EffectInterpretation::Continue
            }

            Effect::Notify { message } => {
                self.post_tool_effects.push(ToolEffect::Notify { message });
                EffectInterpretation::Continue
            }
        }
    }
}

/// Result of interpreting an effect
enum EffectInterpretation {
    /// Continue to the next effect
    Continue,
    /// Emit an event
    Emit(PipelineEvent),
    /// Error - stop execution
    Error(String),
}
