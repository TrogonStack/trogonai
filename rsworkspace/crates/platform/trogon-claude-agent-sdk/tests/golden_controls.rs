#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests that pin the request bytes and the response handling of every query control. The expected
//! lines were captured from `sdk.mjs` 0.3.323 and `claude_agent_sdk` 0.2.181 talking to a scripted CLI, with
//! only the random request id normalized.

use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, Backgrounded, ClaudeAgentOptions, ClaudeSdkClient, ContextUsageDetail, ControlOperation, Error,
    InterruptReceipt, JsonObject, LocalPlugin, MaxThinkingTokensDisplay, McpPermissionModeOverride, McpServerName,
    McpServers, MessageId, ReadFileEncoding, ReadFileOptions, ReloadPluginsOptions, RewindMode, SdkMcpServers,
    SettingsUpdateSource, TaskId, ToolUseId,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// How the scripted CLI answers one control request.
enum Reply {
    /// `success` with this `response`.
    Success(Value),
    /// `success` without a `response` key.
    Omitted,
    /// `error` with this message and, unless `null`, this `error_code`.
    Failure(&'static str, Value),
    /// Writes the frame, then answers.
    After(Value, Box<Reply>),
}

impl Reply {
    fn frames(self, request_id: &Value) -> Vec<Value> {
        match self {
            Self::Success(response) => vec![json!({"type":"control_response","response":{
                "subtype":"success","request_id":request_id,"response":response}})],
            Self::Omitted => vec![json!({"type":"control_response","response":{
                "subtype":"success","request_id":request_id}})],
            Self::Failure(message, Value::Null) => vec![json!({"type":"control_response","response":{
                "subtype":"error","request_id":request_id,"error":message}})],
            Self::Failure(message, code) => vec![json!({"type":"control_response","response":{
                "subtype":"error","request_id":request_id,"error":message,"error_code":code}})],
            Self::After(frame, reply) => {
                let mut frames = vec![frame];
                frames.extend(reply.frames(request_id));
                frames
            }
        }
    }
}

type Script = HashMap<&'static str, VecDeque<Reply>>;

/// Answers each control request with the next scripted reply for its subtype (`{}` once they run out) and
/// records every line written to stdin, byte for byte.
async fn serve(process: ScriptedProcessHarness, mut script: Script) -> Result<Vec<String>, std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut written = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if request["type"] == "control_request" {
            let subtype = request["request"]["subtype"].as_str().unwrap_or_default();
            let reply = script
                .get_mut(subtype)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Reply::Success(json!({})));
            for frame in reply.frames(&request["request_id"]) {
                let mut frame = serde_json::to_vec(&frame)?;
                frame.push(b'\n');
                stdout.write_all(&frame).await?;
            }
        }
        written.push(line);
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(written)
}

async fn connect(
    style: ArgvStyle,
    script: Script,
) -> Result<
    (
        ClaudeSdkClient,
        tokio::task::JoinHandle<Result<Vec<String>, std::io::Error>>,
    ),
    Error,
> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve(harness, script));
    let options = ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        argv_style: style,
        ..ClaudeAgentOptions::default()
    };
    let client = ClaudeSdkClient::connect_with_adapters(
        options,
        RuntimeAdapters {
            environment: FixedEnvironment {
                directory: PathBuf::from("unused-directory"),
                variables: BTreeMap::new(),
            },
            process,
            filesystem: NativeFileSystem,
        },
    )
    .await?;
    Ok((client, server))
}

fn script(entries: impl IntoIterator<Item = (&'static str, Vec<Reply>)>) -> Script {
    entries
        .into_iter()
        .map(|(subtype, replies)| (subtype, replies.into()))
        .collect()
}

/// Swaps the random request id in `line` for `<id>` after checking it against `valid`.
fn normalize(line: &str, key: &str, valid: impl Fn(&str) -> bool) -> String {
    let start = line.find(key).map(|index| index + key.len());
    let Some(start) = start else {
        return line.to_owned();
    };
    let end = line[start..].find('"').map_or(line.len(), |index| start + index);
    let id = &line[start..end];
    assert!(valid(id), "unexpected request id {id:?}");
    format!("{}<id>{}", &line[..start], &line[end..])
}

fn typescript_line(line: &str) -> String {
    normalize(line, r#"{"request_id":""#, |id| {
        id.len() <= 13
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
    })
}

