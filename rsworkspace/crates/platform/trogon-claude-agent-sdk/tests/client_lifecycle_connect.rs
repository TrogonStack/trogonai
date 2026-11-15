#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests for Python `ClaudeSDKClient`'s lifecycle: construction, `connect(prompt)`, `async with`, and
//! `query(prompt, session_id)`. The expected user lines were captured from `claude_agent_sdk` 0.2.181 and, for the
//! string prompt, `sdk.mjs` 0.3.323, each talking to a scripted CLI. Lines are compared as parsed JSON because key
//! order is not pinned here.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, ConnectPrompt, ControlOperation, Error, Message, Prompt, SessionId,
    Timeout, UnconnectedClient, UserInput, VerbatimPrompts,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Server = tokio::task::JoinHandle<Result<Vec<Value>, std::io::Error>>;
type Unconnected = UnconnectedClient<RuntimeAdapters<FixedEnvironment, ScriptedProcessSpawner>>;

const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"result":"ok","session_id":"s","total_cost_usd":0,"usage":{},"modelUsage":{},"permission_denials":[],"uuid":"00000000-0000-4000-8000-000000000002"}"#;

/// Answers each control request with `{}` unless its subtype is `silent`, answers each user line with a result,
/// and records every line written to stdin.
async fn serve(
    process: ScriptedProcessHarness,
    silent: &'static [&'static str],
    mut interrupted: Option<tokio::sync::oneshot::Sender<()>>,
) -> Result<Vec<Value>, std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut written = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if request["request"]["subtype"] == "interrupt"
            && let Some(interrupted) = interrupted.take()
        {
            let _ = interrupted.send(());
        }
        let reply = match request["type"].as_str() {
            Some("control_request") if !silent.iter().any(|subtype| request["request"]["subtype"] == *subtype) => {
                Some(serde_json::to_string(&json!({"type":"control_response","response":{
                    "subtype":"success","request_id":request["request_id"],"response":{}}}))?)
            }
            Some("user") => Some(RESULT.to_owned()),
            _ => None,
        };
        if let Some(mut reply) = reply {
            reply.push('\n');
            stdout.write_all(reply.as_bytes()).await?;
        }
        written.push(request);
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(written)
}

/// `ClaudeSDKClient(options)` against the scripted CLI, before `connect()`.
fn unconnected(
    style: ArgvStyle,
    configure: impl FnOnce(&mut ClaudeAgentOptions),
    silent: &'static [&'static str],
) -> (Unconnected, Server) {
    unconnected_observed(style, configure, silent, None)
}

fn unconnected_observed(
    style: ArgvStyle,
    configure: impl FnOnce(&mut ClaudeAgentOptions),
    silent: &'static [&'static str],
    interrupted: Option<tokio::sync::oneshot::Sender<()>>,
) -> (Unconnected, Server) {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve(harness, silent, interrupted));
    let mut options = ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        argv_style: style,
        ..ClaudeAgentOptions::default()
    };
    configure(&mut options);
    let client = ClaudeSdkClient::new(options).with_adapters(RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-directory"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    });
    (client, server)
}

async fn close_then_cancel_pending_python_control(deadline: Option<Timeout>) -> TestResult {
    let (observed, received) = tokio::sync::oneshot::channel();
    let (client, server) = unconnected_observed(
        ArgvStyle::Python,
        |options| options.limits.control_timeout = deadline,
        &["interrupt"],
        Some(observed),
    );
    let client = client.connect(ConnectPrompt::Empty).await?;
    let handle = client.handle();
    let waiting = tokio::spawn(async move { handle.interrupt().await });
    received.await?;
    client.disconnect().await?;
    server.await??;
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished(), "Python close must leave the caller waiting");
    waiting.abort();
    assert!(waiting.await.is_err_and(|error| error.is_cancelled()));
    tokio::task::yield_now().await;
    Ok(())
}

#[tokio::test]
async fn python_close_keeps_an_unbounded_control_pending_until_the_caller_cancels() -> TestResult {
    close_then_cancel_pending_python_control(None).await
}

#[tokio::test]
async fn python_close_keeps_a_timed_control_pending_when_the_caller_cancels_before_its_deadline() -> TestResult {
    close_then_cancel_pending_python_control(Some(Timeout::new(Duration::from_secs(60)))).await
}

/// The user lines the scripted CLI received, after checking that initialize came first.
async fn user_lines(server: Server) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let written = server.await??;
    assert_eq!(
        written.first().map(|line| &line["request"]["subtype"]),
        Some(&json!("initialize"))
    );
    Ok(written.into_iter().filter(|line| line["type"] == "user").collect())
}

