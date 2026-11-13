#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Pins the TypeScript-only callback options against goldens captured from `sdk.mjs` 0.3.323: the token callbacks,
//! `toolConfig`, `workload` and the executable rules. `connect_with_adapters` is the equivalent of
//! `spawnClaudeCodeProcess`: the spawner receives the program, arguments, directory and environment overrides.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
};
use trogon_claude_agent_sdk::{
    ArgvStyle, AskUserQuestionConfig, CallbackError, ClaudeAgentOptions, ClaudeSdkClient, ConfigValue, Environment,
    EnvironmentMode, ExtraArgument, HostAuthTokenRequest, OAuthDeclineReason, OAuthTokenRequest, OAuthTokenResponse,
    QuestionPreviewFormat, ScriptRuntime, ToolConfig,
    adapters::{NativeFileSystem, RuntimeAdapters},
    test_support::FixedEnvironment,
};

#[path = "support/scripted_process.rs"]
mod scripted_process;

use scripted_process::{CapturedProcessCommand, ScriptedProcessHarness, ScriptedProcessSpawner};

type SeenRequests = Arc<Mutex<Vec<(Option<Value>, Option<String>)>>>;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDEN: &str = include_str!("fixtures/callbacks_ts_golden.json");

fn golden() -> Result<Value, serde_json::Error> {
    serde_json::from_str(GOLDEN)
}

struct Session {
    command: CapturedProcessCommand,
    responses: Vec<Value>,
}

/// Answers `initialize`, then sends `requests` (each `{"request_id", "request"}`) and gathers their responses.
async fn serve(
    process: ScriptedProcessHarness,
    requests: Vec<Value>,
    done: oneshot::Sender<()>,
) -> Result<Vec<Value>, std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut responses = Vec::new();
    let mut done = Some(done);
    // A repeated delivery of an in-flight request is dropped, so it is answered once.
    let expected = requests
        .iter()
        .map(|request| request["request_id"].to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if requests.is_empty()
        && let Some(done) = done.take()
    {
        let _ = done.send(());
    }
    while let Some(line) = lines.next_line().await? {
        let frame: Value = serde_json::from_str(&line)?;
        if frame["request"]["subtype"] == "initialize" {
            let mut reply = serde_json::to_vec(&json!({"type":"control_response","response":{
                "subtype":"success","request_id":frame["request_id"],"response":{"hooks_applied":true,"hook_on_failure_supported":true,"claude_code_version":"2.1.319"}}}))?;
            reply.push(b'\n');
            stdout.write_all(&reply).await?;
            for request in &requests {
                let mut sent = serde_json::to_vec(&json!({
                    "type": "control_request",
                    "request_id": request["request_id"],
                    "request": request["request"],
                }))?;
                sent.push(b'\n');
                stdout.write_all(&sent).await?;
            }
        } else if frame["type"] == "control_response" {
            responses.push(frame["response"].clone());
            if responses.len() == expected
                && let Some(done) = done.take()
            {
                let _ = done.send(());
            }
        }
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(responses)
}

async fn run(
    options: ClaudeAgentOptions,
    variables: &[(&str, &str)],
    requests: Vec<Value>,
) -> Result<Session, Box<dyn std::error::Error>> {
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let (done, finished) = oneshot::channel();
    let server = tokio::spawn(serve(harness, requests, done));
    let variables: BTreeMap<OsString, OsString> = variables.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
    let client = ClaudeSdkClient::connect_with_adapters(
        options,
        RuntimeAdapters {
            environment: FixedEnvironment {
                directory: PathBuf::from("unused-directory"),
                variables,
            },
            process,
            filesystem: NativeFileSystem,
        },
    )
    .await?;
    finished.await?;
    client.disconnect().await?;
    let responses = server.await??;
    let command = observer.commands()?.remove(0);
    Ok(Session { command, responses })
}

fn base() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        ..ClaudeAgentOptions::default()
    }
}

