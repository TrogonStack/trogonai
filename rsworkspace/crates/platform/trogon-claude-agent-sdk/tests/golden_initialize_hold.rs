#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Pins what the TypeScript style writes while the first initialize is unanswered and a hook is set to block on
//! failure or managed settings are sent. The expected order, bytes and rejections were captured from `sdk.mjs`
//! 0.3.323 talking to a scripted CLI, with only the random request id normalized: the prompt and a held request
//! wait for the initialize response, and when initialize fails nothing held is ever written and everything
//! unanswered is rejected with the initialize error.

use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, Error, HookEvent, HookFailureMode, HookOutput, HookRegistration, HookRequest,
    ManagedSettingsDelivery, Prompt, Query, Settings,
    adapters::{NativeFileSystem, RuntimeAdapters},
    query_with_adapters,
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

const CAPTURED: &str = include_str!("fixtures/auth_callbacks/initialize_hold.json");

/// How the scripted CLI answers `initialize`, a moment after it was written so that whatever the SDK writes
/// right behind it is already on the wire.
#[derive(Clone, Copy)]
enum Answer {
    Success(&'static str),
    Error(&'static str),
}

/// What the scripted CLI saw: the order of what it read and sent, and each line it read with the request id
/// normalized.
struct Wire {
    order: Vec<String>,
    lines: Vec<String>,
}

fn normalize(line: &str) -> String {
    match line
        .strip_prefix(r#"{"request_id":""#)
        .and_then(|rest| rest.split_once('"'))
    {
        Some((_, rest)) => format!(r#"{{"request_id":"<id>"{rest}"#),
        None => line.to_owned(),
    }
}

fn kind(frame: &Value) -> String {
    match frame["type"].as_str() {
        Some("control_request") => frame["request"]["subtype"].as_str().unwrap_or_default().to_owned(),
        Some("control_response") => "response".to_owned(),
        Some(other) => other.to_owned(),
        None => String::new(),
    }
}

async fn write(stdout: &mut tokio::io::DuplexStream, frame: &Value) -> Result<(), Failure> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    stdout.write_all(&line).await?;
    Ok(())
}

/// Answers every request but `initialize` at once, `initialize` after `delay`, and a user message with an
/// assistant message and a result.
async fn serve(process: ScriptedProcessHarness, answer: Answer, delay: Duration) -> Result<Wire, Failure> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut wire = Wire {
        order: Vec::new(),
        lines: Vec::new(),
    };
    let mut pending: Option<(Value, tokio::time::Instant)> = None;
    loop {
        let line = tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => line,
                None => break,
            },
            () = async {
                match &pending {
                    Some((_, at)) => tokio::time::sleep_until(*at).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some((id, _)) = pending.take() {
                    let response = match answer {
                        Answer::Success(field) => json!({"type":"control_response","response":{
                            "subtype":"success","request_id":id,"response":{field: true}}}),
                        Answer::Error(message) => json!({"type":"control_response","response":{
                            "subtype":"error","request_id":id,"error":message}}),
                    };
                    write(&mut stdout, &response).await?;
                    wire.order.push("cli answers initialize".to_owned());
                }
                continue;
            }
        };
        let frame: Value = serde_json::from_str(&line)?;
        wire.order.push(format!("sdk writes {}", kind(&frame)));
        wire.lines.push(normalize(&line));
        match frame["type"].as_str() {
            Some("control_request") if frame["request"]["subtype"] == "initialize" => {
                pending = Some((frame["request_id"].clone(), tokio::time::Instant::now() + delay));
            }
            Some("control_request") => {
                write(
                    &mut stdout,
                    &json!({"type":"control_response","response":{
                        "subtype":"success","request_id":frame["request_id"],"response":{}}}),
                )
                .await?;
            }
            Some("user") => {
                write(
                    &mut stdout,
                    &json!({"type":"assistant","session_id":"s","parent_tool_use_id":null,
                        "uuid":"00000000-0000-4000-8000-000000000001",
                        "message":{"id":"mA","type":"message","role":"assistant","model":"m",
                            "content":[{"type":"text","text":"A"}],"stop_reason":"end_turn","stop_sequence":null,
                            "usage":{"input_tokens":1,"output_tokens":1}}}),
                )
                .await?;
                write(
                    &mut stdout,
                    &json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,
                        "duration_api_ms":1,"num_turns":1,"session_id":"s","result":"done","queued_turn_count":0}),
                )
                .await?;
            }
            _ => {}
        }
    }
    wire.order.push("stdin closed".to_owned());
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(wire)
}

