#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "The goldens are the JSON the pinned SDKs produced, compared as JSON."
    )
)]

//! Replays every corpus message through the client in both decoding modes and compares the outcome with the
//! goldens the pinned TypeScript `query()` and Python `query()` produced for the same message
//! (`mise run claude-sdk:message-goldens` regenerates them).

use serde_json::{Value, json};
use std::{collections::BTreeMap, io, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines},
    time::timeout,
};
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, Content, ContentBlock, Error, JsonObject, Message, MessageDecoding,
    SystemEvent, Timeout, ToolResultStatus,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessObserver, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CASES: &str = include_str!("fixtures/message_decoding/cases.json");
const EXPECTED: &str = include_str!("fixtures/message_decoding/expected.json");

/// Python accepts these values where the Rust field type cannot hold them, so the client keeps the message (or
/// the block) as `Unknown` with the value intact instead of the typed struct.
const UNTYPED_IN_RUST: [&str; 2] = ["assistant_tool_use_null_input", "user_content_object"];

struct ScriptedCli {
    lines: Lines<BufReader<DuplexStream>>,
    stdout: DuplexStream,
    stderr: DuplexStream,
    observer: ScriptedProcessObserver,
}

impl From<ScriptedProcessHarness> for ScriptedCli {
    fn from(harness: ScriptedProcessHarness) -> Self {
        Self {
            lines: BufReader::new(harness.stdin).lines(),
            stdout: harness.stdout,
            stderr: harness.stderr,
            observer: harness.observer,
        }
    }
}

impl ScriptedCli {
    async fn read(&mut self) -> Result<Value, Box<dyn std::error::Error>> {
        let line = timeout(Duration::from_secs(2), self.lines.next_line())
            .await??
            .ok_or_else(|| io::Error::other("expected a CLI frame"))?;
        Ok(serde_json::from_str(&line)?)
    }

    async fn send(&mut self, frame: &Value) -> TestResult {
        let mut bytes = serde_json::to_vec(frame)?;
        bytes.push(b'\n');
        self.stdout.write_all(&bytes).await?;
        Ok(())
    }

    fn end(self) {
        self.observer.complete(0);
        drop(self.stdout);
        drop(self.stderr);
    }
}

/// What one run yielded: the messages and the error that ended it.
struct Outcome {
    messages: Vec<Message>,
    error: Option<Error>,
}

async fn run(message: &Value, decoding: MessageDecoding) -> Result<Outcome, Box<dyn std::error::Error>> {
    let mut options = ClaudeAgentOptions {
        message_decoding: decoding,
        ..ClaudeAgentOptions::default()
    };
    options.limits.control_timeout = Some(Timeout::checked(Duration::from_secs(2))?);
    options.limits.shutdown = trogon_claude_agent_sdk::Shutdown::Strict(Timeout::checked(Duration::from_secs(5))?);
    let (process, harness) = ScriptedProcessSpawner::new();
    let mut cli = ScriptedCli::from(harness);
    let adapters = RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-project"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    };
    let connecting = tokio::spawn(ClaudeSdkClient::connect_with_adapters(options, adapters));
    let request = cli.read().await?;
    cli.send(&json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{}}}))
        .await?;
    let mut client = connecting.await??;
    cli.send(message).await?;
    cli.end();
    let mut messages = Vec::new();
    let error = timeout(Duration::from_secs(5), async {
        loop {
            match client.next_message().await {
                Ok(Some(message)) => messages.push(message),
                Ok(None) => return None,
                Err(error) => return Some(error),
            }
        }
    })
    .await?;
    Ok(Outcome { messages, error })
}

fn cases() -> Result<Vec<(String, Value)>, Box<dyn std::error::Error>> {
    let cases: Value = serde_json::from_str(CASES)?;
    let cases = cases["cases"]
        .as_array()
        .ok_or_else(|| io::Error::other("cases is not a list"))?;
    Ok(cases
        .iter()
        .map(|case| {
            (
                case["name"].as_str().unwrap_or_default().to_owned(),
                case["message"].clone(),
            )
        })
        .collect())
}

fn expected(name: &str, side: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let goldens: Value = serde_json::from_str(EXPECTED)?;
    let golden = goldens[name][side].clone();
    if golden.is_null() {
        return Err(io::Error::other(format!("no {side} golden for {name}")).into());
    }
    Ok(golden)
}

