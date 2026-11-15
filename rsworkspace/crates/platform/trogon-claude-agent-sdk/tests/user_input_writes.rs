#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests for the user message lines each SDK writes to the CLI, compared byte for byte, key order included.
//! `tests/fixtures/user_input.json` holds the lines `claude_agent_sdk` 0.2.181 and `sdk.mjs` 0.3.323 wrote to a
//! scripted CLI: a string prompt to `query()`, the messages under `messages` streamed as given, and Python
//! `ClaudeSDKClient.query` filling the session of a streamed message that names none.

use std::{collections::BTreeMap, path::PathBuf};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, Error, InputBlock, JsonObject, MessageId, MessageOrigin, Prompt,
    SessionId, ToolUseId, UserInput, UserInputPriority, VerbatimPrompts,
    adapters::{NativeFileSystem, RuntimeAdapters},
    query_with_adapters,
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Server = tokio::task::JoinHandle<Result<Vec<String>, std::io::Error>>;

const GOLDENS: &str = include_str!("fixtures/user_input.json");

const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"result":"ok","session_id":"s","total_cost_usd":0,"usage":{},"modelUsage":{},"permission_denials":[],"uuid":"00000000-0000-4000-8000-000000000002"}"#;

/// Answers each control request with `{}` and each user line with a result, and returns the user lines exactly as
/// they were written.
async fn serve(process: ScriptedProcessHarness) -> Result<Vec<String>, std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut written = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let frame: Value = serde_json::from_str(&line)?;
        let mut reply = match frame["type"].as_str() {
            Some("control_request") => serde_json::to_string(&json!({"type":"control_response","response":{
                "subtype":"success","request_id":frame["request_id"],"response":{}}}))?,
            _ => {
                written.push(line);
                RESULT.to_owned()
            }
        };
        reply.push('\n');
        stdout.write_all(reply.as_bytes()).await?;
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(written)
}

fn adapters(process: ScriptedProcessSpawner) -> RuntimeAdapters<FixedEnvironment, ScriptedProcessSpawner> {
    RuntimeAdapters {
        environment: FixedEnvironment {
            directory: PathBuf::from("unused-directory"),
            variables: BTreeMap::new(),
        },
        process,
        filesystem: NativeFileSystem,
    }
}

fn options(style: ArgvStyle, verbatim: bool) -> ClaudeAgentOptions {
    let mut options = ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        argv_style: style,
        ..ClaudeAgentOptions::default()
    };
    if verbatim {
        options.verbatim_prompts = VerbatimPrompts::Enabled;
    }
    options
}

fn dialect(style: ArgvStyle) -> &'static str {
    match style {
        ArgvStyle::TypeScript => "typescript",
        ArgvStyle::Python => "python",
    }
}

fn goldens() -> Result<Value, serde_json::Error> {
    serde_json::from_str(GOLDENS)
}

fn golden(style: ArgvStyle, case: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let lines = goldens()?[dialect(style)][case]
        .as_array()
        .ok_or_else(|| format!("missing golden {case}"))?
        .iter()
        .map(|line| line.as_str().map(str::to_owned).ok_or("golden line is not a string"))
        .collect::<Result<_, _>>()?;
    Ok(lines)
}

fn message(case: &str) -> Result<JsonObject, Box<dyn std::error::Error>> {
    let Value::Object(object) = goldens()?["messages"][case].clone() else {
        return Err(format!("missing message {case}").into());
    };
    Ok(object)
}

fn object(value: Value) -> Result<JsonObject, Box<dyn std::error::Error>> {
    let Value::Object(object) = value else {
        return Err("not an object".into());
    };
    Ok(object)
}