fn arguments(session: &Session) -> Vec<String> {
    session
        .command
        .arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The six variables `sdk.mjs` touches, as the child sees them: the inherited ones, then the overrides.
fn child_view(session: &Session, inherited: &[(&str, &str)], clear: bool) -> BTreeMap<String, String> {
    const NAMES: [&str; 6] = [
        "CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH",
        "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH",
        "CLAUDE_CODE_QUESTION_PREVIEW_FORMAT",
        "CLAUDE_CODE_QUESTION_EXTENDED",
        "CLAUDE_CODE_QUESTION_OPTIONAL_DESCRIPTIONS",
        "CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES",
    ];
    let mut view: BTreeMap<String, String> = if clear {
        BTreeMap::new()
    } else {
        inherited
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    };
    for (key, value) in &session.command.environment {
        let key = key.to_string_lossy().into_owned();
        match value {
            Some(value) => view.insert(key, value.to_string_lossy().into_owned()),
            None => view.remove(&key),
        };
    }
    view.retain(|key, _| NAMES.iter().any(|name| key.eq_ignore_ascii_case(name)));
    view
}

fn golden_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

fn oauth_handler(
    behavior: impl Fn(usize) -> Result<OAuthTokenResponse, CallbackError> + Send + Sync + 'static,
    seen: SeenRequests,
) -> Arc<dyn trogon_claude_agent_sdk::OAuthTokenHandler> {
    let calls = Arc::new(AtomicUsize::new(0));
    Arc::new(move |request: OAuthTokenRequest| {
        let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut seen) = seen.lock() {
            seen.push((
                request.kind.clone(),
                request.token_state.map(|state| format!("{state:?}")),
            ));
        }
        let outcome = behavior(call);
        async move { outcome }
    })
}

fn requests(subtypes: &[Value]) -> Vec<Value> {
    subtypes
        .iter()
        .enumerate()
        .map(|(index, request)| json!({"request_id": format!("r{index}"), "request": request}))
        .collect()
}

fn by_id(session: &Session) -> Vec<Value> {
    let mut responses = session.responses.clone();
    responses.sort_by_key(|response| response["request_id"].as_str().map(str::to_owned));
    responses
}

#[tokio::test]
async fn tool_config_and_callbacks_set_the_environment_the_bundle_sets() -> TestResult {
    let golden = golden()?["tool_environment"].clone();
    let odd = [
        ("claude_code_question_extended", "old"),
        ("CLAUDE_CODE_AGENT_PROGRESS_SUMMARIES", "1"),
        ("Claude_Code_Question_Optional_Descriptions", "x"),
    ];
    let mut all = ClaudeAgentOptions {
        environment_mode: EnvironmentMode::Inherit,
        tool_config: ToolConfig {
            ask_user_question: AskUserQuestionConfig {
                preview_format: Some(QuestionPreviewFormat::Html),
                extended_questions: true,
                optional_descriptions: false,
            },
        },
        ..base()
    };
    all.oauth_token_handler = Some(oauth_handler(|_| Ok(OAuthTokenResponse::token("t")), Arc::default()));
    all.host_auth_token_handler = Some(Arc::new(|_: HostAuthTokenRequest| async { Ok(Some("h".to_owned())) }));
    let session = run(all, &odd, Vec::new()).await?;
    assert_eq!(
        child_view(&session, &odd, false),
        golden_map(&golden["all_options_no_env"])
    );

    let none = ClaudeAgentOptions {
        environment_mode: EnvironmentMode::Inherit,
        ..base()
    };
    let session = run(none, &odd, Vec::new()).await?;
    assert_eq!(
        child_view(&session, &odd, false),
        golden_map(&golden["no_options_no_env"])
    );

    let explicit = ClaudeAgentOptions {
        environment_mode: EnvironmentMode::Replace,
        env: Environment::from_iter([("claude_code_question_extended".to_owned(), ConfigValue::from("q"))]),
        tool_config: ToolConfig {
            ask_user_question: AskUserQuestionConfig {
                optional_descriptions: true,
                ..AskUserQuestionConfig::default()
            },
        },
        ..base()
    };
    let session = run(explicit, &odd, Vec::new()).await?;
    assert_eq!(child_view(&session, &odd, true), golden_map(&golden["explicit_env"]));
    Ok(())
}

#[tokio::test]
async fn workload_overrides_an_extra_argument_in_place_like_the_bundle() -> TestResult {
    let golden = golden()?;
    let options = ClaudeAgentOptions {
        workload: Some("cron".into()),
        extra_args: vec![
            ExtraArgument {
                flag: "foo".into(),
                value: Some("x".into()),
            },
            ExtraArgument {
                flag: "workload".into(),
                value: Some("old".into()),
            },
            ExtraArgument {
                flag: "bar".into(),
                value: None,
            },
        ],
        ..base()
    };
    let session = run(options, &[], Vec::new()).await?;
    assert_eq!(arguments(&session)[5..], strings(&golden["workload_argv_tail"])[..]);
    Ok(())
}

