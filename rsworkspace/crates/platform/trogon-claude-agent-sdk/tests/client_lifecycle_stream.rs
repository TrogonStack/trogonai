#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Pins how the message reader ends and how closing it behaves against goldens captured from `sdk.mjs` 0.3.323
//! (`Query.close()`, the async iterator's `return()` and a `for await` that breaks) and `claude_agent_sdk`
//! 0.2.181 (`ClaudeSDKClient.receive_response()` and `receive_messages()`), each run against a scripted CLI that
//! sends the same frames as the one here.

use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    pin::pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, EndWithoutResult, Error, Message, PermissionDecision, Prompt,
    ToolPermissionRequest,
    adapters::{NativeFileSystem, RuntimeAdapters},
    query_with_adapters,
    test_support::FixedEnvironment,
};

#[path = "support/captured.rs"]
mod captured;
#[path = "support/scripted_process.rs"]
mod scripted_process;

use captured::Captured;
use scripted_process::{ScriptedProcessHarness, ScriptedProcessObserver, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Failure = Box<dyn std::error::Error + Send + Sync>;
type Stdout = Arc<tokio::sync::Mutex<tokio::io::DuplexStream>>;
/// What the scripted CLI received, in order: control request subtypes, `user` and `eof` for its stdin closing.
type Log = Arc<Mutex<Vec<String>>>;

const CAPTURED: &str = include_str!("fixtures/client_lifecycle/stream.json");

enum Step {
    Said(&'static str),
    /// A `result` whose text is `done` followed by the number.
    Result(u8),
    Wait(u64),
    /// The CLI exits with status 0 while its stdin is still open.
    Exit,
}

async fn write(stdout: &Stdout, frame: &Value) -> Result<(), Failure> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    stdout.lock().await.write_all(&line).await?;
    Ok(())
}

fn note(log: &Log, event: &str) -> Result<(), Failure> {
    log.lock().map_err(|_| "lock poisoned")?.push(event.to_owned());
    Ok(())
}

fn events(log: &Log) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(log.lock().map_err(|_| "lock poisoned")?.clone())
}

async fn play(stdout: Stdout, steps: Vec<Step>, observer: ScriptedProcessObserver) -> Result<(), Failure> {
    for step in steps {
        let frame = match step {
            Step::Wait(millis) => {
                tokio::time::sleep(Duration::from_millis(millis)).await;
                continue;
            }
            Step::Exit => {
                observer.complete(0);
                return Ok(());
            }
            Step::Said(text) => json!({"type":"assistant","session_id":"s","parent_tool_use_id":null,
                "uuid":format!("00000000-0000-4000-8000-00000000000{}", text.len()),
                "message":{"id":format!("m{text}"),"type":"message","role":"assistant","model":"m",
                    "content":[{"type":"text","text":text}],"stop_reason":"end_turn","stop_sequence":null,
                    "usage":{"input_tokens":1,"output_tokens":1}}}),
            Step::Result(number) => json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,
                "duration_api_ms":1,"num_turns":1,"session_id":"s","result":format!("done{number}"),
                "total_cost_usd":0,"usage":{},"modelUsage":{},"permission_denials":[],
                "uuid":format!("00000000-0000-4000-8000-00000000010{number}")}),
        };
        write(&stdout, &frame).await?;
    }
    Ok(())
}