fn blocking_hook() -> HookRegistration {
    HookRegistration {
        event: HookEvent::PreToolUse,
        matcher: Some("Bash".into()),
        callbacks: vec![Arc::new(|_request: HookRequest| async { Ok(HookOutput::default()) })],
        timeout: None,
        on_failure: Some(HookFailureMode::Block),
    }
}

fn hooks() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        hooks: vec![blocking_hook()],
        ..ClaudeAgentOptions::default()
    }
}

fn managed() -> Result<ClaudeAgentOptions, serde_json::Error> {
    Ok(ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        managed_settings: Some(serde_json::from_str::<Settings>(
            r#"{"permissions":{"allow":["Read"]}}"#,
        )?),
        managed_settings_delivery: ManagedSettingsDelivery::Initialize,
        ..ClaudeAgentOptions::default()
    })
}

async fn start(
    options: ClaudeAgentOptions,
    answer: Answer,
) -> Result<(Query, tokio::task::JoinHandle<Result<Wire, Failure>>), Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve(harness, answer, Duration::from_millis(400)));
    // Like `query()` in `sdk.mjs`, this returns once the initialize request and the held prompt are queued.
    let query = tokio::time::timeout(
        Duration::from_secs(10),
        query_with_adapters(
            Prompt::from("hi"),
            options,
            RuntimeAdapters {
                environment: FixedEnvironment {
                    directory: PathBuf::from("unused-directory"),
                    variables: BTreeMap::new(),
                },
                process,
                filesystem: NativeFileSystem,
            },
        ),
    )
    .await??;
    Ok((query, server))
}

/// The text `sdk.mjs` would show for this error: a rejected request carries the CLI's message.
fn text(error: &Error) -> String {
    match error {
        Error::InitializationFailed(inner) => text(inner),
        Error::ControlRejected { rejection, .. } => rejection.message().to_owned(),
        other => other.to_string(),
    }
}

fn expected(captured: &Captured, scenario: &str, act: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = captured.field(scenario, "seen")?;
    Ok(value["acts"][act]["message"]
        .as_str()
        .ok_or("the captured request was not rejected")?
        .to_owned())
}

/// Reads the stream to its end, keeping the type of each message and the error that ended it.
async fn read_to_the_end(query: &mut Query) -> Result<(Vec<String>, Option<Error>), Box<dyn std::error::Error>> {
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match query.next_message().await {
                Ok(Some(message)) => {
                    seen.push(
                        message
                            .raw()
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    );
                }
                Ok(None) => return None,
                Err(error) => return Some(error),
            }
        }
    })
    .await
    .map(|error| (seen, error))
    .map_err(Into::into)
}

/// The types of the messages the captured run delivered, without the error that may have ended it.
fn captured_stream(captured: &Captured, scenario: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut types = Vec::new();
    for item in captured.field(scenario, "seen")?["stream"]
        .as_array()
        .ok_or("no captured stream")?
    {
        let entry = item.as_str().ok_or("a captured stream entry is not text")?;
        if !entry.starts_with("error:") {
            types.push(entry.split(':').next().unwrap_or_default().to_owned());
        }
    }
    Ok(types)
}

/// Everything after the initialize request, whose bytes the initialize goldens pin.
fn after_initialize(lines: &[String]) -> &[String] {
    lines.get(1..).unwrap_or_default()
}

#[tokio::test]
async fn a_blocking_hook_holds_the_prompt_and_an_interrupt_behind_initialize_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let scenario = "hold_hooks_success";
    let (mut query, server) = start(hooks(), Answer::Success("hook_on_failure_supported")).await?;
    let handle = query.handle().ok_or("the query has no handle")?;
    // Called in this order before initialize is answered: the interrupt waits behind it, the model change does not.
    let (interrupt, set_model) = tokio::join!(handle.interrupt(), handle.set_model(Some("m".into())));
    interrupt?;
    set_model?;
    let (messages, error) = read_to_the_end(&mut query).await?;
    assert!(error.is_none());
    assert_eq!(messages, captured_stream(&captured, scenario)?);
    let wire = server.await?.map_err(|error| error.to_string())?;
    assert_eq!(wire.order, captured.strings(scenario, "order")?);
    assert_eq!(
        after_initialize(&wire.lines),
        after_initialize(&captured.strings(scenario, "lines")?)
    );
    Ok(())
}

