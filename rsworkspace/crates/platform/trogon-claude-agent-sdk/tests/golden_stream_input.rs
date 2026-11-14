#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Pins `stream_input` against TypeScript `Query.streamInput` (`sdk.mjs` 0.3.323) and Python
//! `Query.stream_input` (`claude_agent_sdk` 0.2.181), both recorded against a scripted CLI: what the CLI read on
//! stdin, whether stdin closed while the session was still open, and what the call returned.
//!
//! TypeScript, the default, stops at an abort and resolves, rejects a write after `close()`, rejects with the
//! stream's error and leaves stdin open, and queues behind `promptHold` when a blocking hook waits for
//! initialize. Python stops at a close, logs a failing stream and still closes stdin, and never raises.

use futures_util::{Stream, StreamExt, stream};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, ClientHandle, HookEvent, HookFailureMode, HookOutput,
    HookRegistration, HookRequest, PermissionDecision, Prompt, ToolPermissionRequest, UserInput, VerbatimPrompts,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/captured.rs"]
mod captured;
#[path = "support/scripted_process.rs"]
mod scripted_process;

use captured::Captured;
use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Failure = Box<dyn std::error::Error + Send + Sync>;
type Stdout = Arc<tokio::sync::Mutex<tokio::io::DuplexStream>>;
type Log = Arc<Mutex<Vec<String>>>;

const CAPTURED: &str = include_str!("fixtures/auth_callbacks/stream_input.json");

/// What the caller's stream yields when it fails: the upstream iterable throwing `Error("boom")`.
#[derive(Debug)]
enum StreamError {
    Stream(&'static str),
    Sdk(trogon_claude_agent_sdk::Error),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream(message) => formatter.write_str(message),
            Self::Sdk(error) => error.fmt(formatter),
        }
    }
}

impl From<trogon_claude_agent_sdk::Error> for StreamError {
    fn from(error: trogon_claude_agent_sdk::Error) -> Self {
        Self::Sdk(error)
    }
}

type Item = Result<UserInput, StreamError>;
type Inputs = Pin<Box<dyn Stream<Item = Item>>>;

/// How the scripted CLI answers, as the recording stub did.
#[derive(Clone, Copy, Default)]
struct Cli {
    /// Answer every prompt with a result, 100 ms after the first prompt still unanswered.
    results: bool,
    /// Reject initialize with this message.
    init_error: Option<&'static str>,
    /// Answer initialize this many milliseconds late.
    init_delay: u64,
}

fn note(log: &Log, entry: &str) -> Result<(), Failure> {
    log.lock().map_err(|_| "lock poisoned")?.push(entry.to_owned());
    Ok(())
}

async fn reply(stdout: Stdout, log: Log, delay: u64, frames: Vec<(&'static str, Value)>) -> Result<(), Failure> {
    tokio::time::sleep(Duration::from_millis(delay)).await;
    let mut out = stdout.lock().await;
    for (label, frame) in frames {
        let mut line = serde_json::to_vec(&frame)?;
        line.push(b'\n');
        note(&log, label)?;
        out.write_all(&line).await?;
    }
    Ok(())
}

fn result() -> Value {
    json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,
        "num_turns":1,"result":"ok","session_id":"s","total_cost_usd":0,"usage":{},"modelUsage":{},
        "permission_denials":[],"uuid":"00000000-0000-4000-8000-000000000002"})
}

/// What the CLI saw: every stdin line, what it sent, and `eof` when stdin closed before the test shut the
/// session down; plus the `client_composed` flag of each user message.
struct Seen {
    cli: Vec<String>,
    client_composed: Vec<bool>,
}

