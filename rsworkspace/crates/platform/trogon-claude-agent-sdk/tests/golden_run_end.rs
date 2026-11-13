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
//! the session reported no state or is idle. The reader keeps yielding what the CLI sends after the result until
//! the CLI exits, as `sdk.mjs` and `claude_agent_sdk` do; only `query_turn` stops at the result.
//! A CLI that never sends a result gets its stdin closed right after the last prompt when nothing needs it, and
//! keeps it open otherwise.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, HookEvent, HookFuture, HookOutput, HookRegistration, Message,
    PermissionDecision, Prompt, Query, ToolPermissionRequest,
    adapters::{NativeFileSystem, RuntimeAdapters},
    query_turn_with_adapters, query_with_adapters,
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
const POST: &str = include_str!("fixtures/auth_callbacks/run_post.json");

enum Step {
    Wait(u64),
    State(&'static str),
    Said(&'static str),
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
            Step::Said(text) => (
                text,
                json!({"type":"assistant","session_id":"s","parent_tool_use_id":null,
                    "uuid":format!("00000000-0000-4000-8000-00000000000{}", text.len()),
                    "message":{"id":format!("m{text}"),"type":"message","role":"assistant","model":"m",
                        "content":[{"type":"text","text":text}],"stop_reason":"end_turn","stop_sequence":null,
                        "usage":{"input_tokens":1,"output_tokens":1}}}),
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
/// when the SDK closed stdin. `closing` is set by the test just before it shuts the session down, so a stdin that
/// only closed then was never closed by the run itself and is reported as `"never"`.
async fn serve(
    process: ScriptedProcessHarness,
    steps: Vec<Step>,
    closing: Arc<AtomicBool>,
) -> Result<&'static str, Failure> {
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
    let closed_after = if closing.load(Ordering::SeqCst) {
        "never"
    } else {
        *last.lock().map_err(|_| "lock poisoned")?
    };
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
    let server = tokio::spawn(serve(harness, steps, Arc::new(AtomicBool::new(false))));
    let query = query_turn_with_adapters(Prompt::from("hi"), options(needs), adapters(process)).await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    query.close().await?;
    Ok(server.await?.map_err(|error| error.to_string())?)
}

async fn streamed_input(needs: bool, steps: Vec<Step>) -> Result<&'static str, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve(harness, steps, Arc::new(AtomicBool::new(false))));
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

/// Everything the CLI sends: the result comes in the middle, with messages on both sides of it.
fn after_the_result() -> Vec<Step> {
    vec![
        Step::Wait(100),
        Step::State("running"),
        Step::Wait(100),
        Step::Said("A"),
        Step::Result,
        Step::Wait(300),
        Step::Said("BB"),
        Step::State("idle"),
        Step::Wait(300),
        Step::Said("CCC"),
    ]
}

fn python_options(needs: bool) -> ClaudeAgentOptions {
    let mut options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        ..options(false)
    };
    if needs {
        options.hooks = vec![HookRegistration {
            event: HookEvent::PreToolUse,
            matcher: None,
            callbacks: vec![Arc::new(|_: trogon_claude_agent_sdk::HookRequest| {
                Box::pin(async { Ok(HookOutput::default()) }) as HookFuture
            })],
            timeout: None,
            on_failure: None,
        }];
    }
    options
}

/// What the reader saw: the label of each message it got, and whether the stream ended by itself.
struct Seen {
    closed_after: &'static str,
    yielded: Vec<String>,
    ended: bool,
}

fn label(message: &Message) -> String {
    let raw = message.raw();
    let kind = raw.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let text = raw
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.get(0))
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str);
    match (kind, text) {
        ("assistant", Some(text)) => text.to_owned(),
        ("system", _) if raw.contains_key("state") => raw.get("state").and_then(Value::as_str).unwrap_or("").to_owned(),
        _ => kind.to_owned(),
    }
}

async fn read_query(query: &mut Query, yielded: &mut Vec<String>) -> Result<(), trogon_claude_agent_sdk::Error> {
    while let Some(message) = query.next_message().await? {
        yielded.push(label(&message));
    }
    Ok(())
}

async fn query_to_the_end(options: ClaudeAgentOptions, steps: Vec<Step>) -> Result<Seen, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let closing = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(serve(harness, steps, Arc::clone(&closing)));
    let mut query = query_with_adapters(Prompt::from("hi"), options, adapters(process)).await?;
    let mut yielded = Vec::new();
    let ended = match tokio::time::timeout(Duration::from_secs(2), read_query(&mut query, &mut yielded)).await {
        Ok(result) => {
            result?;
            true
        }
        Err(_) => false,
    };
    closing.store(true, Ordering::SeqCst);
    query.close().await?;
    let closed_after = server.await?.map_err(|error| error.to_string())?;
    Ok(Seen {
        closed_after,
        yielded,
        ended,
    })
}