/// Connects with `prompt`, runs `body`, disconnects, and returns the user lines written.
async fn record(
    style: ArgvStyle,
    configure: impl FnOnce(&mut ClaudeAgentOptions),
    prompt: impl Into<ConnectPrompt>,
    body: impl AsyncFnOnce(&ClaudeSdkClient) -> Result<(), Error>,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let (client, server) = unconnected(style, configure, &[]);
    let client = client.connect(prompt).await?;
    body(&client).await?;
    client.disconnect().await?;
    user_lines(server).await
}

fn python_user(content: &str, session_id: &str) -> Value {
    json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":null,"session_id":session_id})
}

fn input(content: &str, session_id: Option<&str>) -> UserInput {
    let mut input = UserInput::from(Prompt::from(content));
    input.session_id = session_id.map(SessionId::from);
    input
}

async fn nothing(_: &ClaudeSdkClient) -> Result<(), Error> {
    Ok(())
}

/// Python `ClaudeSDKClient.connect(None)`: only initialize is written and stdin stays open until disconnect.
#[tokio::test]
async fn connect_without_a_prompt_writes_nothing_after_initialize() -> TestResult {
    for style in [ArgvStyle::Python, ArgvStyle::TypeScript] {
        let lines = record(style, |_| {}, ConnectPrompt::Empty, nothing).await?;
        assert_eq!(lines, Vec::<Value>::new(), "{style:?}");
    }
    Ok(())
}

/// Python `ClaudeSDKClient.connect(str)`: the prompt follows initialize in session `"default"`.
#[tokio::test]
async fn connect_with_text_writes_it_in_the_default_session() -> TestResult {
    let lines = record(ArgvStyle::Python, |_| {}, "hi", nothing).await?;
    assert_eq!(lines, [python_user("hi", "default")]);
    Ok(())
}

/// Python `ClaudeSDKClient.connect(str)` with `verbatim_prompts`: the prompt is stamped `client_composed`.
#[tokio::test]
async fn connect_with_verbatim_text_stamps_it() -> TestResult {
    let verbatim = |options: &mut ClaudeAgentOptions| options.verbatim_prompts = VerbatimPrompts::Enabled;
    let lines = record(ArgvStyle::Python, verbatim, "hi", nothing).await?;
    let mut expected = python_user("hi", "default");
    expected["client_composed"] = json!(true);
    assert_eq!(lines, [expected]);
    Ok(())
}

/// `sdk.mjs` `query({prompt: string})`: the TypeScript style writes the prompt in the empty session, as one text
/// block.
#[tokio::test]
async fn connect_with_text_under_typescript_uses_the_empty_session() -> TestResult {
    let lines = record(ArgvStyle::TypeScript, |_| {}, "x", nothing).await?;
    assert_eq!(
        lines,
        [
            json!({"type":"user","session_id":"","message":{"role":"user","content":[{"type":"text","text":"x"}]},
                "parent_tool_use_id":null})
        ]
    );
    Ok(())
}

/// Python `ClaudeSDKClient.connect(AsyncIterable)`: a background task streams the inputs as given, then ends
/// stdin. An input that names no session goes without one, as Python writes it.
#[tokio::test]
async fn connect_with_a_stream_streams_it_in_the_background() -> TestResult {
    for style in [ArgvStyle::Python, ArgvStyle::TypeScript] {
        let (client, server) = unconnected(style, |_| {}, &[]);
        let stream = futures_util::stream::iter([input("one", None), input("two", Some("keep"))]);
        let client = client.connect(ConnectPrompt::stream(stream)).await?;
        let lines = user_lines(server).await?;
        client.disconnect().await?;
        let unnamed = json!({"type":"user","message":{"role":"user","content":"one"},"parent_tool_use_id":null});
        assert_eq!(lines, [unnamed, python_user("two", "keep")], "{style:?}");
    }
    Ok(())
}

/// Python `ClaudeSDKClient.query(str)`: session `"default"` when none is named.
#[tokio::test]
async fn query_writes_in_the_default_session() -> TestResult {
    let lines = record(
        ArgvStyle::Python,
        |_| {},
        ConnectPrompt::Empty,
        async |client| client.query(Prompt::from("x")).await,
    )
    .await?;
    assert_eq!(lines, [python_user("x", "default")]);
    Ok(())
}

/// Python `ClaudeSDKClient.query(str, session_id)`.
#[tokio::test]
async fn query_in_session_writes_in_that_session() -> TestResult {
    let lines = record(
        ArgvStyle::Python,
        |_| {},
        ConnectPrompt::Empty,
        async |client| client.query_in_session(Prompt::from("x"), "s").await,
    )
    .await?;
    assert_eq!(lines, [python_user("x", "s")]);
    Ok(())
}

/// Python `ClaudeSDKClient.query(AsyncIterable)`: inputs that name no session go in `"default"`.
#[tokio::test]
async fn query_stream_fills_the_default_session() -> TestResult {
    let lines = record(
        ArgvStyle::Python,
        |_| {},
        ConnectPrompt::Empty,
        async |client| {
            client
                .query_stream(futures_util::stream::iter([input("one", None)]))
                .await
        },
    )
    .await?;
    assert_eq!(lines, [python_user("one", "default")]);
    Ok(())
}