#[tokio::test]
async fn typescript_decoding_yields_what_the_pinned_typescript_query_yields() -> TestResult {
    for (name, message) in cases()? {
        let golden = expected(&name, "typescript")?;
        let outcome = run(&message, MessageDecoding::TypeScript).await?;
        let yielded: Vec<Value> = outcome
            .messages
            .iter()
            .map(|message| Value::Object(message.raw().clone()))
            .collect();
        assert_eq!(Value::Array(yielded), golden["yielded"], "{name}");
        assert!(outcome.error.is_none(), "{name}: {:?}", outcome.error);
        assert!(golden["error"].is_null(), "{name}");
        for message in &outcome.messages {
            let encoded = serde_json::to_value(message.to_sdk_message())?;
            assert_eq!(encoded, Value::Object(message.raw().clone()), "{name}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn python_decoding_yields_what_the_pinned_python_query_yields() -> TestResult {
    for (name, message) in cases()? {
        let golden = expected(&name, "python")?;
        let outcome = run(&message, MessageDecoding::Python).await?;
        match (&outcome.error, golden["error"].as_object()) {
            (None, None) => {}
            (Some(Error::MessageParse(error)), Some(expected)) => {
                assert_eq!(expected["class"], "MessageParseError", "{name}");
                assert_eq!(Value::from(error.message()), expected["message"], "{name}");
                assert_eq!(error.data(), message.as_object(), "{name}");
            }
            (error, expected) => panic!("{name}: error {error:?}, expected {expected:?}"),
        }
        let expected_messages = golden["yielded"]
            .as_array()
            .ok_or_else(|| io::Error::other("yielded is not a list"))?;
        assert_eq!(outcome.messages.len(), expected_messages.len(), "{name}");
        for (message, expected) in outcome.messages.iter().zip(expected_messages) {
            assert_eq!(
                Value::Object(message.raw().clone()),
                Value::Object(raw_of(&name)?),
                "{name}"
            );
            match python_view(message) {
                Some(view) => {
                    assert!(!UNTYPED_IN_RUST.contains(&name.as_str()), "{name} is typed now");
                    assert_eq!(numbers_as_floats(view), numbers_as_floats(expected.clone()), "{name}");
                }
                None => assert!(UNTYPED_IN_RUST.contains(&name.as_str()), "{name} is not typed"),
            }
        }
    }
    Ok(())
}

fn raw_of(name: &str) -> Result<JsonObject, Box<dyn std::error::Error>> {
    let (_, message) = cases()?
        .into_iter()
        .find(|(case, _)| case == name)
        .ok_or_else(|| io::Error::other("unknown case"))?;
    message
        .as_object()
        .cloned()
        .ok_or_else(|| io::Error::other("case message is not an object").into())
}

/// Python compares `0` and `0.0` as equal; JSON values do not.
fn numbers_as_floats(value: Value) -> Value {
    match value {
        Value::Number(number) => number.as_f64().map_or(Value::Number(number), |float| json!(float)),
        Value::Array(values) => Value::Array(values.into_iter().map(numbers_as_floats).collect()),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, numbers_as_floats(value)))
                .collect(),
        ),
        other => other,
    }
}

