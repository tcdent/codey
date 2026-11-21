//! Integration tests for the rockem-backed agent, exercising the public lib API
//! against a live rockem service. `#[ignore]`d by default because they need one
//! running; run explicitly with:
//!
//! ```bash
//! cargo test --test rockem_integration -- --ignored --nocapture
//! ```
//!
//! The assertions are about the loop (text streams, calls round-trip, the
//! conversation carries), not about how well the served model answers.

use std::sync::Arc;
use std::time::Duration;

use codey::{Agent, AgentRuntimeConfig, AgentStep, Backend, RequestMode, SimpleTool, ToolRegistry};
use serde_json::json;
use tokio::time::timeout;

/// Per-step timeout. Steps take about a second on a free node; longer is a finding.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

struct Outcome {
    text: String,
    tool_calls: Vec<(String, serde_json::Value)>,
    finished: bool,
    error: Option<String>,
}

/// Drive `agent.next()` until the turn finishes, answering tool calls with
/// `handle_tool(name, params)`.
async fn drive(
    agent: &mut Agent,
    mut handle_tool: impl FnMut(&str, &serde_json::Value) -> String,
) -> Outcome {
    let mut out = Outcome { text: String::new(), tool_calls: Vec::new(), finished: false, error: None };
    loop {
        match timeout(STEP_TIMEOUT, agent.next()).await {
            Ok(Some(step)) => match step {
                AgentStep::TextDelta(t) => out.text.push_str(&t),
                AgentStep::ThinkingDelta(_) | AgentStep::Retrying { .. } => {},
                AgentStep::ToolRequest(calls) => {
                    for c in calls {
                        let result = handle_tool(&c.name, &c.params);
                        out.tool_calls.push((c.name.clone(), c.params.clone()));
                        agent.submit_tool_result(&c.call_id, result);
                    }
                },
                AgentStep::Finished { .. } => {
                    out.finished = true;
                    break;
                },
                AgentStep::Error(e) => {
                    out.error = Some(e);
                    break;
                },
            },
            Ok(None) => break,
            Err(_) => {
                out.error = Some("timed out waiting for next agent step".to_string());
                break;
            },
        }
    }
    out
}

fn test_config() -> AgentRuntimeConfig {
    let url = std::env::var("ROCKEM_URL").unwrap_or_else(|_| "https://rockem.a10k.co".to_string());
    AgentRuntimeConfig { max_tokens: 256, backend: Backend::Rockem { url }, ..AgentRuntimeConfig::default() }
}

/// A plain question streams text and finishes; the bundle names the model.
#[tokio::test]
#[ignore = "needs a live rockem service at ROCKEM_URL"]
async fn streaming_basic() {
    let mut agent = Agent::new(test_config(), "You are a terse assistant.", ToolRegistry::empty());
    agent.send_request("What is the capital of France? One word.", RequestMode::Normal);
    let out = drive(&mut agent, |_, _| String::new()).await;

    assert!(out.error.is_none(), "agent errored: {:?}", out.error);
    assert!(out.finished, "turn did not finish; text so far: {:?}", out.text);
    assert!(!out.text.trim().is_empty(), "no text streamed");
    assert!(agent.total_usage().context_tokens > 0);
    eprintln!("[streaming_basic] {} t/s", agent.total_usage().tokens_per_second);
    eprintln!("[streaming_basic] {}", out.text.trim());
}

/// One round trip: a call comes back as a ToolRequest, and once its result is
/// submitted the next request goes out and generates. Only the first round is
/// checked: V2-Lite calls again every round and never ends the turn on its own.
#[tokio::test]
#[ignore = "needs a live rockem service at ROCKEM_URL"]
async fn tool_round_trip() {
    let mut tools = ToolRegistry::empty();
    tools.register(Arc::new(SimpleTool::new(
        "read_file",
        "Read a file.",
        json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
    )));
    let mut agent = Agent::new(test_config(), "You are a coding assistant. Use tools.", tools);
    agent.send_request("Read the file src/main.rs.", RequestMode::Normal);

    let calls = loop {
        match timeout(STEP_TIMEOUT, agent.next()).await.expect("timed out before the call") {
            Some(AgentStep::ToolRequest(calls)) => break calls,
            Some(AgentStep::TextDelta(_)) => {},
            Some(AgentStep::Error(e)) => panic!("agent errored: {e}"),
            Some(AgentStep::Finished { .. }) | None => panic!("turn ended without a call"),
            Some(_) => {},
        }
    };
    assert_eq!(calls.len(), 1, "expected one call, got {}", calls.len());
    assert_eq!(calls[0].name, "read_file");
    assert_eq!(calls[0].params["path"], "src/main.rs");
    agent.submit_tool_result(&calls[0].call_id, "fn main() {}\n".to_string());

    // The result went out with the next request: it generates something.
    match timeout(STEP_TIMEOUT, agent.next()).await.expect("timed out after the result") {
        Some(AgentStep::Error(e)) => panic!("request with the result errored: {e}"),
        None => panic!("no request went out after the result"),
        Some(_) => {},
    }
    agent.cancel();
}