async fn streamed_to_the_end(
    options: ClaudeAgentOptions,
    steps: Vec<Step>,
) -> Result<Seen, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let closing = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(serve(harness, steps, Arc::clone(&closing)));
    let mut client = ClaudeSdkClient::connect_with_adapters(options, adapters(process)).await?;
    client.send_prompt("hi".into()).await?;
    client.finish_input().await?;
    let mut yielded = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(message) = client.next_message().await? {
            yielded.push(label(&message));
        }
        Ok::<(), trogon_claude_agent_sdk::Error>(())
    })
    .await;
    let ended = match ended {
        Ok(result) => {
            result?;
            true
        }
        Err(_) => false,
    };
    closing.store(true, Ordering::SeqCst);
    client.disconnect().await?;
    let closed_after = server.await?.map_err(|error| error.to_string())?;
    Ok(Seen {
        closed_after,
        yielded,
        ended,
    })
}

fn assert_seen(captured: &Captured, scenario: &str, seen: &Seen) -> TestResult {
    assert_eq!(
        captured.field(scenario, "stdin_closed_after")?.as_str(),
        Some(seen.closed_after),
        "{scenario}: when stdin closed"
    );
    assert_eq!(
        captured.strings(scenario, "yielded")?,
        seen.yielded,
        "{scenario}: what the reader yielded"
    );
    assert_eq!(
        captured.field(scenario, "ended")?.as_bool(),
        Some(seen.ended),
        "{scenario}: whether the stream ended by itself"
    );
    Ok(())
}

#[tokio::test]
async fn a_single_prompt_keeps_yielding_after_its_result_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(POST)?;
    assert_seen(
        &captured,
        "post_turn_no_needs",
        &query_to_the_end(options(false), after_the_result()).await?,
    )?;
    assert_seen(
        &captured,
        "post_turn_needs",
        &query_to_the_end(options(true), after_the_result()).await?,
    )?;
    Ok(())
}

#[tokio::test]
async fn a_streamed_input_keeps_yielding_after_its_result_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(POST)?;
    assert_seen(
        &captured,
        "post_stream_no_needs",
        &streamed_to_the_end(options(false), after_the_result()).await?,
    )?;
    assert_seen(
        &captured,
        "post_stream_needs",
        &streamed_to_the_end(options(true), after_the_result()).await?,
    )?;
    Ok(())
}

#[tokio::test]
async fn python_closes_stdin_once_the_prompt_is_written_and_keeps_yielding() -> TestResult {
    let captured = Captured::parse(POST)?;
    assert_seen(
        &captured,
        "python_post_no_needs",
        &query_to_the_end(python_options(false), after_the_result()).await?,
    )?;
    assert_seen(
        &captured,
        "python_post_needs",
        &query_to_the_end(python_options(true), after_the_result()).await?,
    )?;
    Ok(())
}

/// The CLI sends one message and never a result.
fn no_result() -> Vec<Step> {
    vec![Step::Wait(100), Step::Said("A")]
}

#[tokio::test]
async fn without_a_result_stdin_closes_when_the_input_is_written_unless_something_needs_the_cli() -> TestResult {
    let captured = Captured::parse(POST)?;
    assert_seen(
        &captured,
        "none_stream_no_needs",
        &streamed_to_the_end(options(false), no_result()).await?,
    )?;
    assert_seen(
        &captured,
        "none_stream_needs",
        &streamed_to_the_end(options(true), no_result()).await?,
    )?;
    assert_seen(
        &captured,
        "none_turn_no_needs",
        &query_to_the_end(options(false), no_result()).await?,
    )?;
    assert_seen(
        &captured,
        "none_turn_needs",
        &query_to_the_end(options(true), no_result()).await?,
    )?;
    Ok(())
}

#[tokio::test]
async fn python_without_a_result_closes_stdin_once_the_prompt_is_written_unless_something_needs_the_cli() -> TestResult
{
    let captured = Captured::parse(POST)?;
    assert_seen(
        &captured,
        "python_none_no_needs",
        &query_to_the_end(python_options(false), no_result()).await?,
    )?;
    assert_seen(
        &captured,
        "python_none_needs",
        &query_to_the_end(python_options(true), no_result()).await?,
    )?;
    Ok(())
}
