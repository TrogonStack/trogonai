#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Golden tests that pin how inbound control requests are answered. The expected lines were captured from
//! `sdk.mjs` 0.3.323 and `claude_agent_sdk` 0.2.181 answering the same requests from a scripted CLI with
//! the same handlers.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream},
    sync::oneshot,
};
use trogon_claude_agent_sdk::{
    ArgvStyle, CallbackError, ClaudeAgentOptions, ClaudeSdkClient, ComputerFolder, DenialDisposition, DialogKind,
    DuplicateControlRequests, ElicitationMode, ElicitationRequest, ElicitationResponse, FeatureSupport,
    HostAuthTokenRequest, JsonObject, MatchedAskRule, OAuthTokenRequest, OAuthTokenResponse, OAuthTokenState,
    PermissionDecision, PermissionDecisionClassification, PermissionMode, PermissionUpdate,
    PermissionUpdateDestination, ToolPermissionRequest, ToolUseId, UserDialogRequest, UserDialogResponse,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{ScriptedProcessHarness, ScriptedProcessSpawner};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One thing the scripted CLI does after `initialize` is answered.
enum Step {
    Send(Value),
    Wait(u64),
}

async fn write(stdout: &mut DuplexStream, frame: &Value) -> Result<(), std::io::Error> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    stdout.write_all(&line).await
}

/// Answers the SDK's own control requests with `{}`, plays `steps` once `initialize` is answered, and reports
/// every `control_response` line the SDK writes until `expected` arrived and a quiet grace period passed. It
/// then keeps reading until the SDK closes stdin.
async fn serve(
    process: ScriptedProcessHarness,
    steps: Vec<Step>,
    expected: usize,
    report: oneshot::Sender<Vec<String>>,
) -> Result<(), std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut responses = Vec::new();
    let mut steps = Some(steps);
    let mut report = Some(report);
    loop {
        let grace = match (&steps, responses.len() >= expected) {
            (Some(_), _) => Duration::from_secs(30),
            (None, true) => Duration::from_millis(300),
            (None, false) => Duration::from_secs(5),
        };
        let line = match tokio::time::timeout(grace, lines.next_line()).await {
            Ok(line) => line?,
            Err(_) => {
                if let Some(report) = report.take() {
                    let _ = report.send(std::mem::take(&mut responses));
                }
                lines.next_line().await?
            }
        };
        let Some(line) = line else { break };
        let frame: Value = serde_json::from_str(&line)?;
        if frame["type"] == "control_request" {
            write(
                &mut stdout,
                &json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{}}}),
            )
            .await?;
            if frame["request"]["subtype"] == "initialize"
                && let Some(steps) = steps.take()
            {
                for step in steps {
                    match step {
                        Step::Send(frame) => write(&mut stdout, &frame).await?,
                        Step::Wait(millis) => tokio::time::sleep(Duration::from_millis(millis)).await,
                    }
                }
            }
        } else if frame["type"] == "control_response" && report.is_some() {
            responses.push(line);
        }
    }
    if let Some(report) = report.take() {
        let _ = report.send(responses);
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(())
}

/// Connects a client with `options`, plays `steps` and returns the response lines the SDK wrote.
async fn run(
    mut options: ClaudeAgentOptions,
    steps: Vec<Step>,
    expected: usize,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let (report, reported) = oneshot::channel();
    let server = tokio::spawn(serve(harness, steps, expected, report));
    options.cli_path = Some("claude".into());
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
    let responses = reported.await?;
    client.disconnect().await?;
    server.await??;
    Ok(responses)
}

fn options(style: ArgvStyle) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: style,
        ..ClaudeAgentOptions::default()
    }
}

fn request(id: &str, request: Value) -> Step {
    Step::Send(json!({"type":"control_request","request_id":id,"request":request}))
}

fn bash(id: &str, tool_use_id: Option<&str>) -> Step {
    let mut body = json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm"}});
    if let Some(tool_use_id) = tool_use_id {
        body["tool_use_id"] = tool_use_id.into();
    }
    request(id, body)
}

fn with_permission(
    style: ArgvStyle,
    decide: impl Fn(ArgvStyle) -> PermissionDecision + Send + Sync + 'static,
) -> ClaudeAgentOptions {
    let mut configured = options(style);
    configured.permission_handler = Some(Arc::new(move |_: ToolPermissionRequest| {
        let decision = decide(style);
        async move { decision }
    }));
    configured
}

