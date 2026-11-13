#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests that pin the initialize request bytes, the control request ids and the initialize response
//! checks. The expected lines were captured from `sdk.mjs` 0.3.323 and `claude_agent_sdk` 0.2.181 talking to a
//! scripted CLI, with only the random request id normalized.

use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use trogon_claude_agent_sdk::{
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, DialogKind, Error, HookEvent, HookFailureMode, HookOutput,
    HookRegistration, JsonObject, OutputSchema, Prompt, SystemPrompt, Timeout, TimeoutError, UserDialogRequest,
    UserDialogResponse,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

/// Answers initialize with `response` and records every line written to stdin, byte for byte.
async fn serve(process: ScriptedProcessHarness, response: Value) -> Result<Vec<String>, std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut written = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let request: Value = serde_json::from_str(&line)?;
        if request["request"]["subtype"] == "initialize" {
            let mut frame = serde_json::to_vec(&json!({"type":"control_response","response":{
                "subtype":"success","request_id":request["request_id"],"response":response}}))?;
            frame.push(b'\n');
            stdout.write_all(&frame).await?;
        }
        written.push(line);
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(written)
}

fn accepting() -> Value {
    json!({"hooks_applied":true,"hook_on_failure_supported":true,"claude_code_version":"2.1.319"})
}

type Connected = (
    Result<ClaudeSdkClient, Error>,
    tokio::task::JoinHandle<Result<Vec<String>, std::io::Error>>,
);

async fn connect(options: ClaudeAgentOptions, response: Value) -> Connected {
    let (process, harness) = ScriptedProcessSpawner::new();
    let server = tokio::spawn(serve(harness, response));
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
    .await;
    (client, server)
}

/// Connects, sends `hi`, disconnects and returns the stdin lines.
async fn transcript(options: ClaudeAgentOptions) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (client, server) = connect(options, accepting()).await;
    let client = client?;
    client.send_prompt(Prompt::from("hi")).await?;
    client.disconnect().await?;
    Ok(server.await??)
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

