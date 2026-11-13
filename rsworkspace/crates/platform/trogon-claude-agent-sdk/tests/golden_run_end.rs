#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Pins when the SDK closes the CLI's stdin relative to what the CLI has sent, as `sdk.mjs` 0.3.323 does with
//! `hasBidirectionalNeeds()`: with nothing that needs the CLI to keep asking (no MCP server, hook, permission
//! callback, elicitation or dialog handler, auth callback or work secret answerer), a streamed input is closed as
//! soon as it was written and a single turn ends at its result. With such a need, a result only ends the run when
//! the session reported no state or is idle.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, PermissionDecision, Prompt, ToolPermissionRequest,
    adapters::{NativeFileSystem, RuntimeAdapters},
    query_turn_with_adapters,
    test_support::FixedEnvironment,
};

#[path = "support/captured.rs"]
mod captured;
#[path = "support/scripted_process.rs"]
mod scripted_process;

use captured::Captured;
use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CAPTURED: &str = include_str!("fixtures/auth_callbacks/run_end.json");

enum Step {
    Wait(u64),
    State(&'static str),
    Result,
}

type Failure = Box<dyn std::error::Error + Send + Sync>;
type Stdout = Arc<tokio::sync::Mutex<tokio::io::DuplexStream>>;

async fn write(stdout: &Stdout, frame: &Value) -> Result<(), Failure> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    stdout.lock().await.write_all(&line).await?;
    Ok(())
}

/// Plays `steps` and notes the last `result` or session state it sent.
async fn play(stdout: Stdout, steps: Vec<Step>, last: Arc<Mutex<&'static str>>) -> Result<(), Failure> {
    for step in steps {
        let (name, frame) = match step {
            Step::Wait(millis) => {
                tokio::time::sleep(Duration::from_millis(millis)).await;
                continue;
            }
            Step::State(state) => (
                state,
                json!({"type":"system","subtype":"session_state_changed","state":state,"session_id":"s"}),
            ),
            Step::Result => (
                "result",
                json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,
                    "num_turns":1,"session_id":"s","result":"done","queued_turn_count":0}),
            ),
        };
        *last.lock().map_err(|_| "lock poisoned")? = name;
        if write(&stdout, &frame).await.is_err() {
            break;
        }
    }
    Ok(())
}

/// Answers `initialize`, plays `steps` once the first user message arrives and reports what the CLI had sent
/// when the SDK closed stdin.
async fn serve(process: ScriptedProcessHarness, steps: Vec<Step>) -> Result<&'static str, Failure> {
    let mut lines = BufReader::new(process.stdin).lines();
    let stdout: Stdout = Arc::new(tokio::sync::Mutex::new(process.stdout));
    let stderr = process.stderr;
    let last = Arc::new(Mutex::new("nothing"));
    let mut steps = Some(steps);
    let mut player = None;
    while let Some(line) = lines.next_line().await? {
        let frame: Value = serde_json::from_str(&line)?;
        if frame["type"] == "control_request" {
            write(
                &stdout,
                &json!({"type":"control_response","response":{
                    "subtype":"success","request_id":frame["request_id"],"response":{}}}),
            )
            .await?;
        } else if frame["type"] == "user"
            && let Some(steps) = steps.take()
        {
            player = Some(tokio::spawn(play(Arc::clone(&stdout), steps, Arc::clone(&last))));
        }
    }
    let closed_after = *last.lock().map_err(|_| "lock poisoned")?;
    if let Some(player) = player {
        player.await??;
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(closed_after)
}

fn running() -> Vec<Step> {
    vec![
        Step::State("running"),
        Step::Wait(100),
        Step::Result,
        Step::Wait(400),
        Step::State("idle"),
    ]
}

fn without_state() -> Vec<Step> {
    vec![Step::Wait(100), Step::Result, Step::Wait(400), Step::State("idle")]
}

fn streamed() -> Vec<Step> {
    let mut steps = vec![Step::Wait(100)];
    steps.extend(running());
    steps
}

fn options(needs: bool) -> ClaudeAgentOptions {
    let mut options = ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        ..ClaudeAgentOptions::default()
    };
    if needs {
        options.permission_handler = Some(Arc::new(|_: ToolPermissionRequest| async {
            PermissionDecision::Allow {
                updated_input: None,
                updated_permissions: None,
                decision_classification: None,
            }
        }));
    }
    options
}

fn adapters(
    process: scripted_process::ScriptedProcessSpawner,
) -> RuntimeAdapters<FixedEnvironment, scripted_process::ScriptedProcessSpawner, NativeFileSystem> {
    RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-directory"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    }
}

async fn turn(needs: bool, steps: Vec<Step>) -> Result<&'static str, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve(harness, steps));
    let query = query_turn_with_adapters(Prompt::from("hi"), options(needs), adapters(process)).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    query.close().await?;
    Ok(server.await?.map_err(|error| error.to_string())?)
}

async fn streamed_input(needs: bool, steps: Vec<Step>) -> Result<&'static str, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve(harness, steps));
    let client = ClaudeSdkClient::connect_with_adapters(options(needs), adapters(process)).await?;
    client.send_prompt("hi".into()).await?;
    client.finish_input().await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    client.disconnect().await?;
    Ok(server.await?.map_err(|error| error.to_string())?)
}

#[tokio::test]
async fn a_single_turn_ends_at_its_result_unless_something_needs_the_cli_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let expected = |scenario: &str| -> Result<String, Box<dyn std::error::Error>> {
        Ok(captured
            .field(scenario, "stdin_closed_after")?
            .as_str()
            .ok_or("not a string")?
            .to_owned())
    };
    assert_eq!(turn(false, running()).await?, expected("turn_no_needs")?);
    assert_eq!(turn(true, running()).await?, expected("turn_needs_running")?);
    assert_eq!(turn(true, without_state()).await?, expected("turn_needs_no_state")?);
    assert_eq!(turn(false, without_state()).await?, expected("turn_no_needs_no_state")?);
    Ok(())
}

#[tokio::test]
async fn a_streamed_input_is_closed_when_written_unless_something_needs_the_cli_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let expected = |scenario: &str| -> Result<String, Box<dyn std::error::Error>> {
        Ok(captured
            .field(scenario, "stdin_closed_after")?
            .as_str()
            .ok_or("not a string")?
            .to_owned())
    };
    assert_eq!(streamed_input(false, streamed()).await?, expected("stream_no_needs")?);
    assert_eq!(
        streamed_input(true, streamed()).await?,
        expected("stream_needs_running")?
    );
    assert_eq!(
        streamed_input(true, without_state()).await?,
        expected("stream_needs_no_state")?
    );
    Ok(())
}
