#![cfg(all(unix, feature = "test-support"))]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Golden tests pin exact protocol frames through a scripted child process."
    )
)]

//! Pins what the SDK writes, and when, relative to the CLI answering `initialize`. The expected order was
//! captured from `sdk.mjs` 0.3.323 with `getOAuthToken` and `getHostAuthToken` set: the SDK writes `initialize`
//! first, answers the CLI's refresh requests as soon as they arrive, even before `initialize` is answered, and
//! keeps answering after it.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream},
    time::Instant,
};
use trogon_claude_agent_sdk::{
    CallbackError, ClaudeAgentOptions, ClaudeSdkClient, HostAuthToken, HostAuthTokenRequest, OAuthTokenRequest,
    OAuthTokenResponse,
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

const CAPTURED: &str = include_str!("fixtures/auth_callbacks/auth_callbacks.json");
/// The delay the capture put between the CLI's early requests and its `initialize` answer.
const INITIALIZE_DELAY: Duration = Duration::from_millis(300);
type Trace = Arc<Mutex<Vec<String>>>;

fn note(trace: &Trace, event: String) {
    if let Ok(mut trace) = trace.lock() {
        trace.push(event);
    }
}

async fn send(stdout: &mut DuplexStream, trace: &Trace, id: &str, request: Value) -> Result<(), std::io::Error> {
    note(
        trace,
        format!("cli sends {} {id}", request["subtype"].as_str().unwrap_or_default()),
    );
    let mut line = serde_json::to_vec(&json!({"type":"control_request","request_id":id,"request":request}))?;
    line.push(b'\n');
    stdout.write_all(&line).await
}

/// Sends two refresh requests when `initialize` arrives, answers it after the delay and sends a third request
/// right after, reading everything the SDK writes meanwhile.
async fn serve(process: ScriptedProcessHarness, trace: Trace) -> Result<(), std::io::Error> {
    let mut lines = BufReader::new(process.stdin).lines();
    let mut stdout = process.stdout;
    let stderr = process.stderr;
    let mut initialize_id = None;
    let mut answer_at = None;
    loop {
        let due = async {
            match answer_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let frame: Value = serde_json::from_str(&line)?;
                let subtype = frame["request"]["subtype"].as_str().unwrap_or_default();
                if frame["type"] == "control_request" {
                    note(&trace, format!("sdk writes {subtype} request"));
                    if subtype == "initialize" {
                        initialize_id = Some(frame["request_id"].clone());
                        send(&mut stdout, &trace, "e1", json!({"subtype":"oauth_token_refresh"})).await?;
                        send(&mut stdout, &trace, "e2", json!({"subtype":"host_auth_token_refresh","status":"s"})).await?;
                        answer_at = Some(Instant::now() + INITIALIZE_DELAY);
                    }
                } else {
                    note(&trace, format!(
                        "sdk writes {} response {}",
                        frame["response"]["subtype"].as_str().unwrap_or_default(),
                        frame["response"]["request_id"].as_str().unwrap_or_default()
                    ));
                }
            }
            () = due => {
                answer_at = None;
                let mut line = serde_json::to_vec(&json!({"type":"control_response","response":{
                    "subtype":"success","request_id":initialize_id,"response":{}}}))?;
                line.push(b'\n');
                stdout.write_all(&line).await?;
                note(&trace, "cli answers initialize".to_owned());
                send(&mut stdout, &trace, "e3", json!({"subtype":"oauth_token_refresh"})).await?;
            }
        }
    }
    drop(stdout);
    drop(stderr);
    process.observer.complete(0);
    Ok(())
}

#[tokio::test]
async fn refresh_requests_are_answered_around_the_initialize_answer_like_sdk_mjs() -> TestResult {
    let (process, harness) = ScriptedProcessSpawner::new();
    let trace = Trace::default();
    let server = tokio::spawn(serve(harness, Arc::clone(&trace)));
    let mut options = ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        ..ClaudeAgentOptions::default()
    };
    options.oauth_token_handler = Some(Arc::new(|_: OAuthTokenRequest| async {
        Ok::<_, CallbackError>(OAuthTokenResponse::token("tok"))
    }));
    options.host_auth_token_handler = Some(Arc::new(|_: HostAuthTokenRequest| async {
        Ok::<_, CallbackError>(HostAuthToken::Token("ht".to_owned()))
    }));
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
    let expected = Captured::parse(CAPTURED)?.strings("order_inbound_before_init", "order")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while trace.lock().map_err(|_| "lock poisoned")?.len() < expected.len() {
        if Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    client.disconnect().await?;
    server.await??;
    assert_eq!(*trace.lock().map_err(|_| "lock poisoned")?, expected);
    Ok(())
}
