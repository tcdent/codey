//! rockem-backed agent: Codey's own agent loop over rockem's API (`rockem-client`).
//!
//! The service holds no state, so this agent keeps the conversation and sends all of
//! it with every request; the node reuses the attention rows of the unchanged front,
//! so only the new tail is computed. A turn that ends in calls surfaces them as one
//! `AgentStep::ToolRequest`; once every result is in, the assistant message and its
//! results join the conversation and the next request goes out.
//!
//! `next()` is cancel-safe: the state, the request being sent and the open stream
//! live on `self`, so a dropped poll resumes where it stopped. Dropping the stream closes the connection,
//! which aborts the generation and frees the node.
#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{debug, info, warn};

use rockem_client::{Client, Event, Events, Message, Reason, Request, StreamExt, Tool};

use super::agent::{Activity, AgentStep, Phase, RequestMode, SystemPromptBuilder, Usage};
use crate::config::AgentRuntimeConfig;
use crate::tools::{ToolCall, ToolRegistry};
use crate::transcript::{BlockType, Role, Status, Transcript};

/// A request on its way out, until the service answers with a stream or a refusal.
type Sending = Pin<Box<dyn Future<Output = Result<Events, rockem_client::Error>> + Send>>;

enum State {
    /// The next request is ready to go out.
    NeedsRequest,
    /// The request is out; waiting for the stream to open (queueing, prefill).
    Sending(Sending),
    /// Reading the open stream.
    Streaming,
    /// Calls were surfaced; waiting for every result.
    AwaitingTools,
}

pub struct RockemAgent {
    client: Client,
    config: AgentRuntimeConfig,
    tools: Vec<Tool>,
    /// The whole conversation, system prompt first.
    messages: Vec<Message>,
    system_prompt_builder: Option<SystemPromptBuilder>,
    total_usage: Usage,

    state: Option<State>,
    stream: Option<Events>,
    /// This request's reasoning, text and calls, joined into the conversation when it
    /// ends. The reasoning goes back as it came, so the turn renders as it was
    /// generated and the node's cache matches past it.
    thinking: String,
    text: String,
    calls: Vec<rockem_client::ToolCall>,
    /// Results in arrival order, keyed by call id.
    results: Vec<(String, String)>,

    /// Whether a node has taken this request (`started`): queued before, prefill after.
    started: bool,
    /// When this request's first token arrived, after the prompt.
    first_event: Option<Instant>,
    /// Whether the latest tokens were reasoning.
    in_thinking: bool,
    /// Tokens generated after each request's first, and the time they took, summed
    /// over the turn's requests: the turn's generation rate without its prompts.
    turn_tokens: u32,
    turn_time: Duration,
    /// When the turn began, and the ids generated in it so far, for the readout.
    turn_started: Instant,
    turn_ids: u32,
}

impl RockemAgent {
    pub fn new(config: AgentRuntimeConfig, url: &str, system_prompt: &str, tools: ToolRegistry) -> Self {
        Self::build(config, url, system_prompt.to_string(), None, tools)
    }

    pub fn with_dynamic_prompt(
        config: AgentRuntimeConfig,
        url: &str,
        prompt_builder: SystemPromptBuilder,
        tools: ToolRegistry,
    ) -> Self {
        let system_prompt = prompt_builder();
        Self::build(config, url, system_prompt, Some(prompt_builder), tools)
    }

    fn build(
        config: AgentRuntimeConfig,
        url: &str,
        system_prompt: String,
        system_prompt_builder: Option<SystemPromptBuilder>,
        tools: ToolRegistry,
    ) -> Self {
        let tools = tools
            .values()
            .map(|t| Tool {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.schema(),
            })
            .collect();
        Self {
            client: Client::new(url),
            config,
            tools,
            messages: vec![Message::system(system_prompt)],
            system_prompt_builder,
            total_usage: Usage::default(),
            state: None,
            stream: None,
            thinking: String::new(),
            text: String::new(),
            calls: Vec::new(),
            results: Vec::new(),
            started: false,
            first_event: None,
            in_thinking: false,
            turn_tokens: 0,
            turn_time: Duration::ZERO,
            turn_started: Instant::now(),
            turn_ids: 0,
        }
    }

    pub fn total_usage(&self) -> Usage {
        self.total_usage
    }

    pub fn last_message(&self) -> Option<String> {
        if self.text.is_empty() {
            None
        } else {
            Some(self.text.clone())
        }
    }