/// The agent keeps the conversation: a follow-up sees the first turn.
#[tokio::test]
#[ignore = "needs a live rockem service at ROCKEM_URL"]
async fn multi_turn_context() {
    let mut agent = Agent::new(test_config(), "You are a terse assistant.", ToolRegistry::empty());
    agent.send_request("My favorite color is teal. Reply with just: OK", RequestMode::Normal);
    let first = drive(&mut agent, |_, _| String::new()).await;
    assert!(first.error.is_none(), "turn 1 errored: {:?}", first.error);
    assert!(first.finished, "turn 1 did not finish");

    agent.send_request("What is my favorite color? One word.", RequestMode::Normal);
    let second = drive(&mut agent, |_, _| String::new()).await;
    assert!(second.error.is_none(), "turn 2 errored: {:?}", second.error);
    assert!(second.finished, "turn 2 did not finish");
    eprintln!("[multi_turn_context] turn 1: {}; turn 2: {}", first.text.trim(), second.text.trim());
    assert!(
        second.text.to_lowercase().contains("teal"),
        "expected 'teal' from turn 1, got: {:?}",
        second.text
    );
}

/// Cancelling mid-generation drops the stream, which frees the node: the next
/// request on the same agent is served at once.
#[tokio::test]
#[ignore = "needs a live rockem service at ROCKEM_URL"]
async fn cancel_frees_the_node() {
    let config = AgentRuntimeConfig { max_tokens: 4096, ..test_config() };
    let mut agent = Agent::new(config, "You are a verbose assistant.", ToolRegistry::empty());
    agent.send_request("Count from 1 to 1000, one number per line.", RequestMode::Normal);
    // The first generated output: reasoning on a thinking model, text otherwise.
    match timeout(STEP_TIMEOUT, agent.next()).await.expect("timed out before the first output") {
        Some(AgentStep::ThinkingDelta(_) | AgentStep::TextDelta(_)) => {},
        other => panic!("expected generated output first, got {}", other.is_some()),
    }
    agent.cancel();

    agent.send_request("What is the capital of France? One word.", RequestMode::Normal);
    let out = drive(&mut agent, |_, _| String::new()).await;
    assert!(out.error.is_none(), "request after cancel errored: {:?}", out.error);
    assert!(out.finished, "request after cancel did not finish");
    eprintln!("[cancel_frees_the_node] after cancel: {}", out.text.trim());
}

/// Read one HTTP request off the socket and return its body.
async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = socket.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                return String::from_utf8_lossy(&buf[end + 4..end + 4 + length]).into_owned();
            }
        }
    }
    String::new()
}

/// A slow fake service: each stage of a generation 150 ms apart (queued, prefill,
/// reasoning, reply), the stream written as it goes. Returns its URL, the number of
/// connections it took, and every request body.
async fn slow_service() -> (String, Arc<std::sync::atomic::AtomicUsize>, Arc<std::sync::Mutex<Vec<String>>>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (counted, kept) = (connections.clone(), bodies.clone());
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            counted.fetch_add(1, Ordering::SeqCst);
            let kept = kept.clone();
            tokio::spawn(async move {
                let body = read_request(&mut socket).await;
                kept.lock().unwrap().push(body);
                let stage = Duration::from_millis(150);
                tokio::time::sleep(stage).await; // queued
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let events = [
                    r#"{"type":"started","prompt_tokens":5}"#,
                    r#"{"type":"thinking","text":"The capital.","ids":[7,8]}"#,
                    r#"{"type":"text","text":"Paris","ids":[1,2]}"#,
                    r#"{"type":"done","reason":"end_turn","stop_sequence":null,"usage":{"prompt_tokens":5,"completion_tokens":4,"positions":100},"model":"fake","identity":"0"}"#,
                ];
                for event in events {
                    let _ = socket.write_all(format!("data: {event}\n\n").as_bytes()).await;
                    let _ = socket.flush().await;
                    tokio::time::sleep(stage).await; // prefill, reasoning, reply
                }
            });
        }
    });
    (url, connections, bodies)
}

