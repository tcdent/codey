//! Tool execution engine
//!
//! Executes tool pipelines with approval flow and streaming output.

use crate::llm::AgentId;
use crate::tools::pipeline::{Effect, ToolPipeline};
use crate::tools::ToolRegistry;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

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

/// Active pipeline execution state
struct ActivePipeline {
    agent_id: AgentId,
    call_id: String,
    pipeline: ToolPipeline,
    index: usize,
    output: String,
    post_effects: Vec<Effect>,
}

/// Executes tools with approval flow and streaming output
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
            // Continue active pipeline
            if self.active.is_some() {
                let active = self.active.as_mut().unwrap();

                if active.index >= active.pipeline.effects.len() {
                    // Pipeline complete
                    let active = self.active.take().unwrap();
                    return Some(ToolEvent::Completed {
                        agent_id: active.agent_id,
                        call_id: active.call_id,
                        content: active.output,
                        is_error: false,
                        effects: active.post_effects,
                    });
                }

                let effect = active.pipeline.effects[active.index].clone();
                active.index += 1;

                // Drop the mutable borrow before calling interpret_effect
                drop(active);

                let interpretation = Self::interpret_effect(effect).await;

                // Re-borrow to update state
                let active = self.active.as_mut().unwrap();
                match interpretation {
                    Interpretation::Continue => continue,
                    Interpretation::Output(content) => {
                        active.output = content;
                        continue;
                    }
                    Interpretation::Delta(delta) => {
                        return Some(ToolEvent::OutputDelta {
                            agent_id: active.agent_id,
                            call_id: active.call_id.clone(),
                            delta,
                        });
                    }
                    Interpretation::PostEffect(effect) => {
                        active.post_effects.push(effect);
                        continue;
                    }
                    Interpretation::Error(msg) => {
                        let active = self.active.take().unwrap();
                        return Some(ToolEvent::Completed {
                            agent_id: active.agent_id,
                            call_id: active.call_id,
                            content: msg,
                            is_error: true,
                            effects: vec![],
                        });
                    }
                }
            }

            // Start next pending tool
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
                    let tool = self.tools.get(&tool_call.name);
                    let pipeline = tool.compose(tool_call.params.clone());

                    self.active = Some(ActivePipeline {
                        agent_id: tool_call.agent_id,
                        call_id: tool_call.call_id,
                        pipeline,
                        index: 0,
                        output: String::new(),
                        post_effects: vec![],
                    });
                    continue;
                }
            }
        }
    }

    async fn interpret_effect(effect: Effect) -> Interpretation {
        match effect {
            // === Validation ===
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

            // === IDE effects - post effects for app to handle ===
            Effect::IdeOpen { .. }
            | Effect::IdeShowPreview { .. }
            | Effect::IdeReloadBuffer { .. }
            | Effect::IdeClosePreview => Interpretation::PostEffect(effect),

            // === Control flow ===
            Effect::AwaitApproval => Interpretation::Continue, // Already approved at this point
            Effect::Output { content } => Interpretation::Output(content),
            Effect::StreamDelta { content } => Interpretation::Delta(content),
            Effect::Error { message } => Interpretation::Error(message),

            // === File system ===
            Effect::ReadFile { ref path } => {
                match Self::read_file(path) {
                    Ok(content) => Interpretation::Output(content),
                    Err(e) => Interpretation::Error(e),
                }
            }
            Effect::WriteFile { ref path, ref content } => {
                // Create parent directories if needed
                if let Some(parent) = path.parent() {
                    if !parent.exists() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            return Interpretation::Error(format!(
                                "Failed to create directory {}: {}", parent.display(), e
                            ));
                        }
                    }
                }
                match fs::write(path, content) {
                    Ok(()) => Interpretation::Continue,
                    Err(e) => Interpretation::Error(format!("Failed to write {}: {}", path.display(), e)),
                }
            }

            // === Shell ===
            Effect::Shell { command, working_dir, timeout_secs } => {
                Self::execute_shell(&command, working_dir.as_deref(), timeout_secs).await
            }

            // === Network ===
            Effect::FetchUrl { url, max_length } => {
                Self::fetch_url(&url, max_length).await
            }
            Effect::WebSearch { query, count } => {
                Self::web_search(&query, count).await
            }

            // === Agents - post effects for app to handle ===
            Effect::SpawnAgent { .. } | Effect::Notify { .. } => {
                Interpretation::PostEffect(effect)
            }
        }
    }

    fn read_file(path: &Path) -> Result<String, String> {
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }
        if !path.is_file() {
            return Err(format!("Not a file: {}", path.display()));
        }

        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read file: {}", e))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        let line_num_width = total_lines.to_string().len().max(4);

        let mut output = String::new();
        for (i, line) in lines.iter().enumerate() {
            output.push_str(&format!("{:>width$}│{}\n", i + 1, line, width = line_num_width));
        }

        Ok(output)
    }

    async fn execute_shell(command: &str, working_dir: Option<&str>, timeout_secs: u64) -> Interpretation {
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(command);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if let Some(dir) = working_dir {
            let path = Path::new(dir);
            if !path.exists() {
                return Interpretation::Error(format!("Working directory does not exist: {}", dir));
            }
            if !path.is_dir() {
                return Interpretation::Error(format!("Not a directory: {}", dir));
            }
            cmd.current_dir(dir);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Interpretation::Error(format!("Failed to spawn: {}", e)),
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let mut collected = String::new();

        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                collected.push_str(&line);
                collected.push('\n');
            }
        }

        let mut stderr_output = String::new();
        if let Some(stderr) = stderr {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                stderr_output.push_str(&line);
                stderr_output.push('\n');
            }
        }

        let status = match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait(),
        ).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Interpretation::Error(format!("Wait failed: {}", e)),
            Err(_) => {
                let _ = child.kill().await;
                return Interpretation::Error(format!("Command timed out after {} seconds", timeout_secs));
            }
        };

        let exit_code = status.code().unwrap_or(-1);
        let mut result = collected;

        if !stderr_output.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("[stderr]\n");
            result.push_str(&stderr_output);
        }

        if result.is_empty() {
            result = "(no output)".to_string();
        }

        if exit_code != 0 {
            result.push_str(&format!("\n[exit code: {}]", exit_code));
        }

        // Truncate if too long
        const MAX_OUTPUT: usize = 50000;
        if result.len() > MAX_OUTPUT {
            result = format!(
                "{}\n\n[... output truncated ({} bytes total)]",
                &result[..MAX_OUTPUT],
                result.len()
            );
        }

        if status.success() {
            Interpretation::Output(result)
        } else {
            Interpretation::Error(result)
        }
    }

    async fn fetch_url(url: &str, max_length: Option<usize>) -> Interpretation {
        let max_length = max_length.unwrap_or(50000);

        let parsed_url = match url::Url::parse(url) {
            Ok(u) => u,
            Err(e) => return Interpretation::Error(format!("Invalid URL: {}", e)),
        };

        if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
            return Interpretation::Error(format!(
                "Unsupported URL scheme: {}. Only http and https are allowed.",
                parsed_url.scheme()
            ));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.get(url).send(),
        ).await;

        match result {
            Ok(Ok(response)) => {
                let status = response.status();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("unknown")
                    .to_string();

                if !status.is_success() {
                    return Interpretation::Error(format!(
                        "HTTP error: {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown")
                    ));
                }

                match response.text().await {
                    Ok(mut text) => {
                        let original_len = text.len();
                        if text.len() > max_length {
                            text = text[..max_length].to_string();
                            text.push_str(&format!(
                                "\n\n[... truncated, {} of {} bytes shown]",
                                max_length, original_len
                            ));
                        }

                        let header = format!(
                            "[URL: {}]\n[Content-Type: {}]\n[Size: {} bytes]\n\n",
                            url, content_type, original_len
                        );

                        Interpretation::Output(header + &text)
                    }
                    Err(e) => Interpretation::Error(format!("Failed to read response body: {}", e)),
                }
            }
            Ok(Err(e)) => Interpretation::Error(format!("Request failed: {}", e)),
            Err(_) => Interpretation::Error("Request timed out after 30 seconds".to_string()),
        }
    }

    async fn web_search(query: &str, count: u32) -> Interpretation {
        let api_key = match std::env::var("BRAVE_API_KEY") {
            Ok(key) => key,
            Err(_) => return Interpretation::Error(
                "BRAVE_API_KEY environment variable not set. \
                 Get an API key from https://brave.com/search/api/".to_string()
            ),
        };

        let count = count.min(20);
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding::encode(query),
            count
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client
                .get(&url)
                .header("Accept", "application/json")
                .header("X-Subscription-Token", &api_key)
                .send(),
        ).await;

        match result {
            Ok(Ok(response)) => {
                let status = response.status();
                if !status.is_success() {
                    let error_text = response.text().await.unwrap_or_default();
                    return Interpretation::Error(format!(
                        "Brave Search API error: {} {} - {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown"),
                        error_text
                    ));
                }

                match response.json::<BraveSearchResponse>().await {
                    Ok(search_response) => {
                        let mut output = String::new();
                        if let Some(web) = search_response.web {
                            if web.results.is_empty() {
                                output.push_str("No results found.");
                            } else {
                                for (i, result) in web.results.iter().enumerate() {
                                    output.push_str(&format!(
                                        "{}. [{}]({})\n", i + 1, result.title, result.url
                                    ));
                                }
                            }
                        } else {
                            output.push_str("No web results found.");
                        }
                        Interpretation::Output(output)
                    }
                    Err(e) => Interpretation::Error(format!(
                        "Failed to parse Brave Search response: {}", e
                    )),
                }
            }
            Ok(Err(e)) => Interpretation::Error(format!("Request failed: {}", e)),
            Err(_) => Interpretation::Error("Request timed out after 30 seconds".to_string()),
        }
    }
}

enum Interpretation {
    Continue,
    Output(String),
    Delta(String),
    PostEffect(Effect),
    Error(String),
}

// Brave Search API response structures
#[derive(Debug, serde::Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<WebResults>,
}

#[derive(Debug, serde::Deserialize)]
struct WebResults {
    #[serde(default)]
    results: Vec<WebResult>,
}

#[derive(Debug, serde::Deserialize)]
struct WebResult {
    title: String,
    url: String,
}