fn python_line(line: &str, counter: usize) -> String {
    let prefix = format!("req_{counter}_");
    normalize(line, r#""request_id": ""#, |id| {
        id.strip_prefix(prefix.as_str()).is_some_and(|hex| {
            hex.len() == 8
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    })
}

fn object(value: Value) -> Result<JsonObject, Box<dyn std::error::Error>> {
    match value {
        Value::Object(object) => Ok(object),
        _ => Err("expected an object".into()),
    }
}

fn first_initialize() -> Value {
    json!({"commands":[{"name":"a","description":"d","argumentHint":""}],"models":[],"agents":[],
        "account":{"email":"x"}})
}

/// The requests `sdk.mjs` 0.3.323 wrote for the same calls, in order.
const TYPESCRIPT_REQUESTS: &[&str] = &[
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"initialize","systemPrompt":[""]}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"set_model"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"set_model","model":"opus"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"set_mcp_permission_mode_override","serverName":"srv","mode":"auto"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"set_mcp_permission_mode_override","serverName":"srv","mode":null}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":1024}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":null,"thinking_display":null}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"set_max_thinking_tokens","max_thinking_tokens":2048,"thinking_display":"summarized"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"apply_flag_settings","settings":{"model":"x","effortLevel":null}}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"update_settings","source":"localSettings","settings":{"a":1}}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"get_context_usage"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"get_context_usage","detail":"full"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"get_usage"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"get_usage","skip_behaviors":true}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"get_usage"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"read_file","path":"a.txt"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"read_file","path":"b","max_bytes":10,"encoding":"base64"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"reload_plugins"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"reload_plugins","hold_on_cache_impact":true,"add_plugins":[{"type":"local","path":"/p"}]}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"reload_plugins"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"reload_skills"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"reload_output_styles"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"seed_read_state","path":"f","mtime":1700000000000}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"seed_read_state","path":"f","mtime":1.5}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"mcp_read_resource","serverName":"srv","uri":"file://x"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"rewind_files","user_message_id":"u1"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"rewind_files","user_message_id":"u1","dry_run":true}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"rewind_files","user_message_id":"u1"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"background_tasks"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"background_tasks","tool_use_id":"tu1"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"mcp_status"}}"#,
    r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"initialize","systemPrompt":[""]}}"#,
];