/// Answers every control request except the `unanswered` subtypes, plays `steps` once the first user message
/// arrives and exits once stdin closes.
async fn serve(
    process: ScriptedProcessHarness,
    steps: Vec<Step>,
    unanswered: &'static [&'static str],
    log: Log,
) -> Result<(), Failure> {
    let mut lines = BufReader::new(process.stdin).lines();
    let stdout: Stdout = Arc::new(tokio::sync::Mutex::new(process.stdout));
    let stderr = process.stderr;
    let mut steps = Some(steps);
    let mut player = None;
    while let Some(line) = lines.next_line().await? {
        let frame: Value = serde_json::from_str(&line)?;
        if frame["type"] == "control_request" {
            let subtype = frame["request"]["subtype"].as_str().unwrap_or("unknown");
            note(&log, subtype)?;
            if !unanswered.contains(&subtype) {
                write(
                    &stdout,
                    &json!({"type":"control_response","response":{
                        "subtype":"success","request_id":frame["request_id"],"response":{}}}),
                )
                .await?;
            }
        } else if frame["type"] == "user" {
            note(&log, "user")?;
            if let Some(steps) = steps.take() {
                let observer = process.observer.clone();
                player = Some(tokio::spawn(play(Arc::clone(&stdout), steps, observer)));
            }
        }
    }
    note(&log, "eof")?;
    if let Some(player) = player {
        player.await??;
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(())
}

fn adapters(
    process: ScriptedProcessSpawner,
) -> RuntimeAdapters<FixedEnvironment, ScriptedProcessSpawner, NativeFileSystem> {
    RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-directory"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    }
}

/// The captured TypeScript runs pass `canUseTool`, so `sdk.mjs` keeps stdin open after the prompt.
fn typescript_options() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        permission_handler: Some(Arc::new(|_: ToolPermissionRequest| async {
            PermissionDecision::Allow {
                updated_input: None,
                updated_permissions: None,
                decision_classification: None,
            }
        })),
        ..ClaudeAgentOptions::default()
    }
}

fn python_options() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        argv_style: ArgvStyle::Python,
        ..ClaudeAgentOptions::default()
    }
}

/// The text of an assistant message, the text of a result, or the message type otherwise.
fn label(message: &Message) -> String {
    let raw = message.raw();
    let kind = raw.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let text = match kind {
        "assistant" => raw
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.get(0))
            .and_then(|block| block.get("text"))
            .and_then(Value::as_str),
        "result" => raw.get("result").and_then(Value::as_str),
        _ => None,
    };
    text.unwrap_or(kind).to_owned()
}

/// Reads `stream` to its end: the labels of the messages it yielded and the error it ended with, if any.
async fn drain(stream: impl Stream<Item = Result<Message, Error>>) -> (Vec<String>, Option<Error>) {
    let mut stream = pin!(stream);
    let mut labels = Vec::new();
    let mut error = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(message) => labels.push(label(&message)),
            Err(failure) => error = Some(failure),
        }
    }
    (labels, error)
}

fn typescript_steps() -> Vec<Step> {
    vec![Step::Said("A"), Step::Said("BB"), Step::Said("CCC")]
}