/// The `full` message built from the typed fields rather than written as given.
fn typed_full() -> Result<UserInput, Box<dyn std::error::Error>> {
    let mut input = UserInput::blocks(vec![InputBlock::text("hi")]);
    input.parent_tool_use_id = Some(ToolUseId::from("toolu_1"));
    input.is_synthetic = Some(true);
    input.tool_use_result = Some(json!({"ok": 1}));
    input.priority = Some(UserInputPriority::Next);
    input.origin = Some(MessageOrigin::new("human", JsonObject::new()));
    input.should_query = Some(false);
    input.timestamp = Some("2026-01-01T00:00:00.000Z".into());
    input.uuid = Some(MessageId::from("00000000-0000-4000-8000-000000000003"));
    input.pasted_content = Some(vec![json!("p"), json!([{"type": "text", "text": "b"}])]);
    input.inline_pastes = Some(vec!["q".into()]);
    input.session_id = Some(SessionId::from("s1"));
    input.agent_id = Some("a1".into());
    input.subagent_type = Some("general-purpose".into());
    input.task_description = Some("d".into());
    input.additional_fields = object(json!({"future_field": {"nested": [1, 2]}}))?;
    Ok(input)
}

/// `stream_input` of `inputs` on a connected client, returning the user lines the CLI saw.
async fn streamed(
    style: ArgvStyle,
    verbatim: bool,
    inputs: Vec<UserInput>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server: Server = tokio::spawn(serve(harness));
    let client = ClaudeSdkClient::connect_with_adapters(options(style, verbatim), adapters(process)).await?;
    client.stream_input(futures_util::stream::iter(inputs)).await?;
    client.disconnect().await?;
    Ok(server.await??)
}

/// Runs `body` on a connected client, returning the user lines the CLI saw.
async fn on_client(
    style: ArgvStyle,
    verbatim: bool,
    body: impl AsyncFnOnce(&ClaudeSdkClient) -> Result<(), Error>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server: Server = tokio::spawn(serve(harness));
    let client = ClaudeSdkClient::connect_with_adapters(options(style, verbatim), adapters(process)).await?;
    body(&client).await?;
    client.disconnect().await?;
    Ok(server.await??)
}

/// `query({prompt: "héllo"})` / `query(prompt="héllo")`: `sdk.mjs` writes the text as one text block and Python as
/// a string, both with `session_id` ahead of `message`.
#[tokio::test]
async fn a_string_prompt_is_written_as_each_sdk_writes_it() -> TestResult {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        for (verbatim, case) in [(false, "string_prompt"), (true, "string_prompt_verbatim")] {
            let (process, harness) = ScriptedProcessSpawner::new();
            let server: Server = tokio::spawn(serve(harness));
            let mut query =
                query_with_adapters(Prompt::from("héllo"), options(style, verbatim), adapters(process)).await?;
            while query.next_message().await?.is_some() {}
            query.disconnect().await?;
            assert_eq!(server.await??, golden(style, case)?, "{style:?} {case}");
        }
    }
    Ok(())
}

/// Both SDKs write a streamed message as given: key order, unknown keys and every `SDKUserMessage` field survive,
/// and `verbatimPrompts` / `verbatim_prompts` sets `client_composed` in place or adds it last.
#[tokio::test]
async fn a_streamed_message_is_written_as_given() -> TestResult {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        for (verbatim, suffix) in [(false, ""), (true, "_verbatim")] {
            let full = format!("stream_full{suffix}");
            let lines = streamed(style, verbatim, vec![UserInput::from_json(message("full")?)]).await?;
            assert_eq!(lines, golden(style, &full)?, "{style:?} {full} as given");
            let lines = streamed(style, verbatim, vec![typed_full()?]).await?;
            assert_eq!(lines, golden(style, &full)?, "{style:?} {full} typed");
            let reordered = format!("stream_reordered{suffix}");
            let lines = streamed(style, verbatim, vec![UserInput::from_json(message("reordered")?)]).await?;
            assert_eq!(lines, golden(style, &reordered)?, "{style:?} {reordered}");
        }
        let lines = streamed(style, false, vec![UserInput::from(Prompt::from("plain"))]).await?;
        assert_eq!(lines, golden(style, "stream_min")?, "{style:?} stream_min");
    }
    Ok(())
}