    /// Rebuild the conversation from a saved transcript (`--continue`), in the shape
    /// it was sent: the service holds nothing, so this is the whole of it.
    ///
    /// A transcript turn is one assistant turn for the whole agent loop; the
    /// conversation has one assistant message per request, each followed by its
    /// results. A request's blocks run reasoning, text, calls, so a reasoning or text
    /// block after a call begins the next one. Rebuilt the same, the node's cache
    /// matches it. Pending blocks were never sent (the welcome message), and a call
    /// with no result (quit while it ran) is left out with it.
    pub fn restore_from_transcript(&mut self, transcript: &Transcript) {
        self.messages.truncate(1);
        for turn in transcript.turns() {
            let blocks = turn.content.iter().filter(|b| b.status() != Status::Pending);
            match turn.role {
                Role::System => {},
                Role::User => {
                    let text: Vec<&str> =
                        blocks.filter(|b| b.kind() == BlockType::Text).filter_map(|b| b.text()).collect();
                    if !text.is_empty() {
                        self.messages.push(Message::user(text.join("\n\n")));
                    }
                },
                Role::Assistant => {
                    let mut request = Restored::default();
                    for block in blocks {
                        match block.kind() {
                            BlockType::Thinking | BlockType::Text if !request.calls.is_empty() => {
                                request.push_into(&mut self.messages);
                            },
                            _ => {},
                        }
                        match block.kind() {
                            BlockType::Thinking => request.thinking.push_str(block.text().unwrap_or_default()),
                            BlockType::Text => request.text.push_str(block.text().unwrap_or_default()),
                            BlockType::Tool => {
                                if let (Some(id), Some(name), Some(params), Some(result)) =
                                    (block.call_id(), block.tool_name(), block.params(), block.text())
                                {
                                    request.calls.push(rockem_client::ToolCall {
                                        id: id.to_string(),
                                        name: name.to_string(),
                                        arguments: params.as_object().cloned().unwrap_or_default(),
                                        ids: Vec::new(),
                                    });
                                    request.results.push(result.to_string());
                                }
                            },
                        }
                    }
                    request.push_into(&mut self.messages);
                },
            }
        }
        info!("Restored {} messages from transcript", self.messages.len() - 1);
    }

    /// The running turn, read off the loop's state; `None` between turns.
    pub fn activity(&self) -> Option<Activity> {
        let phase = match self.state.as_ref()? {
            State::NeedsRequest | State::Sending(_) => Phase::Queued,
            State::Streaming if !self.started => Phase::Queued,
            State::Streaming if self.first_event.is_none() => Phase::Prefill,
            State::Streaming if self.in_thinking => Phase::Thinking,
            State::Streaming => Phase::Writing,
            State::AwaitingTools => Phase::Tools,
        };
        Some(Activity { started: self.turn_started, phase, tokens: Some(self.turn_ids) })
    }

    pub fn send_request(&mut self, user_input: &str, _mode: RequestMode) {
        self.messages.push(Message::user(user_input));
        self.turn_tokens = 0;
        self.turn_time = Duration::ZERO;
        self.turn_started = Instant::now();
        self.turn_ids = 0;
        self.state = Some(State::NeedsRequest);
    }

    /// Stop the turn. Text generated so far stays in the conversation; calls not yet
    /// answered are dropped with it, since the conversation can't carry a call
    /// without its result.
    pub fn cancel(&mut self) {
        debug!("RockemAgent::cancel");
        self.end_turn();
    }

    /// End the turn where it stands (cancelled, or the stream broke): what was
    /// generated stays in the conversation, as the transcript shows it.
    fn end_turn(&mut self) {
        self.stream = None;
        self.state = None;
        self.calls.clear();
        self.results.clear();
        if !self.text.is_empty() || !self.thinking.is_empty() {
            self.messages.push(self.assistant_message(Vec::new()));
        }
    }