/// TypeScript `Query.close()` while the caller is still reading: what was received stays readable, the stream
/// then ends cleanly and stays ended, a control request still waiting fails, and a second close does nothing.
#[tokio::test]
async fn close_while_iterating_keeps_received_messages_and_ends_the_stream_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let (process, harness) = ScriptedProcessSpawner::new();
    let log = Log::default();
    let server = tokio::spawn(serve(harness, typescript_steps(), &["interrupt"], Arc::clone(&log)));
    let mut query = query_with_adapters(Prompt::from("hi"), typescript_options(), adapters(process)).await?;
    let first = query.next().await.ok_or("the query ended before its first message")??;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let handle = query.handle().ok_or("the query has no handle")?;
    let pending = tokio::spawn(async move { handle.interrupt().await.map(|_| ()) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    query.close();
    query.close();
    let (after_close, error) = tokio::time::timeout(Duration::from_secs(5), drain(&mut query)).await?;
    assert!(error.is_none(), "{error:?}");
    assert!(query.next().await.is_none());
    let pending = pending.await?.err().ok_or("the waiting interrupt was answered")?;
    server.await?.map_err(|error| error.to_string())?;

    let scenario = "close_while_iterating";
    assert_eq!(vec![label(&first)], captured.strings(scenario, "before_close")?);
    assert_eq!(after_close, captured.strings(scenario, "after_close")?);
    assert_eq!(captured.field(scenario, "ended")?, "done");
    assert_eq!(captured.field(scenario, "next_after_end")?, "done");
    assert!(matches!(pending, Error::QueryClosed), "{pending:?}");
    let rejection = captured.field(scenario, "pending")?.as_str().ok_or("not a string")?;
    assert_eq!(Some(pending.to_string().as_str()), rejection.strip_prefix("Error: "));
    assert_eq!(events(&log)?, captured.strings(scenario, "cli_saw")?);
    query.disconnect().await?;
    Ok(())
}

/// Reads one message, through the `Stream` or through `next_message`, then stops the way a TypeScript `for await`
/// that breaks, or the iterator's `return()`, does: `disconnect` closes stdin and waits for cleanup before it
/// returns, and what was queued is discarded. A second read is unrepresentable because `disconnect` consumes the
/// query.
async fn stop_after_the_first(through_stream: bool) -> Result<(Vec<String>, Vec<String>), Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let log = Log::default();
    let server = tokio::spawn(serve(harness, typescript_steps(), &[], Arc::clone(&log)));
    let mut query = query_with_adapters(Prompt::from("hi"), typescript_options(), adapters(process)).await?;
    let message = if through_stream {
        query.next().await
    } else {
        query.next_message().await.transpose()
    };
    let message = message.ok_or("the query ended before its first message")??;
    tokio::time::sleep(Duration::from_millis(300)).await;
    query.disconnect().await?;
    let cli_saw = events(&log)?;
    server.await?.map_err(|error| error.to_string())?;
    Ok((vec![label(&message)], cli_saw))
}

/// TypeScript `for await (const message of query) { break }` and `Query.return()`, which both await `cleanup()`.
#[tokio::test]
async fn breaking_out_and_return_map_to_disconnect_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;

    let (yielded, cli_saw) = stop_after_the_first(true).await?;
    assert_eq!(yielded, captured.strings("for_await_break", "yielded")?);
    assert_eq!(
        cli_saw,
        captured.strings("for_await_break", "cli_saw_when_break_returned")?
    );
    assert_eq!(captured.field("for_await_break", "next_after_break")?, "done");

    let (yielded, cli_saw) = stop_after_the_first(false).await?;
    assert_eq!(yielded, captured.strings("return_discards_queued", "yielded")?);
    assert_eq!(
        cli_saw,
        captured.strings("return_discards_queued", "cli_saw_when_return_resolved")?
    );
    assert_eq!(captured.field("return_discards_queued", "return_result")?, "done");
    assert_eq!(captured.field("return_discards_queued", "next_after_return")?, "done");
    Ok(())
}

/// Python `ClaudeSDKClient.receive_response()` stops after the first result and leaves what follows queued;
/// `receive_messages()` then reads on from there.
#[tokio::test]
async fn receive_response_stops_after_the_result_like_python() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let scenario = "receive_response_stops_after_the_result";
    let (process, harness) = ScriptedProcessSpawner::new();
    let steps = vec![Step::Said("A"), Step::Result(1), Step::Said("BB"), Step::Result(2)];
    let server = tokio::spawn(serve(harness, steps, &[], Log::default()));
    let mut client = ClaudeSdkClient::connect_with_adapters(python_options(), adapters(process)).await?;
    client.query(Prompt::from("hi")).await?;

    let (response, error) = tokio::time::timeout(Duration::from_secs(5), drain(client.receive_response())).await?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(response, captured.strings(scenario, "receive_response")?);
    assert_eq!(captured.field(scenario, "receive_response_ended")?, "done");

    let mut rest = Vec::new();
    {
        let mut messages = pin!(client.receive_messages());
        while let Some(message) = tokio::time::timeout(Duration::from_secs(5), messages.next()).await? {
            let message = message?;
            rest.push(label(&message));
            if matches!(message, Message::Result(_)) {
                break;
            }
        }
    }
    assert_eq!(rest, captured.strings(scenario, "receive_messages")?);
    assert_eq!(captured.field(scenario, "receive_messages_ended")?, "done");
    client.disconnect().await?;
    server.await?.map_err(|error| error.to_string())?;
    Ok(())
}