/// `query({prompt: messages})` / `query(prompt=messages)`: a one-shot query writes a message that is not a string
/// prompt as given, with no session added.
#[tokio::test]
async fn a_one_shot_message_prompt_is_written_as_given() -> TestResult {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        for (verbatim, suffix) in [(false, ""), (true, "_verbatim")] {
            let full = format!("stream_full{suffix}");
            for (input, how) in [
                (UserInput::from_json(message("full")?), "as given"),
                (typed_full()?, "typed"),
            ] {
                let (process, harness) = ScriptedProcessSpawner::new();
                let server: Server = tokio::spawn(serve(harness));
                let mut query = query_with_adapters(input, options(style, verbatim), adapters(process)).await?;
                while query.next_message().await?.is_some() {}
                query.disconnect().await?;
                assert_eq!(server.await??, golden(style, &full)?, "{style:?} {full} {how}");
            }
        }
    }
    Ok(())
}

/// Python `ClaudeSDKClient.query(str, session_id="default")`.
#[tokio::test]
async fn python_client_query_writes_the_session_last() -> TestResult {
    let style = ArgvStyle::Python;
    let lines = on_client(style, false, async |client| client.query(Prompt::from("héllo")).await).await?;
    assert_eq!(lines, golden(style, "client_query")?);
    let lines = on_client(style, false, async |client| {
        client.query_in_session(Prompt::from("x"), "s").await
    })
    .await?;
    assert_eq!(lines, golden(style, "client_query_in_session")?);
    let lines = on_client(style, true, async |client| client.query(Prompt::from("x")).await).await?;
    assert_eq!(lines, golden(style, "client_query_verbatim")?);
    Ok(())
}

/// The TypeScript SDK has no `ClaudeSDKClient`; its `query()` writes a string prompt in the empty session as one
/// text block, which the TypeScript style follows.
#[tokio::test]
async fn typescript_client_query_writes_the_sdk_mjs_string_prompt() -> TestResult {
    let style = ArgvStyle::TypeScript;
    let lines = on_client(style, false, async |client| client.query(Prompt::from("héllo")).await).await?;
    assert_eq!(lines, golden(style, "string_prompt")?);
    Ok(())
}

/// Python `ClaudeSDKClient.query(AsyncIterable)`: a message without a `session_id` key gets one last, after its
/// own keys; a message that names one keeps it where it is.
#[tokio::test]
async fn python_client_query_stream_fills_only_an_unnamed_session() -> TestResult {
    let style = ArgvStyle::Python;
    let unnamed = || -> Result<UserInput, Box<dyn std::error::Error>> {
        let mut input = UserInput::from(Prompt::from("one"));
        input.additional_fields = object(json!({"future_field": 1}))?;
        Ok(input)
    };
    let named = || -> Result<UserInput, Box<dyn std::error::Error>> {
        Ok(UserInput::from_json(object(
            json!({"type": "user", "session_id": "keep", "message": {"role": "user", "content": "two"}, "parent_tool_use_id": null}),
        )?))
    };
    let inputs = vec![unnamed()?, named()?];
    let lines = on_client(style, false, async |client| {
        client.query_stream(futures_util::stream::iter(inputs)).await
    })
    .await?;
    assert_eq!(lines, golden(style, "client_query_stream")?);
    let inputs = vec![unnamed()?, named()?];
    let lines = on_client(style, false, async |client| {
        client
            .query_stream_in_session(futures_util::stream::iter(inputs), "q")
            .await
    })
    .await?;
    assert_eq!(lines, golden(style, "client_query_stream_in_session")?);
    let inputs = vec![unnamed()?];
    let lines = on_client(style, true, async |client| {
        client.query_stream(futures_util::stream::iter(inputs)).await
    })
    .await?;
    assert_eq!(lines, golden(style, "client_query_stream_verbatim")?);
    Ok(())
}
