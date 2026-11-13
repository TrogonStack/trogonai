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
    ArgvStyle, ClaudeAgentOptions, ClaudeSdkClient, ComputerFolder, DenialDisposition, DuplicateControlRequests,
    FeatureSupport, JsonObject, MatchedAskRule, PermissionDecision, PermissionDecisionClassification, PermissionMode,
    PermissionUpdate, PermissionUpdateDestination, ToolPermissionRequest, ToolUseId,
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