async fn serve(process: ScriptedProcessHarness, cli: Cli, closing: Arc<AtomicBool>) -> Result<Seen, Failure> {
    let mut lines = BufReader::new(process.stdin).lines();
    let stdout: Stdout = Arc::new(tokio::sync::Mutex::new(process.stdout));
    let stderr = process.stderr;
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let unanswered = Arc::new(AtomicUsize::new(0));
    let mut client_composed = Vec::new();
    let mut tasks = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let frame: Value = serde_json::from_str(&line)?;
        if frame["type"] == "user" {
            let content = &frame["message"]["content"];
            let text = content
                .as_str()
                .or_else(|| content[0]["text"].as_str())
                .unwrap_or_default();
            note(&log, text)?;
            client_composed.push(frame["client_composed"] == true);
            // Prompts that arrive together are answered together, so the order against stdin closing is stable.
            if cli.results && unanswered.fetch_add(1, Ordering::SeqCst) == 0 {
                let unanswered = Arc::clone(&unanswered);
                let (stdout, log) = (Arc::clone(&stdout), Arc::clone(&log));
                tasks.push(tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let count = unanswered.swap(0, Ordering::SeqCst);
                    reply(stdout, log, 0, (0..count).map(|_| ("result", result())).collect()).await
                }));
            }
            continue;
        }
        let subtype = frame["request"]["subtype"].as_str().unwrap_or_default();
        note(&log, subtype)?;
        let id = frame["request_id"].clone();
        let (answer, delay) = match (subtype, cli.init_error) {
            ("initialize", Some(error)) => (
                json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":error}}),
                cli.init_delay,
            ),
            ("initialize", None) => (
                json!({"type":"control_response","response":{"subtype":"success","request_id":id,
                    "response":{"hook_on_failure_supported":true,"commands":[],"agents":[]}}}),
                cli.init_delay,
            ),
            _ => (
                json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{}}}),
                0,
            ),
        };
        let answering = reply(
            Arc::clone(&stdout),
            Arc::clone(&log),
            delay,
            vec![("control_response", answer)],
        );
        // The recording stub answered an immediate request before it read the next line: `sdk.mjs` writes the
        // first prompt right behind initialize without waiting for the answer, yet every recording logs the answer
        // first. Only a late answer runs beside the reads.
        if delay == 0 {
            answering.await?;
        } else {
            tasks.push(tokio::spawn(answering));
        }
    }
    if !closing.load(Ordering::SeqCst) {
        note(&log, "eof")?;
    }
    // The stub exits once stdin closes, so an answer still waiting is never sent.
    for task in tasks {
        task.abort();
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    let cli = log.lock().map_err(|_| "lock poisoned")?.clone();
    Ok(Seen { cli, client_composed })
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

fn typescript() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        ..ClaudeAgentOptions::default()
    }
}

fn python() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        ..typescript()
    }
}

/// `canUseTool` / `can_use_tool`: a need that keeps stdin open until the run ends.
fn with_needs(mut options: ClaudeAgentOptions) -> ClaudeAgentOptions {
    options.permission_handler = Some(Arc::new(|_: ToolPermissionRequest| async {
        PermissionDecision::allow()
    }));
    options
}

/// A `PreToolUse` hook with `onFailure: "block"`, which puts the prompts behind `promptHold`.
fn with_blocking_hook(mut options: ClaudeAgentOptions) -> ClaudeAgentOptions {
    options.hooks = vec![HookRegistration {
        event: HookEvent::PreToolUse,
        matcher: None,
        callbacks: vec![Arc::new(|_request: HookRequest| async { Ok(HookOutput::default()) })],
        timeout: None,
        on_failure: Some(HookFailureMode::Block),
    }];
    options
}

fn verbatim(mut options: ClaudeAgentOptions) -> ClaudeAgentOptions {
    options.verbatim_prompts = VerbatimPrompts::Enabled;
    options
}

fn input(text: &str) -> Item {
    Ok(UserInput::from(Prompt::from(text)))
}

fn ready(items: Vec<Item>) -> Inputs {
    Box::pin(stream::iter(items))
}

/// Yields `first`, then waits for `then` and yields what it returns.
fn after(first: Vec<Item>, then: impl Future<Output = Vec<Item>> + 'static) -> Inputs {
    Box::pin(stream::iter(first).chain(stream::once(then).map(stream::iter).flatten()))
}

/// Connects, streams what `inputs` builds, waits as the recording did, then shuts down.
async fn run(
    options: ClaudeAgentOptions,
    cli: Cli,
    inputs: impl FnOnce(ClientHandle) -> Inputs,
) -> Result<(String, Seen), Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let closing = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(serve(harness, cli, Arc::clone(&closing)));
    let client = ClaudeSdkClient::connect_with_adapters(options, adapters(process)).await?;
    let outcome = match client.try_stream_input(inputs(client.handle())).await {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("rejected: {error}"),
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    closing.store(true, Ordering::SeqCst);
    // A run the abort or a failed initialize ended reports that error here; only stdin is under test.
    let _ = client.disconnect().await;
    let seen = server.await?.map_err(|error| error.to_string())?;
    Ok((outcome, seen))
}