#[tokio::test]
async fn typescript_controls_match_sdk_mjs_bytes_and_returns() -> TestResult {
    let (client, server) = connect(
        ArgvStyle::TypeScript,
        script([
            ("initialize", vec![Reply::Success(first_initialize())]),
            ("set_mcp_permission_mode_override", vec![Reply::Omitted]),
            (
                "read_file",
                vec![
                    Reply::Failure("missing", Value::Null),
                    Reply::Success(json!({"contents":"hi","x":1})),
                ],
            ),
            (
                "mcp_status",
                vec![Reply::Success(
                    json!({"mcpServers":[{"name":"s","status":"connected"}],"extra":1}),
                )],
            ),
        ]),
    )
    .await?;
    let handle = client.handle();
    handle.set_model(None).await?;
    handle.set_model(Some("opus".into())).await?;
    let override_reply = handle
        .set_mcp_permission_mode_override("srv", Some(McpPermissionModeOverride::Auto))
        .await?;
    assert_eq!(override_reply.as_json(), Some(&json!({})));
    handle.set_mcp_permission_mode_override("srv", None).await?;
    handle.set_max_thinking_tokens(Some(1024)).await?;
    handle.set_max_thinking_tokens_with_display(None, None).await?;
    handle
        .set_max_thinking_tokens_with_display(Some(2048), Some(MaxThinkingTokensDisplay::Summarized))
        .await?;
    handle
        .apply_flag_settings(&object(json!({"model":"x","effortLevel":null}))?)
        .await?;
    handle
        .update_settings(SettingsUpdateSource::LocalSettings, &object(json!({"a":1}))?)
        .await?;
    handle.context_usage(None).await?;
    handle.context_usage(Some(ContextUsageDetail::Full)).await?;
    handle.usage_experimental(false).await?;
    handle.usage_experimental(true).await?;
    handle.usage_experimental(false).await?;
    assert!(handle.read_file("a.txt", ReadFileOptions::default()).await.is_none());
    let file = handle
        .read_file(
            "b",
            ReadFileOptions {
                max_bytes: Some(10),
                encoding: Some(ReadFileEncoding::Base64),
            },
        )
        .await
        .ok_or("read_file failed")?;
    assert_eq!(file.as_json(), Some(&json!({"contents":"hi","x":1})));
    handle.reload_plugins(&ReloadPluginsOptions::default()).await?;
    handle
        .reload_plugins(&ReloadPluginsOptions {
            hold_on_cache_impact: true,
            add_plugins: vec![LocalPlugin::new("/p")],
        })
        .await?;
    handle
        .reload_plugins(&ReloadPluginsOptions {
            hold_on_cache_impact: false,
            add_plugins: Vec::new(),
        })
        .await?;
    handle.reload_skills().await?;
    handle.reload_output_styles().await?;
    handle.seed_read_state("f", 1_700_000_000_000.0).await?;
    handle.seed_read_state("f", 1.5).await?;
    handle.read_mcp_resource(McpServerName::from("srv"), "file://x").await?;
    handle.rewind_files(MessageId::from("u1"), None).await?;
    handle.rewind_files(MessageId::from("u1"), RewindMode::DryRun).await?;
    handle.rewind_files(MessageId::from("u1"), None).await?;
    assert_eq!(handle.background_tasks(None).await?, Backgrounded::Bool(true));
    handle.background_tasks(Some(ToolUseId::from("tu1"))).await?;
    let servers = handle.mcp_server_status().await?;
    assert_eq!(servers.as_json(), Some(&json!([{"name":"s","status":"connected"}])));
    assert_eq!(servers.decode()?[0].name, "s");
    let commands = handle.supported_commands()?;
    assert_eq!(
        commands.as_json(),
        Some(&json!([{"name":"a","description":"d","argumentHint":""}]))
    );
    assert_eq!(handle.account_info()?.as_json(), Some(&json!({"email":"x"})));
    assert_eq!(handle.supported_models()?.as_json(), Some(&json!([])));
    assert_eq!(handle.supported_agents()?.as_json(), Some(&json!([])));
    let reinitialized = handle.reinitialize().await?;
    assert_eq!(reinitialized.as_json(), Some(&json!({})));
    assert_eq!(handle.initialization_result()?.as_json(), Some(&first_initialize()));
    client.disconnect().await?;
    let lines: Vec<String> = server.await??.iter().map(|line| typescript_line(line)).collect();
    assert_eq!(lines, TYPESCRIPT_REQUESTS);
    Ok(())
}

#[tokio::test]
async fn typescript_rejections_keep_a_well_formed_error_code() -> TestResult {
    let (client, server) = connect(
        ArgvStyle::TypeScript,
        script([
            ("stop_task", vec![Reply::Failure("nope", json!("bad_thing"))]),
            ("set_model", vec![Reply::Failure("no model", json!("BAD"))]),
            ("background_tasks", vec![Reply::Success(json!({"backgrounded":false}))]),
        ]),
    )
    .await?;
    let handle = client.handle();
    let Err(Error::ControlRejected { operation, rejection }) = handle.stop_task(TaskId::from("t")).await else {
        return Err("stop_task must be rejected".into());
    };
    assert_eq!(operation, ControlOperation::StopTask);
    assert_eq!(rejection.message(), "nope");
    assert_eq!(rejection.error_code(), Some("bad_thing"));
    let Err(Error::ControlRejected { rejection, .. }) = handle.set_model(Some("m".into())).await else {
        return Err("set_model must be rejected".into());
    };
    assert_eq!(rejection.message(), "no model");
    assert_eq!(rejection.error_code(), None);
    assert!(!handle.background_tasks(None).await?.into_bool()?);
    client.disconnect().await?;
    server.await??;
    Ok(())
}

const BACKGROUND_TASKS_GOLDENS: &str = include_str!("fixtures/background_tasks/goldens.json");