/// `sdk.mjs` keeps the user's classification; `claude_agent_sdk` has no way to send one.
fn classification(
    style: ArgvStyle,
    value: PermissionDecisionClassification,
) -> Option<PermissionDecisionClassification> {
    (style == ArgvStyle::TypeScript).then_some(value)
}

#[tokio::test]
async fn permission_requests_carry_every_typescript_context_field() -> TestResult {
    let seen = Arc::new(Mutex::new(Vec::<ToolPermissionRequest>::new()));
    let observed = Arc::clone(&seen);
    let mut configured = options(ArgvStyle::TypeScript);
    configured.permission_handler = Some(Arc::new(move |request: ToolPermissionRequest| {
        if let Ok(mut seen) = observed.lock() {
            seen.push(request);
        }
        async { PermissionDecision::allow() }
    }));
    let responses = run(
        configured,
        vec![
            request(
                "p1",
                json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"rm -rf build"},
                    "permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}],
                    "blocked_path":"/repo/build","decision_reason":"matched ask rule","decision_reason_type":"rule",
                    "decision_reason_code":"ask_rule","server_policy_ask_only":true,"title":"Run command",
                    "display_name":"Bash","description":"Deletes build","default_to_no":true,
                    "suppress_always_allow_rule":false,"tool_use_id":"tu-1","agent_id":"agent-1","plugin":"fmt",
                    "requires_user_interaction":true,"server_prompt":"Allow?",
                    "computer_folder":{"path":"/repo","computer_name":"box"},
                    "matched_ask_rule":{"source":"userSettings","tool_name":"Bash","rule_content":"rm:*"},
                    "mcp_server":{"name":"local","source":"project"},"classifier_approvable":false}),
            ),
            request(
                "p2",
                json!({"subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"a"},
                    "decision_reason_type":7,"server_policy_ask_only":"yes","plugin":null,
                    "computer_folder":{"path":"/repo"},
                    "matched_ask_rule":{"source":"localSettings","tool_name":"Read"}}),
            ),
        ],
        2,
    )
    .await?;
    assert_eq!(
        responses,
        [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"p1","response":{"behavior":"allow","toolUseID":"tu-1"}}}"#,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"p2","response":{"behavior":"allow"}}}"#,
        ]
    );
    let seen = seen.lock().map_err(|_| "permission requests lock poisoned")?;
    let [full, lenient] = seen.as_slice() else {
        return Err("expected two permission requests".into());
    };
    assert_eq!(full.request_id.as_str(), "p1");
    assert_eq!(full.tool_use_id.as_ref().map(ToolUseId::as_str), Some("tu-1"));
    assert_eq!(full.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(full.blocked_path.as_deref(), Some("/repo/build"));
    assert_eq!(full.decision_reason.as_deref(), Some("matched ask rule"));
    assert_eq!(full.decision_reason_type.as_deref(), Some("rule"));
    assert_eq!(full.decision_reason_code.as_deref(), Some("ask_rule"));
    assert_eq!(full.server_policy_ask_only, FeatureSupport::Supported);
    assert_eq!(full.title.as_deref(), Some("Run command"));
    assert_eq!(full.display_name.as_deref(), Some("Bash"));
    assert_eq!(full.description.as_deref(), Some("Deletes build"));
    assert_eq!(full.default_to_no, FeatureSupport::Supported);
    assert_eq!(full.suppress_always_allow_rule, FeatureSupport::Unsupported);
    assert_eq!(full.requires_user_interaction, FeatureSupport::Supported);
    assert_eq!(full.plugin.as_deref(), Some("fmt"));
    assert_eq!(full.server_prompt.as_deref(), Some("Allow?"));
    assert_eq!(
        full.computer_folder,
        Some(ComputerFolder {
            path: "/repo".into(),
            computer_name: "box".into(),
        })
    );
    assert_eq!(
        full.matched_ask_rule,
        Some(MatchedAskRule {
            source: "userSettings".into(),
            tool_name: "Bash".into(),
            rule_content: Some("rm:*".into()),
        })
    );
    assert!(
        full.mcp_server
            .as_ref()
            .is_some_and(|server| server.name == "local" && server.source == "project")
    );
    assert_eq!(full.suggestions.len(), 1);
    assert_eq!(
        Value::Object(full.additional_fields.clone()),
        json!({"classifier_approvable":false})
    );

    assert_eq!(lenient.request_id.as_str(), "p2");
    assert_eq!(lenient.decision_reason_type, None);
    assert_eq!(lenient.server_policy_ask_only, FeatureSupport::NotReported);
    assert_eq!(lenient.plugin, None);
    assert_eq!(lenient.computer_folder, None);
    assert_eq!(
        lenient.matched_ask_rule,
        Some(MatchedAskRule {
            source: "localSettings".into(),
            tool_name: "Read".into(),
            rule_content: None,
        })
    );
    // TypeScript ignores values of the wrong type; they stay available here instead of being dropped.
    assert_eq!(
        Value::Object(lenient.additional_fields.clone()),
        json!({"decision_reason_type":7,"server_policy_ask_only":"yes","plugin":null,"computer_folder":{"path":"/repo"}})
    );
    Ok(())
}