fn typescript_id(id: &str) -> bool {
    id.len() <= 13
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

fn python_id(id: &str) -> bool {
    id.strip_prefix("req_1_").is_some_and(|hex| {
        hex.len() == 8
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn noop_hook() -> Arc<dyn trogon_claude_agent_sdk::HookHandler> {
    Arc::new(|_: trogon_claude_agent_sdk::HookRequest| {
        Box::pin(async { Ok(HookOutput::default()) }) as trogon_claude_agent_sdk::HookFuture
    })
}

/// The hook layout of the captured sessions: an unmatched `PostToolUse` hook, a `Bash` matcher with two blocking
/// hooks and an integer timeout, and an unmatched, empty matcher with a fractional timeout.
fn captured_hooks() -> Result<Vec<HookRegistration>, TimeoutError> {
    Ok(vec![
        HookRegistration {
            event: HookEvent::PostToolUse,
            matcher: None,
            callbacks: vec![noop_hook()],
            timeout: None,
            on_failure: None,
        },
        HookRegistration {
            event: HookEvent::PreToolUse,
            matcher: Some("Bash".into()),
            callbacks: vec![noop_hook(), noop_hook()],
            timeout: Some(Timeout::checked(Duration::from_secs(30))?),
            on_failure: Some(HookFailureMode::Block),
        },
        HookRegistration {
            event: HookEvent::PreToolUse,
            matcher: None,
            callbacks: Vec::new(),
            timeout: Some(Timeout::checked(Duration::from_millis(1500))?),
            on_failure: None,
        },
    ])
}

fn base() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        ..ClaudeAgentOptions::default()
    }
}

fn dialog_handler() -> Arc<dyn trogon_claude_agent_sdk::UserDialogHandler> {
    Arc::new(|_: UserDialogRequest| async { Ok(UserDialogResponse::Cancelled) })
}

#[tokio::test]
async fn typescript_initialize_matches_sdk_mjs_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let schema: JsonObject = serde_json::from_str(r#"{"type":"object","properties":{"a":{"type":"string"}}}"#)?;
    let mut options = base();
    options.hooks = captured_hooks()?;
    options.output_schema = Some(OutputSchema::new(schema));
    options.system_prompt = SystemPrompt::Custom("be brief".into());
    options.supported_dialog_kinds = vec![DialogKind::from("k")];
    options.user_dialog_handler = Some(dialog_handler());
    let lines = transcript(options).await?;
    assert_eq!(
        normalize(&lines[0], r#"{"request_id":""#, typescript_id),
        concat!(
            r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"initialize","#,
            r#""hooks":{"PostToolUse":[{"hookCallbackIds":["hook_0"]}],"#,
            r#""PreToolUse":[{"matcher":"Bash","hookCallbackIds":["hook_1","hook_2"],"timeout":30,"onFailure":"block"},"#,
            r#"{"hookCallbackIds":[],"timeout":1.5}]},"#,
            r#""jsonSchema":{"type":"object","properties":{"a":{"type":"string"}}},"#,
            r#""systemPrompt":["be brief"],"supportedDialogKinds":["k"]}}"#,
        )
    );
    Ok(())
}

#[tokio::test]
async fn typescript_default_initialize_matches_sdk_mjs_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let lines = transcript(base()).await?;
    assert_eq!(
        normalize(&lines[0], r#"{"request_id":""#, typescript_id),
        r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"initialize","systemPrompt":[""]}}"#
    );
    let mut preset = base();
    preset.system_prompt = SystemPrompt::ClaudeCode;
    let lines = transcript(preset).await?;
    assert_eq!(
        normalize(&lines[0], r#"{"request_id":""#, typescript_id),
        r#"{"request_id":"<id>","type":"control_request","request":{"subtype":"initialize"}}"#
    );
    Ok(())
}

#[tokio::test]
async fn python_initialize_matches_claude_agent_sdk_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.argv_style = ArgvStyle::Python;
    options.hooks = captured_hooks()?;
    options.system_prompt = SystemPrompt::Custom("be brief".into());
    let lines = transcript(options).await?;
    assert_eq!(
        normalize(&lines[0], r#""request_id": ""#, python_id),
        concat!(
            r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "initialize", "hooks": "#,
            r#"{"PostToolUse": [{"matcher": null, "hookCallbackIds": ["hook_0"]}], "#,
            r#""PreToolUse": [{"matcher": "Bash", "hookCallbackIds": ["hook_1", "hook_2"], "timeout": 30, "onFailure": "block"}, "#,
            r#"{"matcher": null, "hookCallbackIds": [], "timeout": 1.5}]}}}"#,
        )
    );
    Ok(())
}

#[tokio::test]
async fn python_default_initialize_sends_null_hooks() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.argv_style = ArgvStyle::Python;
    let lines = transcript(options).await?;
    assert_eq!(
        normalize(&lines[0], r#""request_id": ""#, python_id),
        r#"{"type": "control_request", "request_id": "<id>", "request": {"subtype": "initialize", "hooks": null}}"#
    );
    Ok(())
}

#[tokio::test]
async fn dialog_kinds_without_a_handler_are_refused_before_initialize() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.supported_dialog_kinds = vec![DialogKind::from("k")];
    let (client, _server) = connect(options, accepting()).await;
    let error = client.err().ok_or("dialog kinds without a handler must be refused")?;
    assert!(matches!(error, Error::DialogKindsWithoutHandler), "{error:?}");
    assert_eq!(
        error.to_string(),
        "supportedDialogKinds requires an onUserDialog callback -- declaring dialog kinds without a handler would \
         park dialogs nothing can answer. Provide onUserDialog, or omit supportedDialogKinds."
    );
    Ok(())
}

fn blocking_hook() -> Vec<HookRegistration> {
    vec![HookRegistration {
        event: HookEvent::Stop,
        matcher: None,
        callbacks: vec![noop_hook()],
        timeout: None,
        on_failure: Some(HookFailureMode::Block),
    }]
}

#[tokio::test]
async fn blocking_hooks_need_on_failure_support() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.hooks = blocking_hook();
    let (client, _server) = connect(options, json!({"hooks_applied":true})).await;
    let error = client.err().ok_or("a CLI without on_failure support must be refused")?;
    assert!(matches!(error, Error::BlockingHooksUnsupported), "{error:?}");
    assert_eq!(
        error.to_string(),
        "The process answering initialize does not honor on_failure on hook matchers, so a hook that fails would \
         not block. Update Claude Code if it is old, or remove on_failure=\"block\" from your hooks."
    );
    assert_eq!(
        error.typescript_message().as_deref(),
        Some(
            "The process answering initialize does not honor onFailure on hook matchers, so a hook that fails would \
         not block. Update Claude Code if it is old, or remove onFailure: \"block\" from your hooks."
        )
    );
    Ok(())
}

#[tokio::test]
async fn hooks_that_were_not_applied_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = base();
    options.hooks = blocking_hook();
    let (client, _server) = connect(options, json!({"hooks_applied":false,"hook_on_failure_supported":true})).await;
    let error = client.err().ok_or("unapplied hooks must be refused")?;
    assert!(matches!(error, Error::HooksNotApplied), "{error:?}");
    assert_eq!(
        error.typescript_message().as_deref(),
        Some(
            "This session was already running and did not register your hooks, so a hook set to onFailure: \"block\" \
         would never run. Start a new session, or remove onFailure: \"block\" from your hooks."
        )
    );
    Ok(())
}

#[tokio::test]
async fn hooks_applied_false_only_matters_for_blocking_hooks() -> Result<(), Box<dyn std::error::Error>> {
    let (client, server) = connect(base(), json!({"hooks_applied":false})).await;
    client?.disconnect().await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn a_non_object_initialize_response_is_kept_in_typescript_and_empty_in_python()
-> Result<(), Box<dyn std::error::Error>> {
    for (style, expected) in [(ArgvStyle::TypeScript, json!(42)), (ArgvStyle::Python, json!({}))] {
        let mut options = base();
        options.argv_style = style;
        let (client, server) = connect(options, json!(42)).await;
        let client = client?;
        assert_eq!(client.connection_info().as_json(), &expected);
        client.disconnect().await?;
        server.await??;
    }
    Ok(())
}