/// Each reply `sdk.mjs` was given and what `backgroundTasks` returned or threw for it: `backgrounded` of any
/// type comes back as sent, and only a reply without a `response` object to read it from fails.
#[tokio::test]
async fn background_tasks_reads_backgrounded_like_typescript() -> TestResult {
    let goldens: Value = serde_json::from_str(BACKGROUND_TASKS_GOLDENS)?;
    let cases = goldens["cases"].as_array().ok_or("cases")?;
    let replies: Vec<Reply> = cases
        .iter()
        .map(|case| match case["reply"].get("response") {
            Some(response) => Reply::Success(response.clone()),
            None => Reply::Omitted,
        })
        .collect();
    let (client, server) = connect(ArgvStyle::TypeScript, script([("background_tasks", replies)])).await?;
    let handle = client.handle();
    for case in cases {
        let name = case["name"].as_str().ok_or("name")?;
        let outcome = handle.background_tasks(Some(ToolUseId::from("toolu_1"))).await;
        match (case["typescript"].get("value"), outcome) {
            (None, Err(Error::Protocol(_))) => {}
            (Some(Value::Bool(expected)), Ok(Backgrounded::Bool(backgrounded))) => {
                assert_eq!(backgrounded, *expected, "{name}");
            }
            (Some(Value::Bool(_)), outcome) => return Err(format!("{name}: {outcome:?}").into()),
            (Some(expected), Ok(backgrounded)) => {
                assert_eq!(backgrounded, Backgrounded::Other(expected.clone()), "{name}");
                assert_eq!(backgrounded.as_bool(), None, "{name}");
                let Err(error) = backgrounded.into_bool() else {
                    return Err(format!("{name}: the opt-in check accepted a non-boolean").into());
                };
                assert_eq!(error.value(), expected, "{name}");
            }
            (_, outcome) => return Err(format!("{name}: {outcome:?}").into()),
        }
    }
    client.disconnect().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn responses_without_an_object_report_nothing_or_fail_like_typescript() -> TestResult {
    let (client, server) = connect(
        ArgvStyle::TypeScript,
        script([
            ("interrupt", vec![Reply::Omitted]),
            ("mcp_status", vec![Reply::Omitted]),
        ]),
    )
    .await?;
    let handle = client.handle();
    assert!(matches!(
        handle.interrupt().await?,
        InterruptReceipt::NotReported { additional_fields } if additional_fields.is_empty()
    ));
    assert!(matches!(handle.mcp_server_status().await, Err(Error::Protocol(_))));
    client.disconnect().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn an_absent_response_stays_undefined_and_a_null_one_stays_null_like_typescript() -> TestResult {
    let (client, server) = connect(
        ArgvStyle::TypeScript,
        script([
            ("initialize", vec![Reply::Omitted]),
            ("reload_skills", vec![Reply::Omitted, Reply::Success(Value::Null)]),
            ("get_context_usage", vec![Reply::Omitted]),
        ]),
    )
    .await?;
    let handle = client.handle();
    assert_eq!(client.connection_info().as_json(), None);
    assert_eq!(handle.initialization_result()?.as_json(), None);
    assert!(matches!(handle.supported_models(), Err(Error::Protocol(_))));
    assert!(matches!(handle.supported_commands(), Err(Error::Protocol(_))));
    assert_eq!(handle.reload_skills().await?.as_json(), None);
    assert_eq!(handle.reload_skills().await?.as_json(), Some(&Value::Null));
    assert_eq!(handle.context_usage(None).await?.as_json(), None);
    client.disconnect().await?;
    server.await??;
    Ok(())
}

/// `setMcpServers` returns the reply's `response` field as sent, and `claude_agent_sdk` control requests read
/// anything but an object as `{}`. Recorded from both SDKs answering five `mcp_set_servers` requests with no
/// `response`, `null`, a full result, a string and a malformed result.
#[tokio::test]
async fn mcp_replacement_replies_are_kept_like_each_sdk() -> TestResult {
    let result = json!({"added":["a"],"removed":[],"errors":{},"extra":1});
    let malformed = json!({"added":"x"});
    let replies = || {
        vec![
            Reply::Omitted,
            Reply::Success(Value::Null),
            Reply::Success(result.clone()),
            Reply::Success(json!("text")),
            Reply::Success(malformed.clone()),
        ]
    };
    for (style, expected) in [
        (
            ArgvStyle::TypeScript,
            [
                None,
                Some(Value::Null),
                Some(result.clone()),
                Some(json!("text")),
                Some(malformed.clone()),
            ],
        ),
        (
            ArgvStyle::Python,
            [
                Some(json!({})),
                Some(json!({})),
                Some(result.clone()),
                Some(json!({})),
                Some(malformed.clone()),
            ],
        ),
    ] {
        let (client, server) = connect(style, script([("mcp_set_servers", replies())])).await?;
        let handle = client.handle();
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(handle.set_mcp_servers(McpServers::new(), SdkMcpServers::new()).await?);
        }
        let decoded = seen[2].decode()?;
        assert_eq!(decoded.added, ["a"]);
        assert_eq!(decoded.additional_fields.get("extra"), Some(&json!(1)));
        assert!(seen[4].decode().is_err());
        let seen: Vec<Option<Value>> = seen
            .into_iter()
            .map(trogon_claude_agent_sdk::ControlReply::into_json)
            .collect();
        assert_eq!(seen, expected);
        client.disconnect().await?;
        server.await??;
    }
    Ok(())
}

#[tokio::test]
async fn supported_commands_follow_commands_changed() -> TestResult {
    let changed = json!({"type":"system","subtype":"commands_changed",
        "commands":[{"name":"b","description":"e","argumentHint":"<x>"}]});
    let ignored = json!({"type":"system","subtype":"commands_changed","commands":"none"});
    let (client, server) = connect(
        ArgvStyle::TypeScript,
        script([
            ("initialize", vec![Reply::Success(first_initialize())]),
            (
                "reload_skills",
                vec![
                    Reply::After(changed, Box::new(Reply::Success(json!({})))),
                    Reply::After(ignored, Box::new(Reply::Success(json!({})))),
                ],
            ),
        ]),
    )
    .await?;
    let handle = client.handle();
    handle.reload_skills().await?;
    let expected = json!([{"name":"b","description":"e","argumentHint":"<x>"}]);
    assert_eq!(handle.supported_commands()?.as_json(), Some(&expected));
    handle.reload_skills().await?;
    assert_eq!(handle.supported_commands()?.as_json(), Some(&expected));
    client.disconnect().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn javascript_numbers_are_spelled_like_json_stringify() -> TestResult {
    let (client, server) = connect(ArgvStyle::TypeScript, Script::new()).await?;
    let handle = client.handle();
    handle.seed_read_state("f", 1e21).await?;
    handle.seed_read_state("f", f64::NAN).await?;
    handle.seed_read_state("f", -0.0).await?;
    client.disconnect().await?;
    let lines: Vec<String> = server.await??.iter().map(|line| typescript_line(line)).collect();
    assert_eq!(
        &lines[1..],
        [
            r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"seed_read_state","path":"f","mtime":1e+21}}"#,
            r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"seed_read_state","path":"f","mtime":null}}"#,
            r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"seed_read_state","path":"f","mtime":0}}"#,
        ]
    );
    Ok(())
}