#[tokio::test]
async fn managed_settings_hold_every_request_behind_initialize_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let scenario = "hold_managed_success";
    let (mut query, server) = start(managed()?, Answer::Success("managed_settings_applied")).await?;
    let handle = query.handle().ok_or("the query has no handle")?;
    let (set_model, interrupt) = tokio::join!(handle.set_model(Some("m".into())), handle.interrupt());
    set_model?;
    interrupt?;
    let (messages, error) = read_to_the_end(&mut query).await?;
    assert!(error.is_none());
    assert_eq!(messages, captured_stream(&captured, scenario)?);
    let wire = server.await?.map_err(|error| error.to_string())?;
    assert_eq!(wire.order, captured.strings(scenario, "order")?);
    assert_eq!(
        after_initialize(&wire.lines),
        after_initialize(&captured.strings(scenario, "lines")?)
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_initialize_writes_no_prompt_and_rejects_what_waited_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let scenario = "hold_hooks_init_error";
    let (mut query, server) = start(hooks(), Answer::Error("boom")).await?;
    let handle = query.handle().ok_or("the query has no handle")?;
    let (interrupt, set_model, initialization) = tokio::join!(
        handle.interrupt(),
        handle.set_model(Some("m".into())),
        handle.initialization_result()
    );
    // The held interrupt and the initialization result reject with the initialize error. The model change was
    // written before the error arrived and was answered.
    assert_eq!(
        text(&interrupt.err().ok_or("the held interrupt must be rejected")?),
        expected(&captured, scenario, "interrupt")?
    );
    assert_eq!(
        text(&initialization.err().ok_or("initializationResult() must reject")?),
        expected(&captured, scenario, "initialization_result")?
    );
    assert_eq!(
        captured.field(scenario, "seen")?["acts"]["set_model"]["settled"],
        "resolved"
    );
    set_model?;
    let (messages, error) = read_to_the_end(&mut query).await?;
    assert_eq!(messages, captured_stream(&captured, scenario)?);
    assert_eq!(
        text(&error.ok_or("the run must end with the initialize error")?),
        captured.field(scenario, "seen")?["stream_error"]
            .as_str()
            .ok_or("no stream error")?
    );
    let wire = server.await?.map_err(|error| error.to_string())?;
    assert_eq!(wire.order, captured.strings(scenario, "order")?);
    assert_eq!(
        after_initialize(&wire.lines),
        after_initialize(&captured.strings(scenario, "lines")?)
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_initialize_with_managed_settings_writes_nothing_after_it_like_sdk_mjs() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    let scenario = "hold_managed_init_error";
    let (mut query, server) = start(managed()?, Answer::Error("boom")).await?;
    let handle = query.handle().ok_or("the query has no handle")?;
    let (set_model, initialization) = tokio::join!(handle.set_model(Some("m".into())), handle.initialization_result());
    assert_eq!(
        text(&set_model.err().ok_or("the held request must be rejected")?),
        expected(&captured, scenario, "set_model")?
    );
    assert_eq!(
        text(&initialization.err().ok_or("initializationResult() must reject")?),
        expected(&captured, scenario, "initialization_result")?
    );
    let (_, error) = read_to_the_end(&mut query).await?;
    assert_eq!(
        text(&error.ok_or("the run must end with the initialize error")?),
        captured.field(scenario, "seen")?["stream_error"]
            .as_str()
            .ok_or("no stream error")?
    );
    let wire = server.await?.map_err(|error| error.to_string())?;
    assert_eq!(wire.order, captured.strings(scenario, "order")?);
    assert_eq!(
        after_initialize(&wire.lines),
        after_initialize(&captured.strings(scenario, "lines")?)
    );
    Ok(())
}

/// `sdk.mjs` gives up on an initialize that never answers after 660 seconds (the capture adds the node startup time), writes nothing held, and rejects
/// the stream and every waiting request with the timer's message. `driver_tests` runs the same timer on a
/// paused clock, so the duration and the messages are checked against what was captured here.
#[test]
fn the_captured_timer_matches_the_duration_and_messages_the_driver_uses() -> TestResult {
    let captured = Captured::parse(CAPTURED)?;
    for (scenario, expected, act) in [
        ("hold_timer_hooks", Error::InitializeNotConfirmed, "interrupt"),
        ("hold_timer_managed", Error::ManagedSettingsNotConfirmed, "set_model"),
    ] {
        let expected = expected.to_string();
        let seen = captured.field(scenario, "seen")?;
        assert_eq!(seen["stream_error"], expected.as_str());
        assert_eq!(seen["acts"][act]["message"], expected.as_str());
        let waited = seen["ended_after_ms"].as_u64().ok_or("no captured duration")?;
        assert!(
            (660_000..665_000).contains(&waited),
            "the captured timer fired after {waited} ms"
        );
        assert_eq!(
            captured.strings(scenario, "order")?,
            ["sdk writes initialize", "stdin closed"]
        );
    }
    Ok(())
}