#[tokio::test]
async fn python_permission_answers_echo_the_original_input() -> TestResult {
    let responses = run(
        with_permission(ArgvStyle::Python, |_| PermissionDecision::allow()),
        vec![bash("p1", Some("tu-1"))],
        1,
    )
    .await?;
    assert_eq!(
        responses,
        [
            r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "p1", "response": {"behavior": "allow", "updatedInput": {"command": "rm"}}}}"#
        ]
    );
    Ok(())
}

fn allow_full(style: ArgvStyle) -> PermissionDecision {
    let mut input = JsonObject::new();
    input.insert("command".into(), "ls".into());
    PermissionDecision::Allow {
        updated_input: Some(input),
        updated_permissions: Some(vec![PermissionUpdate::SetMode {
            mode: PermissionMode::Plan,
            destination: PermissionUpdateDestination::Session,
        }]),
        decision_classification: classification(style, PermissionDecisionClassification::UserPermanent),
    }
}

fn deny_interrupt(style: ArgvStyle) -> PermissionDecision {
    PermissionDecision::Deny {
        message: "no".into(),
        disposition: DenialDisposition::Interrupt,
        decision_classification: classification(style, PermissionDecisionClassification::UserReject),
    }
}

#[tokio::test]
async fn permission_decisions_are_written_byte_for_byte() -> TestResult {
    type Decide = fn(ArgvStyle) -> PermissionDecision;
    let cases: [(&str, Decide, Option<&str>, &str, &str); 5] = [
        (
            "allow with every field",
            allow_full,
            Some("tu-1"),
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"p1","response":{"behavior":"allow","updatedInput":{"command":"ls"},"updatedPermissions":[{"type":"setMode","mode":"plan","destination":"session"}],"decisionClassification":"user_permanent","toolUseID":"tu-1"}}}"#,
            r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "p1", "response": {"behavior": "allow", "updatedInput": {"command": "ls"}, "updatedPermissions": [{"type": "setMode", "destination": "session", "mode": "plan"}]}}}"#,
        ),
        (
            "deny with interrupt",
            deny_interrupt,
            Some("tu-1"),
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"p1","response":{"behavior":"deny","message":"no","interrupt":true,"decisionClassification":"user_reject","toolUseID":"tu-1"}}}"#,
            r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "p1", "response": {"behavior": "deny", "message": "no", "interrupt": true}}}"#,
        ),
        (
            "deny",
            |_| PermissionDecision::deny("no"),
            None,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"p1","response":{"behavior":"deny","message":"no"}}}"#,
            r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "p1", "response": {"behavior": "deny", "message": "no"}}}"#,
        ),
        (
            "handler failure",
            |_| PermissionDecision::Failed { message: "boom".into() },
            Some("tu-1"),
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"p1","error":"boom"}}"#,
            r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "p1", "error": "boom"}}"#,
        ),
        (
            "null result",
            |_| PermissionDecision::AnsweredOutOfBand,
            Some("tu-1"),
            "",
            r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "p1", "error": "Tool permission callback must return PermissionResult (PermissionResultAllow or PermissionResultDeny), got <class 'NoneType'>"}}"#,
        ),
    ];
    for (name, decide, tool_use_id, typescript, python) in cases {
        for (style, expected) in [(ArgvStyle::TypeScript, typescript), (ArgvStyle::Python, python)] {
            let expected: Vec<&str> = (!expected.is_empty()).then_some(expected).into_iter().collect();
            let responses = run(
                with_permission(style, decide),
                vec![bash("p1", tool_use_id)],
                expected.len(),
            )
            .await?;
            assert_eq!(responses, expected, "{name} ({style:?})");
        }
    }
    Ok(())
}