/// Python `ClaudeSDKClient.receive_response()` when the CLI exits with status 0 before a result: it yields what
/// arrived and ends quietly, and `receive_messages()` then ends at once.
#[tokio::test]
async fn receive_response_ends_with_the_stream_like_python() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let scenario = "receive_response_ends_with_the_stream";
    let (process, harness) = ScriptedProcessSpawner::new();
    let steps = vec![Step::Said("A"), Step::Wait(100), Step::Exit];
    let server = tokio::spawn(serve(harness, steps, &[], Log::default()));
    let mut client = ClaudeSdkClient::connect_with_adapters(python_options(), adapters(process)).await?;
    client.query(Prompt::from("hi")).await?;

    let (response, error) = tokio::time::timeout(Duration::from_secs(5), drain(client.receive_response())).await?;
    assert_eq!(response, captured.strings(scenario, "receive_response")?);
    assert_eq!(captured.field(scenario, "receive_response_ended")?, "done");
    assert!(error.is_none(), "{error:?}");

    let (rest, error) = tokio::time::timeout(Duration::from_secs(5), drain(client.receive_messages())).await?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(rest, captured.strings(scenario, "receive_messages")?);
    assert_eq!(captured.field(scenario, "receive_messages_ended")?, "done");
    client.disconnect().await?;
    server.await?.map_err(|error| error.to_string())?;
    Ok(())
}

/// TypeScript `for await (const message of query)` when the CLI exits with status 0 before a result: it yields
/// what arrived and ends without an error, and the iterator stays ended.
#[tokio::test]
async fn the_query_ends_with_the_stream_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let scenario = "query_ends_with_the_stream";
    let (process, harness) = ScriptedProcessSpawner::new();
    let steps = vec![Step::Said("A"), Step::Wait(100), Step::Exit];
    let server = tokio::spawn(serve(harness, steps, &[], Log::default()));
    let mut query = query_with_adapters(Prompt::from("hi"), typescript_options(), adapters(process)).await?;

    let (yielded, error) = tokio::time::timeout(Duration::from_secs(5), drain(&mut query)).await?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(yielded, captured.strings(scenario, "yielded")?);
    assert_eq!(captured.field(scenario, "ended")?, "done");
    assert!(query.next().await.is_none());
    assert_eq!(captured.field(scenario, "next_after_end")?, "done");
    query.disconnect().await?;
    server.await?.map_err(|error| error.to_string())?;
    Ok(())
}

/// `EndWithoutResult::Fail` is the opt-in that reports the same clean exit as `Error::UnexpectedEof`.
#[tokio::test]
async fn the_opt_in_limit_fails_a_clean_exit_without_a_result() -> TestResult {
    let (process, harness) = ScriptedProcessSpawner::new();
    let steps = vec![Step::Said("A"), Step::Wait(100), Step::Exit];
    let server = tokio::spawn(serve(harness, steps, &[], Log::default()));
    let mut options = typescript_options();
    options.limits.end_without_result = EndWithoutResult::Fail;
    let mut query = query_with_adapters(Prompt::from("hi"), options, adapters(process)).await?;

    let (yielded, error) = tokio::time::timeout(Duration::from_secs(5), drain(&mut query)).await?;
    assert_eq!(yielded, vec!["A".to_owned()]);
    assert!(matches!(error, Some(Error::UnexpectedEof)), "{error:?}");
    query.disconnect().await?;
    server.await?.map_err(|error| error.to_string())?;
    Ok(())
}