#[tokio::test]
async fn executable_rules_match_the_bundle_for_native_and_script_paths() -> TestResult {
    let golden = golden()?["spawn_argv"].clone();
    let runtime = ScriptRuntime {
        executable: Some("node".into()),
        arguments: vec!["--no-warnings".into()],
    };
    let native_tail = strings(&golden["native"]["argv"]);
    let script_tail = strings(&golden["script"]["argv"]);

    for path in ["/cli/rec-native", "/cli/rec-native.JS"] {
        let options = ClaudeAgentOptions {
            cli_path: Some(path.into()),
            script_runtime: runtime.clone(),
            ..base()
        };
        let session = run(options, &[], Vec::new()).await?;
        assert_eq!(session.command.executable, OsString::from(path));
        assert_eq!(arguments(&session), native_tail, "{path}");
    }

    let options = ClaudeAgentOptions {
        cli_path: Some("/cli/rec.js".into()),
        script_runtime: runtime,
        cwd: Some("/work".into()),
        ..base()
    };
    let session = run(options, &[], Vec::new()).await?;
    assert_eq!(session.command.executable, OsString::from("node"));
    let mut expected = vec!["--no-warnings".to_owned(), "/cli/rec.js".to_owned()];
    expected.extend(script_tail);
    assert_eq!(arguments(&session), expected);
    assert_eq!(session.command.directory, Some(PathBuf::from("/work")));
    Ok(())
}

#[tokio::test]
async fn python_dialect_keeps_native_binaries_free_of_executable_arguments() -> TestResult {
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        cli_path: Some("/cli/claude".into()),
        script_runtime: ScriptRuntime {
            executable: None,
            arguments: vec!["--no-warnings".into()],
        },
        ..ClaudeAgentOptions::default()
    };
    let session = run(options, &[], Vec::new()).await?;
    assert_ne!(arguments(&session).first().map(String::as_str), Some("--no-warnings"));
    Ok(())
}

#[tokio::test]
async fn oauth_token_responses_match_the_bundle() -> TestResult {
    let golden = golden()?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut options = base();
    options.oauth_token_handler = Some(oauth_handler(|_| Ok(OAuthTokenResponse::token("tok")), seen.clone()));
    let session = run(
        options,
        &[],
        requests(&[
            json!({"subtype": "oauth_token_refresh"}),
            json!({"subtype": "oauth_token_refresh", "kind": "proactive", "token_state": "refused"}),
            json!({"subtype": "oauth_token_refresh", "token_state": "not_refused"}),
            json!({"subtype": "oauth_token_refresh", "token_state": "weird"}),
        ]),
    )
    .await?;
    assert_eq!(
        by_id(&session),
        golden["oauth_responses"].as_array().cloned().unwrap_or_default()
    );
    let seen = seen.lock().map_err(|_| "poisoned")?.clone();
    let expected: Vec<(Option<Value>, Option<String>)> = golden["oauth_seen"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry.get("kind").cloned(),
                entry["tokenState"].as_str().map(|state| match state {
                    "refused" => "Refused".to_owned(),
                    _ => "NotRefused".to_owned(),
                }),
            )
        })
        .collect();
    assert_eq!(seen, expected);
    Ok(())
}

#[tokio::test]
async fn oauth_decline_reasons_and_failures_match_the_bundle() -> TestResult {
    let golden = golden()?;
    let mut options = base();
    options.oauth_token_handler = Some(oauth_handler(
        |call| match call {
            1 => Ok(OAuthTokenResponse::declined(OAuthDeclineReason::SignedOut)),
            // `onDecline('bogus')` is ignored upstream, so nothing is recorded.
            2 | 5 => Ok(OAuthTokenResponse::default()),
            3 => Ok(OAuthTokenResponse::declined(OAuthDeclineReason::RefreshFailed)),
            4 => Ok(OAuthTokenResponse {
                access_token: Some("tok".into()),
                decline_reason: Some(OAuthDeclineReason::Transient),
            }),
            _ => Err(CallbackError::Failed("boom".into())),
        },
        Arc::default(),
    ));
    // Sequential requests keep the handler call order equal to the request order.
    let mut collected = Vec::new();
    for index in 0..6 {
        let session = run(
            ClaudeAgentOptions {
                oauth_token_handler: options.oauth_token_handler.clone(),
                ..base()
            },
            &[],
            vec![json!({"request_id": format!("r{index}"), "request": {"subtype": "oauth_token_refresh"}})],
        )
        .await?;
        collected.extend(session.responses);
    }
    let expected: Vec<Value> = golden["oauth_decline_responses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(collected, expected);
    Ok(())
}

#[tokio::test]
async fn host_auth_token_responses_match_the_bundle() -> TestResult {
    let golden = golden()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let (counter, recorded) = (calls.clone(), statuses.clone());
    let mut options = base();
    options.host_auth_token_handler = Some(Arc::new(move |request: HostAuthTokenRequest| {
        let call = counter.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut recorded) = recorded.lock() {
            recorded.push(request.status.clone());
        }
        async move {
            match call {
                1 => Ok(Some("h".to_owned())),
                2..=4 => Ok(None),
                _ => Err(CallbackError::Failed("hboom".into())),
            }
        }
    }));
    let mut collected = Vec::new();
    // Upstream also forwards a non-string return unchanged; a typed handler cannot return one, so r3 is skipped.
    for (index, request) in [
        json!({"subtype": "host_auth_token_refresh", "status": "expired"}),
        json!({"subtype": "host_auth_token_refresh"}),
        json!({"subtype": "host_auth_token_refresh"}),
        json!({"subtype": "host_auth_token_refresh", "status": {"a": 1}}),
        json!({"subtype": "host_auth_token_refresh"}),
    ]
    .into_iter()
    .enumerate()
    {
        let session = run(
            ClaudeAgentOptions {
                host_auth_token_handler: options.host_auth_token_handler.clone(),
                ..base()
            },
            &[],
            vec![json!({"request_id": format!("r{index}"), "request": request})],
        )
        .await?;
        collected.extend(session.responses);
    }
    let expected = golden["host_responses"].as_array().cloned().unwrap_or_default();
    for index in [0, 1, 2, 4] {
        assert_eq!(collected[index], expected[index], "r{index}");
    }
    let seen = statuses.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(seen[0], Some(json!("expired")));
    assert_eq!(seen[3], Some(json!({"a": 1})));
    Ok(())
}