/// Like [`run`], beside an earlier `stream_input` that wrote `m1` and stays open, so the session has written a
/// prompt this call did not.
async fn run_beside_an_open_stream(
    options: ClaudeAgentOptions,
    cli: Cli,
    inputs: Inputs,
) -> Result<(String, Seen), Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let closing = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(serve(harness, cli, Arc::clone(&closing)));
    let client = ClaudeSdkClient::connect_with_adapters(options, adapters(process)).await?;
    let handle = client.handle();
    let earlier = tokio::spawn(async move {
        let open = stream::iter([UserInput::from(Prompt::from("m1"))]).chain(stream::pending());
        handle.stream_input(open).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let outcome = match client.try_stream_input(inputs).await {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("rejected: {error}"),
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    closing.store(true, Ordering::SeqCst);
    earlier.abort();
    let _ = client.disconnect().await;
    let seen = server.await?.map_err(|error| error.to_string())?;
    Ok((outcome, seen))
}

fn assert_matches(scenario: &str, (outcome, seen): &(String, Seen)) -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let expected = match captured.field(scenario, "outcome")?.as_str() {
        Some("rejected") => format!(
            "rejected: {}",
            captured.field(scenario, "error")?.as_str().ok_or("not a string")?
        ),
        Some("resolved" | "returned") => "ok".to_owned(),
        other => return Err(format!("{scenario}: unknown outcome {other:?}").into()),
    };
    assert_eq!(*outcome, expected, "{scenario}: what the call returned");
    assert_eq!(
        seen.cli,
        captured.strings(scenario, "cli")?,
        "{scenario}: what the CLI saw"
    );
    let composed = captured
        .field(scenario, "client_composed")?
        .as_array()
        .ok_or("not a list")?
        .iter()
        .map(|flag| flag.as_bool().unwrap_or(false))
        .collect::<Vec<_>>();
    assert_eq!(seen.client_composed, composed, "{scenario}: client_composed");
    Ok(())
}

/// TypeScript `Query.streamInput`: nothing written closes stdin at once, so do prompts nothing needs, and with a
/// need stdin stays open until the run ends. `verbatimPrompts` marks each prompt `client_composed`.
#[tokio::test]
async fn typescript_query_stream_input_ends_the_input_like_sdk_mjs() -> TestResult {
    let cli = Cli::default();
    let results = Cli {
        results: true,
        ..Cli::default()
    };
    let two = || ready(vec![input("m1"), input("m2")]);
    assert_matches("empty_ts", &run(typescript(), cli, |_| ready(Vec::new())).await?)?;
    assert_matches("two_no_needs_ts", &run(typescript(), cli, |_| two()).await?)?;
    assert_matches(
        "two_needs_ts",
        &run(with_needs(typescript()), results, |_| two()).await?,
    )?;
    assert_matches(
        "verbatim_ts",
        &run(verbatim(typescript()), cli, |_| ready(vec![input("m1")])).await?,
    )
}

/// TypeScript `Query.streamInput` breaks out once the abort signal fired and resolves; nothing after it is
/// written.
#[tokio::test]
async fn typescript_query_stream_input_stops_at_an_abort() -> TestResult {
    let seen = run(typescript(), Cli::default(), |handle| {
        after(vec![input("m1")], async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = handle.abort().await;
            vec![input("m2"), input("m3")]
        })
    })
    .await?;
    assert_matches("abort_mid_ts", &seen)
}

/// TypeScript `Query.streamInput` rejects a write after `close()` with "ProcessTransport is not ready for
/// writing".
#[tokio::test]
async fn typescript_query_stream_input_rejects_a_write_after_close() -> TestResult {
    let seen = run(typescript(), Cli::default(), |handle| {
        after(vec![input("m1")], async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            handle.close();
            vec![input("m2"), input("m3")]
        })
    })
    .await?;
    assert_matches("close_mid_ts", &seen)
}

/// TypeScript `Query.streamInput` rejects with the iterable's error and never calls `endInput()`, so stdin stays
/// open.
#[tokio::test]
async fn typescript_query_stream_input_rejects_with_a_failing_stream_and_leaves_stdin_open() -> TestResult {
    let seen = run(typescript(), Cli::default(), |_| {
        after(vec![input("m1")], async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            vec![Err(StreamError::Stream("boom"))]
        })
    })
    .await?;
    assert_matches("throws_after_one_ts", &seen)?;
    let seen = run(typescript(), Cli::default(), |_| {
        ready(vec![Err(StreamError::Stream("boom"))])
    })
    .await?;
    assert_matches("throws_first_ts", &seen)
}