fn unanswered_requests() -> Vec<Step> {
    [
        ("a1", json!({"subtype":"can_use_tool","tool_name":"Bash","input":{}})),
        (
            "a2",
            json!({"subtype":"hook_callback","callback_id":"missing","input":{}}),
        ),
        (
            "a3",
            json!({"subtype":"elicitation","mcp_server_name":"local","message":"m","mode":"form"}),
        ),
        (
            "a4",
            json!({"subtype":"request_user_dialog","dialog_kind":"fixture","payload":{}}),
        ),
        ("a5", json!({"subtype":"future_thing"})),
        ("a6", json!({"subtype":"remote_tool_call"})),
        ("a7", json!({"subtype":"remote_tools_probe"})),
        ("a8", json!({"subtype":"oauth_token_refresh"})),
        ("a9", json!({"subtype":"host_auth_token_refresh"})),
        ("a10", json!({"subtype":"remote_control_work_secret"})),
        ("a11", json!({"subtype":"ui_render"})),
    ]
    .into_iter()
    .map(|(id, body)| request(id, body))
    .collect()
}

#[tokio::test]
async fn typescript_answers_requests_without_a_handler_like_sdk_mjs() -> TestResult {
    let expected = [
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"a1","error":"canUseTool callback is not provided."}}"#,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"a2","error":"No hook callback found for ID: missing"}}"#,
        r#"{"type":"control_response","response":{"subtype":"success","request_id":"a3","response":{"action":"decline"}}}"#,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"a5","error":"Unsupported control request subtype: future_thing","error_code":"unsupported_request"}}"#,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"a8","error":"getOAuthToken callback is not provided."}}"#,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"a9","error":"getHostAuthToken callback is not provided."}}"#,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"a10","error":"refreshWorkSecret callback is not provided."}}"#,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"a11","error":"Unsupported control request subtype: ui_render","error_code":"unsupported_request"}}"#,
    ];
    let responses = run(options(ArgvStyle::TypeScript), unanswered_requests(), expected.len()).await?;
    assert_eq!(responses, expected);
    Ok(())
}

#[tokio::test]
async fn python_answers_requests_without_a_handler_like_claude_agent_sdk() -> TestResult {
    let expected = [
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a1", "error": "canUseTool callback is not provided"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a2", "error": "No hook callback found for ID: missing"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a3", "error": "Unsupported control request subtype: elicitation"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a4", "error": "Unsupported control request subtype: request_user_dialog"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a5", "error": "Unsupported control request subtype: future_thing"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a6", "error": "Unsupported control request subtype: remote_tool_call"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a7", "error": "Unsupported control request subtype: remote_tools_probe"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a8", "error": "Unsupported control request subtype: oauth_token_refresh"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a9", "error": "Unsupported control request subtype: host_auth_token_refresh"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a10", "error": "Unsupported control request subtype: remote_control_work_secret"}}"#,
        r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a11", "error": "Unsupported control request subtype: ui_render"}}"#,
    ];
    let responses = run(options(ArgvStyle::Python), unanswered_requests(), expected.len()).await?;
    assert_eq!(responses, expected);
    Ok(())
}

/// `claude_agent_sdk` has no token callbacks, so a handler set on the options never runs under the Python
/// dialect and the requests get its plain unsupported error, the same lines as with no handler.
#[tokio::test]
async fn python_answers_token_refreshes_as_unsupported_even_with_handlers() -> TestResult {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut configured = options(ArgvStyle::Python);
    let oauth_calls = calls.clone();
    configured.oauth_token_handler = Some(Arc::new(move |_: OAuthTokenRequest| {
        if let Ok(mut calls) = oauth_calls.lock() {
            calls.push("oauth");
        }
        async { Ok::<_, CallbackError>(OAuthTokenResponse::token("t")) }
    }));
    let host_calls = calls.clone();
    configured.host_auth_token_handler = Some(Arc::new(move |_: HostAuthTokenRequest| {
        if let Ok(mut calls) = host_calls.lock() {
            calls.push("host");
        }
        async { Ok::<_, CallbackError>(Some("h".to_owned())) }
    }));
    let steps = vec![
        request("a8", json!({"subtype":"oauth_token_refresh"})),
        request("a9", json!({"subtype":"host_auth_token_refresh"})),
    ];
    assert_eq!(
        run(configured, steps, 2).await?,
        [
            r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a8", "error": "Unsupported control request subtype: oauth_token_refresh"}}"#,
            r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "a9", "error": "Unsupported control request subtype: host_auth_token_refresh"}}"#,
        ]
    );
    assert!(calls.lock().map_err(|_| "poisoned")?.is_empty());
    Ok(())
}