#[tokio::test]
async fn missing_callbacks_answer_with_the_bundle_errors() -> TestResult {
    let golden = golden()?;
    let session = run(
        base(),
        &[],
        requests(&[
            json!({"subtype": "oauth_token_refresh"}),
            json!({"subtype": "host_auth_token_refresh"}),
        ]),
    )
    .await?;
    assert_eq!(
        by_id(&session),
        golden["missing_callback_responses"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    );
    Ok(())
}

#[tokio::test]
async fn a_cancel_request_cancels_the_token_callback_signal() -> TestResult {
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let mut options = base();
    options.oauth_token_handler = Some(Arc::new(move |request: OAuthTokenRequest| {
        let tx = tx.clone();
        // Observe the signal from a detached task so the check does not depend on the handler finishing.
        let signal = request.signal.clone();
        tokio::spawn(async move {
            signal.cancelled().await;
            let _ = tx.send(());
        });
        async move {
            request.signal.cancelled().await;
            Err(CallbackError::Cancelled)
        }
    }));
    let (process, harness) = ScriptedProcessSpawner::new();
    let observer = process.observer();
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(harness.stdin).lines();
        let mut stdout = harness.stdout;
        let stderr = harness.stderr;
        while let Some(line) = lines.next_line().await? {
            let frame: Value = serde_json::from_str(&line)?;
            if frame["request"]["subtype"] == "initialize" {
                for value in [
                    json!({"type":"control_response","response":{"subtype":"success","request_id":frame["request_id"],"response":{}}}),
                    json!({"type":"control_request","request_id":"r0","request":{"subtype":"oauth_token_refresh"}}),
                    json!({"type":"control_cancel_request","request_id":"r0"}),
                ] {
                    let mut bytes = serde_json::to_vec(&value)?;
                    bytes.push(b'\n');
                    stdout.write_all(&bytes).await?;
                    // Let the handler start; cancelling aborts a task that has not been polled yet.
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
            }
        }
        drop(stdout);
        drop(stderr);
        harness.observer.complete(0);
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
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
    let cancelled = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await;
    assert!(matches!(cancelled, Ok(Some(()))), "the signal was not cancelled");
    client.disconnect().await?;
    server.await?.map_err(|error| error.to_string())?;
    drop(observer);
    Ok(())
}

#[tokio::test]
async fn a_repeated_delivery_of_an_in_flight_token_request_is_dropped() -> TestResult {
    let oauth_calls = Arc::new(AtomicUsize::new(0));
    let host_calls = Arc::new(AtomicUsize::new(0));
    let (oauth_counter, host_counter) = (oauth_calls.clone(), host_calls.clone());
    let mut options = base();
    options.oauth_token_handler = Some(Arc::new(move |_: OAuthTokenRequest| {
        oauth_counter.fetch_add(1, Ordering::SeqCst);
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(OAuthTokenResponse::token("t"))
        }
    }));
    options.host_auth_token_handler = Some(Arc::new(move |_: HostAuthTokenRequest| {
        host_counter.fetch_add(1, Ordering::SeqCst);
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(Some("h".to_owned()))
        }
    }));
    let oauth = json!({"request_id": "o", "request": {"subtype": "oauth_token_refresh"}});
    let host = json!({"request_id": "h", "request": {"subtype": "host_auth_token_refresh"}});
    let session = run(options, &[], vec![oauth.clone(), oauth, host.clone(), host]).await?;
    assert_eq!(session.responses.len(), 2);
    assert_eq!(oauth_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host_calls.load(Ordering::SeqCst), 1);
    Ok(())
}