/// A poll dropped while the request is out (the UI's redraw tick does this every
/// second) resumes the same request rather than sending another. Along the way the
/// readout walks queued, prefill, thinking, writing; and the next request carries
/// the turn's reasoning back. Offline, against a slow fake service.
#[tokio::test]
async fn dropped_polls_resume_the_request() {
    use codey::Phase;
    use std::sync::atomic::Ordering;

    let (url, connections, bodies) = slow_service().await;
    let config = AgentRuntimeConfig { backend: Backend::Rockem { url }, ..AgentRuntimeConfig::default() };
    let mut agent = Agent::new(config, "test", ToolRegistry::empty());
    agent.send_request("hi", RequestMode::Normal);

    let (mut thinking, mut text) = (String::new(), String::new());
    let mut phases: Vec<Phase> = Vec::new();
    for drops in 0.. {
        // A request sent again on every drop would never come back.
        assert!(drops < 250, "no reply after 5 s of dropped polls; the request is being re-sent");
        match timeout(Duration::from_millis(20), agent.next()).await {
            Err(_) => {
                // The poll was dropped, as the redraw tick drops it.
                let phase = agent.activity().expect("turn running").phase;
                if phases.last() != Some(&phase) {
                    phases.push(phase);
                }
            },
            Ok(Some(AgentStep::ThinkingDelta(t))) => thinking.push_str(&t),
            Ok(Some(AgentStep::TextDelta(t))) => text.push_str(&t),
            Ok(Some(AgentStep::Finished { .. })) => break,
            Ok(Some(AgentStep::Error(e))) => panic!("agent errored: {e}"),
            Ok(other) => panic!("unexpected step: {}", other.is_some()),
        }
    }
    assert_eq!((thinking.as_str(), text.as_str()), ("The capital.", "Paris"));
    assert_eq!(phases, [Phase::Queued, Phase::Prefill, Phase::Thinking, Phase::Writing]);
    assert!(agent.activity().is_none(), "activity outlived the turn");
    assert_eq!(connections.load(Ordering::SeqCst), 1, "the request was sent more than once");

    agent.send_request("and Spain?", RequestMode::Normal);
    let out = drive(&mut agent, |_, _| String::new()).await;
    assert!(out.finished, "second turn did not finish: {:?}", out.error);
    let second: serde_json::Value = serde_json::from_str(&bodies.lock().unwrap()[1]).unwrap();
    let reply = &second["messages"][2];
    assert_eq!(reply["role"], "assistant");
    assert_eq!((reply["thinking"].as_str(), reply["content"].as_str()), (Some("The capital."), Some("Paris")));
}

/// A stream that breaks mid-reply ends the turn with an error; the next request
/// goes out on the same agent and carries what was generated before the break.
/// Offline, against a fake service that cuts its first reply short.
#[tokio::test]
async fn broken_stream_ends_the_turn_and_keeps_the_partial_reply() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = bodies.clone();
    let served = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (kept, served) = (kept.clone(), served.clone());
            tokio::spawn(async move {
                let body = read_request(&mut socket).await;
                kept.lock().unwrap().push(body);
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n")
                    .await;
                let mut events = vec![
                    r#"{"type":"started","prompt_tokens":5}"#,
                    r#"{"type":"text","text":"Half a ","ids":[1,2]}"#,
                ];
                if served.fetch_add(1, Ordering::SeqCst) > 0 {
                    events.push(r#"{"type":"done","reason":"end_turn","stop_sequence":null,"usage":{"prompt_tokens":5,"completion_tokens":2,"positions":100},"model":"fake","identity":"0"}"#);
                }
                for event in events {
                    let _ = socket.write_all(format!("data: {event}\n\n").as_bytes()).await;
                }
                // The first reply closes here, without done: the socket is gone.
            });
        }
    });

    let config = AgentRuntimeConfig { backend: Backend::Rockem { url }, ..AgentRuntimeConfig::default() };
    let mut agent = Agent::new(config, "test", ToolRegistry::empty());
    agent.send_request("first", RequestMode::Normal);
    let broken = drive(&mut agent, |_, _| String::new()).await;
    assert!(broken.error.is_some(), "a cut stream should end the turn with an error");
    assert!(agent.activity().is_none(), "the turn should be over");

    agent.send_request("second", RequestMode::Normal);
    let out = drive(&mut agent, |_, _| String::new()).await;
    assert!(out.finished, "the next turn did not finish: {:?}", out.error);
    let second: serde_json::Value = serde_json::from_str(&bodies.lock().unwrap()[1]).unwrap();
    let roles: Vec<&str> = second["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"]);
    assert_eq!(second["messages"][2]["content"], "Half a ");
}