#[tokio::test]
async fn python_controls_match_claude_agent_sdk_bytes_and_returns() -> TestResult {
    let (client, server) = connect(
        ArgvStyle::Python,
        script([
            ("rewind_files", vec![Reply::Success(json!({"canRewind":true}))]),
            ("mcp_status", vec![Reply::Omitted]),
            ("get_context_usage", vec![Reply::Success(Value::Null)]),
            ("stop_task", vec![Reply::Failure("nope", json!("bad_thing"))]),
        ]),
    )
    .await?;
    let handle = client.handle();
    handle.set_model(None).await?;
    handle.set_model(Some("opus".into())).await?;
    handle.rewind_files(MessageId::from("u1"), None).await?;
    assert_eq!(handle.mcp_status().await?.as_json(), Some(&json!({})));
    assert_eq!(handle.context_usage(None).await?.as_json(), Some(&json!({})));
    let Err(Error::ControlRejected { rejection, .. }) = handle.stop_task(TaskId::from("t")).await else {
        return Err("stop_task must be rejected".into());
    };
    assert_eq!(rejection.message(), "nope");
    assert_eq!(rejection.error_code(), None);
    client.disconnect().await?;
    let lines: Vec<String> = server
        .await??
        .iter()
        .enumerate()
        .map(|(index, line)| python_line(line, index + 1))
        .collect();
    assert_eq!(
        lines,
        [
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "initialize", "hooks": null}}"#,
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "set_model", "model": null}}"#,
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "set_model", "model": "opus"}}"#,
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "rewind_files", "user_message_id": "u1"}}"#,
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "mcp_status"}}"#,
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "get_context_usage"}}"#,
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "stop_task", "task_id": "t"}}"#,
        ]
    );
    Ok(())
}