/// The message as `dataclasses.asdict` renders the Python class it decodes to, built from the typed Rust fields
/// only. `None` when the message or one of its blocks is `Unknown`.
fn python_view(message: &Message) -> Option<Value> {
    let (class, fields) = match message {
        Message::Assistant(message) => (
            "AssistantMessage",
            json!({
                "content": blocks(&message.content)?,
                "model": message.model,
                "parent_tool_use_id": message.parent_tool_use_id.as_ref().map(|id| id.as_str()),
                "error": message.error,
                "usage": message.usage,
                "message_id": message.message_id.as_ref().map(|id| id.as_str()),
                "stop_reason": message.stop_reason,
                "session_id": message.session_id.as_ref().map(|id| id.as_str()),
                "uuid": message.uuid.as_ref().map(|id| id.as_str()),
            }),
        ),
        Message::User(message) => (
            "UserMessage",
            json!({
                "content": match &message.content {
                    Content::Text(text) => Value::from(text.as_str()),
                    Content::Blocks(content) => blocks(content)?,
                },
                "uuid": message.uuid.as_ref().map(|id| id.as_str()),
                "parent_tool_use_id": message.parent_tool_use_id.as_ref().map(|id| id.as_str()),
                "tool_use_result": message.tool_use_result,
                "origin": message.origin.as_ref().map(serde_json::to_value).transpose().ok()?,
            }),
        ),
        Message::System(message) => {
            let base = json!({"subtype": message.subtype, "data": message.raw});
            let (class, extra) = match &message.event {
                SystemEvent::Other => ("SystemMessage", json!({})),
                SystemEvent::TaskStarted(event) => (
                    "TaskStartedMessage",
                    json!({"task_id": event.task_id, "description": event.description, "uuid": event.uuid,
                        "session_id": event.session_id, "tool_use_id": event.tool_use_id, "task_type": event.task_type}),
                ),
                SystemEvent::TaskProgress(event) => (
                    "TaskProgressMessage",
                    json!({"task_id": event.task_id, "description": event.description, "usage": event.usage,
                        "uuid": event.uuid, "session_id": event.session_id, "tool_use_id": event.tool_use_id,
                        "last_tool_name": event.last_tool_name}),
                ),
                SystemEvent::TaskNotification(event) => (
                    "TaskNotificationMessage",
                    json!({"task_id": event.task_id, "status": event.status, "output_file": event.output_file,
                        "summary": event.summary, "uuid": event.uuid, "session_id": event.session_id,
                        "tool_use_id": event.tool_use_id, "usage": event.usage}),
                ),
                SystemEvent::TaskUpdated(event) => (
                    "TaskUpdatedMessage",
                    json!({"task_id": event.task_id, "patch": event.patch, "status": event.status,
                        "session_id": event.session_id, "uuid": event.uuid}),
                ),
                SystemEvent::MirrorError(event) => {
                    ("MirrorErrorMessage", json!({"key": event.key, "error": event.error}))
                }
                SystemEvent::Hook(event) => (
                    "HookEventMessage",
                    json!({"hook_event_name": event.hook_event_name, "session_id": event.session_id,
                        "uuid": event.uuid}),
                ),
            };
            let mut fields = base.as_object()?.clone();
            fields.extend(extra.as_object()?.clone());
            (class, Value::Object(fields))
        }
        Message::Result(message) => (
            "ResultMessage",
            json!({
                "subtype": message.subtype,
                "duration_ms": message.duration_ms,
                "duration_api_ms": message.duration_api_ms,
                "is_error": matches!(message.status, trogon_claude_agent_sdk::ResultStatus::Error),
                "num_turns": message.num_turns,
                "session_id": message.session_id.as_str(),
                "stop_reason": message.stop_reason,
                "total_cost_usd": message.total_cost_usd,
                "usage": message.usage,
                "result": message.result,
                "structured_output": message.structured_output,
                "model_usage": message.model_usage,
                "permission_denials": message.permission_denials,
                "deferred_tool_use": message.deferred_tool().map(|tool| json!({"id": tool.id, "name": tool.name, "input": tool.input})),
                "errors": message.errors,
                "api_error_status": message.api_error_status,
                "uuid": message.uuid.as_ref().map(|id| id.as_str()),
                "terminal_reason": message.terminal_reason,
                "origin": message.origin.as_ref().map(serde_json::to_value).transpose().ok()?,
            }),
        ),
        Message::StreamEvent(message) => (
            "StreamEvent",
            json!({
                "uuid": message.uuid,
                "session_id": message.session_id.as_str(),
                "event": message.event,
                "parent_tool_use_id": message.parent_tool_use_id.as_ref().map(|id| id.as_str()),
            }),
        ),
        Message::RateLimit(message) => {
            let info = &message.rate_limit_info;
            (
                "RateLimitEvent",
                json!({
                    "rate_limit_info": {
                        "status": info.status,
                        "resets_at": info.resets_at,
                        "rate_limit_type": info.rate_limit_type,
                        "utilization": info.utilization,
                        "overage_status": info.overage_status,
                        "overage_resets_at": info.overage_resets_at,
                        "overage_disabled_reason": info.overage_disabled_reason,
                        "raw": info.raw,
                    },
                    "uuid": message.uuid,
                    "session_id": message.session_id.as_str(),
                }),
            )
        }
        Message::ConversationReset(message) => (
            "ConversationResetMessage",
            json!({
                "new_conversation_id": message.new_conversation_id,
                "uuid": message.uuid,
                "session_id": message.session_id.as_str(),
            }),
        ),
        Message::Unknown { .. } => return None,
    };
    Some(json!({"class": class, "fields": fields}))
}

fn blocks(content: &[ContentBlock]) -> Option<Value> {
    content
        .iter()
        .map(|block| {
            Some(match block {
                ContentBlock::Text { text } => json!({"text": text}),
                ContentBlock::Thinking { thinking, signature } => json!({"thinking": thinking, "signature": signature}),
                ContentBlock::ToolUse { id, name, input } => {
                    json!({"id": id.as_str(), "name": name.as_str(), "input": input})
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    status,
                } => json!({
                    "tool_use_id": tool_use_id.as_str(),
                    "content": content,
                    "is_error": match status {
                        ToolResultStatus::Error => Some(true),
                        ToolResultStatus::Success => Some(false),
                        ToolResultStatus::NotReported => None,
                    },
                }),
                ContentBlock::ServerToolUse { id, name, input } => {
                    json!({"id": id.as_str(), "name": name, "input": input})
                }
                ContentBlock::ServerToolResult { tool_use_id, content } => {
                    json!({"tool_use_id": tool_use_id.as_str(), "content": content})
                }
                ContentBlock::Unknown { .. } => return None,
            })
        })
        .collect::<Option<Vec<_>>>()
        .map(Value::Array)
}