/// Python `ClaudeSDKClient.query(AsyncIterable, session_id)`: only inputs that name no session take
/// `session_id`, and stdin stays open.
#[tokio::test]
async fn query_stream_in_session_keeps_named_sessions() -> TestResult {
    let lines = record(
        ArgvStyle::Python,
        |_| {},
        ConnectPrompt::Empty,
        async |client| -> Result<(), Error> {
            let stream = futures_util::stream::iter([input("one", None), input("two", Some("keep"))]);
            client.query_stream_in_session(stream, "q").await?;
            client.query(Prompt::from("after")).await
        },
    )
    .await?;
    assert_eq!(
        lines,
        [
            python_user("one", "q"),
            python_user("two", "keep"),
            python_user("after", "default")
        ]
    );
    Ok(())
}

/// Python `async with ClaudeSDKClient() as client:` (`__aenter__`, `__aexit__`): connects with no prompt, runs the
/// body, then disconnects.
#[tokio::test]
async fn scope_connects_runs_the_body_and_disconnects() -> TestResult {
    let (client, server) = unconnected(ArgvStyle::Python, |_| {}, &[]);
    let result = client
        .scope(async |client| -> Result<Option<String>, Error> {
            client.query(Prompt::from("x")).await?;
            while let Some(message) = client.next_message().await? {
                if let Message::Result(result) = message {
                    return Ok(result.result);
                }
            }
            Err(Error::UnexpectedEof)
        })
        .await?;
    assert_eq!(result.as_deref(), Some("ok"));
    assert_eq!(user_lines(server).await?, [python_user("x", "default")]);
    Ok(())
}

/// Python `__aexit__` disconnects and returns `False`, so the body's exception propagates unchanged.
#[tokio::test]
async fn scope_returns_the_body_error_after_disconnecting() -> TestResult {
    let (client, server) = unconnected(ArgvStyle::Python, |_| {}, &[]);
    let outcome = client
        .scope(async |client| -> Result<(), Error> {
            client.query(Prompt::from("x")).await?;
            Err(Error::QueryClosed)
        })
        .await;
    assert!(matches!(outcome, Err(Error::QueryClosed)), "{outcome:?}");
    assert_eq!(user_lines(server).await?, [python_user("x", "default")]);
    Ok(())
}

/// When the body and the disconnect both fail, Python raises the disconnect's exception chained to the body's;
/// the scope returns both as `Error::Cleanup`.
#[tokio::test]
async fn scope_reports_a_failed_disconnect_beside_the_body_error() -> TestResult {
    let short = |options: &mut ClaudeAgentOptions| {
        options.limits.control_timeout = Some(Timeout::new(Duration::from_millis(100)));
    };
    let (client, server) = unconnected(ArgvStyle::TypeScript, short, &["interrupt"]);
    let outcome = client
        .scope(async |client| client.handle().interrupt().await.map(drop))
        .await;
    let interrupt = |error: &Error| {
        matches!(
            error,
            Error::ControlTimeout {
                operation: ControlOperation::Interrupt
            }
        )
    };
    assert!(
        matches!(&outcome, Err(Error::Cleanup { primary, cleanup }) if interrupt(primary) && interrupt(cleanup)),
        "{outcome:?}"
    );
    server.await??;
    Ok(())
}

/// Python `ClaudeSDKClient.connect(str)` with the prompt given as an owned string or as a prompt value.
#[tokio::test]
async fn connect_takes_an_owned_string_or_a_prompt() -> TestResult {
    let owned = record(ArgvStyle::Python, |_| {}, String::from("owned"), nothing).await?;
    assert_eq!(owned, [python_user("owned", "default")]);
    let prompt = record(ArgvStyle::Python, |_| {}, Prompt::from("prompt"), nothing).await?;
    assert_eq!(prompt, [python_user("prompt", "default")]);
    Ok(())
}

/// When only the disconnect fails, Python's `__aexit__` raises its exception even though the body returned; the
/// scope returns the disconnect's error.
#[tokio::test]
async fn scope_reports_a_failed_disconnect_after_a_body_that_succeeded() -> TestResult {
    let short = |options: &mut ClaudeAgentOptions| {
        options.limits.control_timeout = Some(Timeout::new(Duration::from_millis(100)));
    };
    let (client, server) = unconnected(ArgvStyle::TypeScript, short, &["interrupt"]);
    let outcome = client
        .scope(async |client| -> Result<(), Error> {
            let interrupted = client.handle().interrupt().await;
            assert!(interrupted.is_err(), "the silent CLI never answers the interrupt");
            Ok(())
        })
        .await;
    assert!(
        matches!(
            &outcome,
            Err(Error::ControlTimeout {
                operation: ControlOperation::Interrupt
            })
        ),
        "{outcome:?}"
    );
    server.await??;
    Ok(())
}