/// `sdk.mjs` compares `token_state` to its two known strings, so any other value, a non-string one included,
/// reaches `getOAuthToken` as no hint and the request is still answered.
#[tokio::test]
async fn a_non_string_token_state_reaches_the_oauth_handler_as_no_hint() -> TestResult {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let mut configured = options(ArgvStyle::TypeScript);
    configured.oauth_token_handler = Some(Arc::new(move |request: OAuthTokenRequest| {
        if let Ok(mut seen) = recorded.lock() {
            seen.push((request.kind.clone(), request.token_state));
        }
        async { Ok::<_, CallbackError>(OAuthTokenResponse::token("t")) }
    }));
    let steps = vec![
        request(
            "t1",
            json!({"subtype":"oauth_token_refresh","kind":"proactive","token_state":7}),
        ),
        request("t2", json!({"subtype":"oauth_token_refresh","token_state":"refused"})),
    ];
    let responses = run(configured, steps, 2).await?;
    for (line, id) in responses.iter().zip(["t1", "t2"]) {
        let frame: Value = serde_json::from_str(line)?;
        assert_eq!(frame["response"]["subtype"], "success", "{line}");
        assert_eq!(frame["response"]["request_id"], id, "{line}");
    }
    assert_eq!(
        *seen.lock().map_err(|_| "poisoned")?,
        [(Some(json!("proactive")), None), (None, Some(OAuthTokenState::Refused))]
    );
    Ok(())
}

fn cancelled(style: ArgvStyle) -> ClaudeAgentOptions {
    let mut configured = options(style);
    configured.permission_handler = Some(Arc::new(|request: ToolPermissionRequest| async move {
        request.signal.cancelled().await;
        PermissionDecision::allow()
    }));
    configured
}

#[tokio::test]
async fn a_cancelled_handler_still_answers_in_typescript_and_stays_silent_in_python() -> TestResult {
    let steps = || {
        vec![
            bash("p1", Some("tu-1")),
            Step::Wait(150),
            Step::Send(json!({"type":"control_cancel_request","request_id":"p1"})),
        ]
    };
    assert_eq!(
        run(cancelled(ArgvStyle::TypeScript), steps(), 1).await?,
        [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"p1","response":{"behavior":"allow","toolUseID":"tu-1"}}}"#
        ]
    );
    assert!(run(cancelled(ArgvStyle::Python), steps(), 0).await?.is_empty());
    Ok(())
}

fn duplicated(style: ArgvStyle, policy: DuplicateControlRequests, seen: Arc<Mutex<Vec<Value>>>) -> ClaudeAgentOptions {
    let mut configured = options(style);
    configured.limits.duplicate_control_requests = policy;
    configured.permission_handler = Some(Arc::new(move |request: ToolPermissionRequest| {
        if let Ok(mut seen) = seen.lock() {
            seen.push(Value::Object(request.input.clone()));
        }
        async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            PermissionDecision::allow()
        }
    }));
    configured
}