/// TypeScript `Query.streamInput` behind `promptHold`: the prompts go out once initialize is confirmed; a failed
/// initialize resolves without writing them, and so does a `close()` before the answer.
#[tokio::test]
async fn typescript_query_stream_input_waits_behind_the_prompt_hold() -> TestResult {
    let late = |results, init_error| Cli {
        results,
        init_error,
        init_delay: 200,
    };
    let two = |_| ready(vec![input("m1"), input("m2")]);
    let held = with_blocking_hook(typescript());
    assert_matches("hold_ok_ts", &run(held.clone(), late(true, None), two).await?)?;
    assert_matches(
        "hold_failed_ts",
        &run(held.clone(), late(false, Some("nope")), two).await?,
    )?;
    let seen = run(
        held,
        Cli {
            init_delay: 400,
            ..Cli::default()
        },
        |handle| {
            after(vec![input("m1")], async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                handle.close();
                vec![input("m2")]
            })
        },
    )
    .await?;
    assert_matches("hold_closed_ts", &seen)
}

/// Python `Query.stream_input`: nothing written closes stdin at once, and so do prompts nothing needs; with a
/// need, `wait_for_result_and_end_input` holds stdin until the run ends. `verbatim_prompts` stamps
/// `client_composed`.
#[tokio::test]
async fn python_query_stream_input_ends_the_input_like_claude_agent_sdk() -> TestResult {
    let cli = Cli::default();
    let results = Cli {
        results: true,
        ..Cli::default()
    };
    let two = || ready(vec![input("m1"), input("m2")]);
    assert_matches("empty_py", &run(python(), cli, |_| ready(Vec::new())).await?)?;
    assert_matches("two_no_needs_py", &run(python(), cli, |_| two()).await?)?;
    assert_matches("two_needs_py", &run(with_needs(python()), results, |_| two()).await?)?;
    assert_matches(
        "verbatim_py",
        &run(verbatim(python()), cli, |_| ready(vec![input("m1")])).await?,
    )
}

/// Python `Query.stream_input` checks `_closed` before each write and returns quietly.
#[tokio::test]
async fn python_query_stream_input_stops_at_a_close() -> TestResult {
    let seen = run(python(), Cli::default(), |handle| {
        after(vec![input("m1")], async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            handle.close();
            vec![input("m2"), input("m3")]
        })
    })
    .await?;
    assert_matches("close_mid_py", &seen)
}

/// Python `Query.stream_input` logs "Prompt stream failed; closing stdin" and still ends the input: at once when
/// nothing needs the CLI or nothing was written, after the run otherwise. The caller never sees the error.
#[tokio::test]
async fn python_query_stream_input_closes_stdin_after_a_failing_stream() -> TestResult {
    let boom_after_one = |_| {
        after(vec![input("m1")], async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            vec![Err(StreamError::Stream("boom"))]
        })
    };
    assert_matches(
        "throws_after_one_py",
        &run(python(), Cli::default(), boom_after_one).await?,
    )?;
    let results = Cli {
        results: true,
        ..Cli::default()
    };
    assert_matches(
        "throws_after_one_needs_py",
        &run(with_needs(python()), results, boom_after_one).await?,
    )?;
    let seen = run(python(), Cli::default(), |_| {
        ready(vec![Err(StreamError::Stream("boom"))])
    })
    .await?;
    assert_matches("throws_first_py", &seen)
}

/// Both SDKs count the prompts of each call: with nothing written by this call, stdin closes at once although an
/// earlier call wrote a prompt whose run a need still waits on.
#[tokio::test]
async fn stream_input_counts_the_prompts_of_its_own_call() -> TestResult {
    let results = Cli {
        results: true,
        ..Cli::default()
    };
    assert_matches(
        "empty_after_prompt_needs_ts",
        &run_beside_an_open_stream(with_needs(typescript()), results, ready(Vec::new())).await?,
    )?;
    assert_matches(
        "empty_after_prompt_needs_py",
        &run_beside_an_open_stream(with_needs(python()), results, ready(Vec::new())).await?,
    )
}

/// The infallible `ClientHandle::stream_input` is the same machine over a stream that cannot fail.
#[tokio::test]
async fn the_infallible_stream_input_streams_like_query_stream_input() -> TestResult {
    let (process, harness) = ScriptedProcessSpawner::new();
    let closing = Arc::new(AtomicBool::new(false));
    let server = tokio::spawn(serve(harness, Cli::default(), Arc::clone(&closing)));
    let client = ClaudeSdkClient::connect_with_adapters(typescript(), adapters(process)).await?;
    client
        .handle()
        .stream_input(stream::iter([
            UserInput::from(Prompt::from("m1")),
            UserInput::from(Prompt::from("m2")),
        ]))
        .await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    closing.store(true, Ordering::SeqCst);
    client.disconnect().await?;
    let seen = server.await?.map_err(|error| error.to_string())?;
    assert_matches("two_no_needs_ts", &("ok".to_owned(), seen))
}