    pub async fn next(&mut self) -> Option<AgentStep> {
        loop {
            match self.state.as_ref()? {
                State::NeedsRequest => {
                    self.refresh_system_prompt();
                    self.thinking.clear();
                    self.text.clear();
                    self.calls.clear();
                    self.results.clear();
                    self.started = false;
                    self.first_event = None;
                    self.in_thinking = false;
                    let request = Request::chat(self.messages.clone())
                        .tools(self.tools.clone())
                        .max_tokens(self.config.max_tokens);
                    let client = self.client.clone();
                    self.state = Some(State::Sending(Box::pin(async move { client.generate(&request).await })));
                },

                // The send lives on `self`, so a dropped poll resumes it instead of
                // sending the request again.
                State::Sending(_) => {
                    let Some(State::Sending(sending)) = self.state.as_mut() else { unreachable!() };
                    match sending.await {
                        Ok(stream) => {
                            self.stream = Some(stream);
                            self.state = Some(State::Streaming);
                        },
                        Err(e) => {
                            self.state = None;
                            return Some(AgentStep::Error(e.to_string()));
                        },
                    }
                },

                State::Streaming => {
                    let stream = self.stream.as_mut()?;
                    let event = stream.next().await;
                    let token = matches!(
                        event,
                        Some(Ok(Event::Thinking { .. } | Event::Text { .. } | Event::ToolCall(_)))
                    );
                    if token && self.first_event.is_none() {
                        self.first_event = Some(Instant::now());
                    }
                    match event {
                        Some(Ok(Event::Started { .. })) => self.started = true,
                        Some(Ok(Event::Thinking { text, ids })) => {
                            self.started = true;
                            self.in_thinking = true;
                            self.turn_ids += ids.len() as u32;
                            if !text.is_empty() {
                                self.thinking.push_str(&text);
                                return Some(AgentStep::ThinkingDelta(text));
                            }
                        },
                        Some(Ok(Event::Text { text, ids })) => {
                            self.started = true;
                            self.in_thinking = false;
                            self.turn_ids += ids.len() as u32;
                            if !text.is_empty() {
                                self.text.push_str(&text);
                                return Some(AgentStep::TextDelta(text));
                            }
                        },
                        Some(Ok(Event::ToolCall(call))) => {
                            self.started = true;
                            self.in_thinking = false;
                            self.turn_ids += call.ids.len() as u32;
                            self.calls.push(call);
                        },
                        Some(Ok(Event::Done(done))) => {
                            self.stream = None;
                            if let Some(first) = self.first_event {
                                self.turn_tokens += done.usage.completion_tokens.saturating_sub(1);
                                self.turn_time += first.elapsed();
                            }
                            let secs = self.turn_time.as_secs_f64();
                            self.total_usage += Usage {
                                output_tokens: done.usage.completion_tokens,
                                context_tokens: done.usage.prompt_tokens + done.usage.completion_tokens,
                                tokens_per_second: if secs > 0.0 {
                                    (self.turn_tokens as f64 / secs).round() as u32
                                } else {
                                    0
                                },
                                ..Usage::default()
                            };
                            info!(
                                "rockem: {} {:?}, {} prompt + {} completion of {} positions",
                                done.model,
                                done.reason,
                                done.usage.prompt_tokens,
                                done.usage.completion_tokens,
                                done.usage.positions
                            );

                            if done.reason == Reason::ContextFull {
                                self.state = None;
                                return Some(AgentStep::Error(format!(
                                    "rockem: context full ({} positions)",
                                    done.usage.positions
                                )));
                            }
                            if !self.calls.is_empty() {
                                self.state = Some(State::AwaitingTools);
                                return Some(AgentStep::ToolRequest(
                                    self.calls.iter().map(to_tool_call).collect(),
                                ));
                            }
                            self.messages.push(self.assistant_message(Vec::new()));
                            self.state = None;
                            return Some(AgentStep::Finished { usage: self.total_usage });
                        },
                        Some(Err(e)) => {
                            self.end_turn();
                            return Some(AgentStep::Error(e.to_string()));
                        },
                        None => {
                            self.end_turn();
                            return Some(AgentStep::Error("rockem: stream ended without done".into()));
                        },
                        // Event types newer than this client knows.
                        Some(Ok(_)) => {},
                    }
                },

                State::AwaitingTools => return None,
            }
        }
    }

    pub fn submit_tool_result(&mut self, call_id: &str, content: String) {
        if !matches!(self.state, Some(State::AwaitingTools)) {
            warn!("submit_tool_result: no calls pending (call_id={call_id})");
            return;
        }
        self.results.push((call_id.to_string(), content));
        if self.results.len() < self.calls.len() {
            return;
        }

        // Every call answered: the turn and its results join the conversation, in
        // the order the calls were made.
        let calls = std::mem::take(&mut self.calls);
        let mut results = std::mem::take(&mut self.results);
        self.messages.push(self.assistant_message(calls.clone()));
        for call in &calls {
            let content = results
                .iter()
                .position(|(id, _)| *id == call.id)
                .map(|i| results.swap_remove(i).1)
                .unwrap_or_default();
            self.messages.push(Message::tool_result(call.id.clone(), content));
        }
        self.state = Some(State::NeedsRequest);
    }

    /// This request's assistant turn as it was generated: reasoning, text and calls.
    fn assistant_message(&self, calls: Vec<rockem_client::ToolCall>) -> Message {
        Message::assistant(self.text.clone(), calls).with_thinking(self.thinking.clone())
    }

