//! Integration tests for the SDK-backed agent, exercising the public lib API
//! against the real `claude` binary. These are `#[ignore]`d by default because
//! they spawn the binary and hit the network; run explicitly with:
//!
//! ```bash
//! ANTHROPIC_API_KEY=sk-ant-... cargo test --test sdk_integration -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, SimpleTool, ToolRegistry};
use serde_json::json;
use tokio::time::timeout;

/// Per-step timeout. The first step includes spawning + connecting the binary.
const STEP_TIMEOUT: Duration = Duration::from_secs(90);

/// Outcome of driving an agent turn to completion.
struct Outcome {
    text: String,
    tool_calls: Vec<(String, serde_json::Value)>,
    finished: bool,
    error: Option<String>,
}

/// Drive `agent.next()` until the turn finishes, answering tool calls with the
/// provided closure (name, params) -> result string.
async fn drive(
    agent: &mut Agent,
    mut handle_tool: impl FnMut(&str, &serde_json::Value) -> String,
) -> Outcome {
    let mut out = Outcome {
        text: String::new(),
        tool_calls: Vec::new(),
        finished: false,
        error: None,
    };

    loop {
        match timeout(STEP_TIMEOUT, agent.next()).await {
            Ok(Some(step)) => match step {
                AgentStep::TextDelta(t) => out.text.push_str(&t),
                AgentStep::ThinkingDelta(_) => {},
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
                AgentStep::Retrying { .. } => {},
            },
            Ok(None) => break, // step channel closed
            Err(_) => {
                out.error = Some("timed out waiting for next agent step".to_string());
                break;
            },
        }
    }

    out
}

fn test_config() -> AgentRuntimeConfig {
    AgentRuntimeConfig {
        // Use a model alias the binary resolves, to avoid pinning a dated ID.
        model: "sonnet".to_string(),
        ..AgentRuntimeConfig::default()
    }
}

/// Basic streaming: a plain question should stream text and finish, no tools.
#[tokio::test]
#[ignore = "spawns the claude binary and hits the network; needs ANTHROPIC_API_KEY"]
async fn streaming_basic() {
    let mut agent = Agent::new(
        test_config(),
        "You are a terse test assistant. Answer in one word.",
        ToolRegistry::empty(),
    );

    agent.send_request("Reply with exactly: PONG", RequestMode::Normal);
    let out = drive(&mut agent, |_, _| String::new()).await;

    assert!(out.error.is_none(), "agent errored: {:?}", out.error);
    assert!(out.finished, "turn did not finish; text so far: {:?}", out.text);
    assert!(
        out.text.to_uppercase().contains("PONG"),
        "expected PONG in response, got: {:?}",
        out.text
    );
    eprintln!("[streaming_basic] response: {}", out.text.trim());
}

/// Tool round-trip: the model should call Codey's in-process tool, and the
/// result we submit should make it back into the final answer.
#[tokio::test]
#[ignore = "spawns the claude binary and hits the network; needs ANTHROPIC_API_KEY"]
async fn tool_round_trip() {
    let mut tools = ToolRegistry::empty();
    tools.register(Arc::new(SimpleTool::new(
        "get_secret_word",
        "Returns the secret word. Call this whenever asked for the secret word.",
        json!({ "type": "object", "properties": {}, "additionalProperties": false }),
    )));

    let mut agent = Agent::new(
        test_config(),
        "You are a test assistant. When asked for the secret word, call the \
         get_secret_word tool, then reply with exactly the value it returns.",
        tools,
    );

    agent.send_request(
        "What is the secret word? Use the tool, then tell me only the word.",
        RequestMode::Normal,
    );

    let secret = "PINEAPPLE";
    let out = drive(&mut agent, |name, _params| {
        assert_eq!(name, "get_secret_word", "unexpected tool call: {name}");
        secret.to_string()
    })
    .await;

    assert!(out.error.is_none(), "agent errored: {:?}", out.error);
    assert!(out.finished, "turn did not finish; text so far: {:?}", out.text);
    assert!(
        !out.tool_calls.is_empty(),
        "model never called the in-process tool"
    );
    assert!(
        out.text.to_uppercase().contains(secret),
        "expected the tool result {secret:?} in the final answer, got: {:?}",
        out.text
    );
    eprintln!(
        "[tool_round_trip] tool calls: {:?}; response: {}",
        out.tool_calls,
        out.text.trim()
    );
}