#[tokio::test]
async fn a_redelivered_in_flight_request_is_skipped_or_run_twice() -> TestResult {
    let steps = || {
        vec![
            request(
                "p1",
                json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"one"}}),
            ),
            request(
                "p1",
                json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"two"}}),
            ),
        ]
    };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let responses = run(
        duplicated(ArgvStyle::TypeScript, DuplicateControlRequests::Skip, Arc::clone(&seen)),
        steps(),
        1,
    )
    .await?;
    assert_eq!(
        responses,
        [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"p1","response":{"behavior":"allow"}}}"#
        ]
    );
    assert_eq!(*seen.lock().map_err(|_| "lock poisoned")?, [json!({"command":"one"})]);

    let seen = Arc::new(Mutex::new(Vec::new()));
    let responses = run(
        duplicated(ArgvStyle::Python, DuplicateControlRequests::RunBoth, Arc::clone(&seen)),
        steps(),
        2,
    )
    .await?;
    assert_eq!(
        responses,
        [
            r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "p1", "response": {"behavior": "allow", "updatedInput": {"command": "one"}}}}"#,
            r#"{"type": "control_response", "response": {"subtype": "success", "request_id": "p1", "response": {"behavior": "allow", "updatedInput": {"command": "two"}}}}"#,
        ]
    );
    assert_eq!(
        *seen.lock().map_err(|_| "lock poisoned")?,
        [json!({"command":"one"}), json!({"command":"two"})]
    );
    Ok(())
}

#[tokio::test]
async fn elicitation_passes_unknown_modes_and_a_null_answer_writes_nothing() -> TestResult {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let mut configured = options(ArgvStyle::TypeScript);
    configured.elicitation_handler = Some(Arc::new(move |request: ElicitationRequest| {
        let silent = request.message == "silent";
        if let Ok(mut seen) = observed.lock() {
            seen.push((request.request_id.as_str().to_owned(), request.mode.clone()));
        }
        async move {
            if silent {
                return Ok::<_, CallbackError>(ElicitationResponse::AnsweredOutOfBand);
            }
            let mut content = JsonObject::new();
            content.insert("a".into(), 1.into());
            Ok::<_, CallbackError>(ElicitationResponse::Accept { content: Some(content) })
        }
    }));
    let responses = run(
        configured,
        vec![
            request(
                "e1",
                json!({"subtype":"elicitation","mcp_server_name":"local","message":"Pick","mode":"form",
                    "requested_schema":{"type":"object"},"elicitation_id":"el-1","title":"T","display_name":"D",
                    "description":"Desc"}),
            ),
            request(
                "e2",
                json!({"subtype":"elicitation","mcp_server_name":"local","message":"silent","mode":"future"}),
            ),
        ],
        1,
    )
    .await?;
    assert_eq!(
        responses,
        [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"e1","response":{"action":"accept","content":{"a":1}}}}"#
        ]
    );
    assert_eq!(
        *seen.lock().map_err(|_| "lock poisoned")?,
        [
            ("e1".to_owned(), Some(ElicitationMode::Form)),
            ("e2".to_owned(), Some(ElicitationMode::Other("future".into()))),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn every_dialog_kind_reaches_the_handler_and_a_null_answer_writes_nothing() -> TestResult {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let mut configured = options(ArgvStyle::TypeScript);
    configured.supported_dialog_kinds = vec![DialogKind::from("fixture")];
    configured.user_dialog_handler = Some(Arc::new(move |request: UserDialogRequest| {
        let kind = request.dialog_kind.as_str().to_owned();
        if let Ok(mut seen) = observed.lock() {
            seen.push((request.request_id.as_str().to_owned(), kind.clone()));
        }
        async move {
            Ok::<_, CallbackError>(match kind.as_str() {
                "fixture" => UserDialogResponse::Completed { result: json!({"x":1}) },
                "silent" => UserDialogResponse::AnsweredOutOfBand,
                _ => UserDialogResponse::Cancelled,
            })
        }
    }));
    let responses = run(
        configured,
        vec![
            request(
                "d1",
                json!({"subtype":"request_user_dialog","dialog_kind":"fixture","payload":{"choice":1},"tool_use_id":"tu-1"}),
            ),
            request("d2", json!({"subtype":"request_user_dialog","dialog_kind":"undeclared","payload":{}})),
            request("d3", json!({"subtype":"request_user_dialog","dialog_kind":"silent","payload":{}})),
        ],
        2,
    )
    .await?;
    assert_eq!(
        responses,
        [
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"d1","response":{"behavior":"completed","result":{"x":1}}}}"#,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"d2","response":{"behavior":"cancelled"}}}"#,
        ]
    );
    assert_eq!(
        *seen.lock().map_err(|_| "lock poisoned")?,
        [
            ("d1".to_owned(), "fixture".to_owned()),
            ("d2".to_owned(), "undeclared".to_owned()),
            ("d3".to_owned(), "silent".to_owned()),
        ]
    );
    Ok(())
}