    fn refresh_system_prompt(&mut self) {
        if let Some(builder) = &self.system_prompt_builder {
            let prompt = builder();
            if self.messages[0].content != prompt {
                // A changed system prompt recomputes everything after it on the node.
                debug!("System prompt changed");
                self.messages[0] = Message::system(prompt);
            }
        }
    }
}

/// One request's worth of a saved assistant turn, being rebuilt.
#[derive(Default)]
struct Restored {
    thinking: String,
    text: String,
    calls: Vec<rockem_client::ToolCall>,
    results: Vec<String>,
}

impl Restored {
    /// Append the request's assistant message and its results; start the next.
    fn push_into(&mut self, messages: &mut Vec<Message>) {
        let request = std::mem::take(self);
        if request.thinking.is_empty() && request.text.is_empty() && request.calls.is_empty() {
            return;
        }
        let ids: Vec<String> = request.calls.iter().map(|c| c.id.clone()).collect();
        messages.push(Message::assistant(request.text, request.calls).with_thinking(request.thinking));
        for (id, result) in ids.into_iter().zip(request.results) {
            messages.push(Message::tool_result(id, result));
        }
    }
}

/// A call as Codey's tool flow takes it; `background` is Codey's own flag, not the tool's.
fn to_tool_call(call: &rockem_client::ToolCall) -> ToolCall {
    let mut params = Value::Object(call.arguments.clone());
    let background = params
        .as_object_mut()
        .and_then(|obj| obj.remove("background"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ToolCall {
        agent_id: 0, // set by App when handling ToolRequest
        call_id: call.id.clone(),
        name: call.name.clone(),
        params,
        background,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Block, TextBlock, ThinkingBlock, ToolBlock};
    use rockem_client::Role as Wire;
    use serde_json::json;

    fn done(mut block: impl Block + 'static) -> Box<dyn Block> {
        block.set_status(Status::Complete);
        Box::new(block)
    }

    fn tool(id: &str, path: &str, result: Option<&str>) -> Box<dyn Block> {
        let mut block = ToolBlock::new(id, "read_file", json!({ "path": path }), false);
        if let Some(result) = result {
            block.append_text(result);
            block.set_status(Status::Complete);
        }
        Box::new(block)
    }

    /// The transcript the app keeps for: welcome; a question; one agent turn of
    /// two requests (reasoning, text, a call; then reasoning, text); a follow-up,
    /// answered with a call the user quit on.
    fn transcript() -> Transcript {
        let mut t = Transcript::with_path(std::env::temp_dir().join("codey-restore-test.json"));
        t.add_turn(Role::Assistant, TextBlock::pending("Welcome to Codey!"));
        let user = t.add_empty(Role::User);
        t.get_mut(user).unwrap().add_block(done(TextBlock::new("What is in main.rs?")));
        let reply = t.add_empty(Role::Assistant);
        for block in [
            done(ThinkingBlock::new("Read it.")),
            done(TextBlock::new("Let me look.")),
            tool("call_1", "src/main.rs", Some("fn main() {}\n")),
            done(ThinkingBlock::new("It is empty.")),
            done(TextBlock::new("An empty main.")),
        ] {
            t.get_mut(reply).unwrap().add_block(block);
        }
        let user = t.add_empty(Role::User);
        t.get_mut(user).unwrap().add_block(done(TextBlock::new("And lib.rs?")));
        let reply = t.add_empty(Role::Assistant);
        t.get_mut(reply).unwrap().add_block(tool("call_2", "src/lib.rs", None));
        t
    }

    #[test]
    fn restore_rebuilds_the_conversation_as_sent() {
        let mut agent = RockemAgent::new(AgentRuntimeConfig::default(), "http://unused", "system", ToolRegistry::empty());
        agent.restore_from_transcript(&transcript());
        let m = &agent.messages;

        let shape: Vec<(Wire, &str, &str, usize)> =
            m.iter().map(|m| (m.role, m.content.as_str(), m.thinking.as_str(), m.tool_calls.len())).collect();
        assert_eq!(
            shape,
            [
                (Wire::System, "system", "", 0),
                (Wire::User, "What is in main.rs?", "", 0),
                (Wire::Assistant, "Let me look.", "Read it.", 1),
                (Wire::Tool, "fn main() {}\n", "", 0),
                (Wire::Assistant, "An empty main.", "It is empty.", 0),
                (Wire::User, "And lib.rs?", "", 0),
            ]
        );
        assert_eq!(m[2].tool_calls[0].id, "call_1");
        assert_eq!(m[2].tool_calls[0].arguments["path"], "src/main.rs");
        assert_eq!(m[3].tool_call_id.as_deref(), Some("call_1"));
    }
}
