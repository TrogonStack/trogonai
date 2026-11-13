#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests exercise exact upstream JSON fixtures.")
)]

//! `browser_query` against goldens recorded by running the browser entrypoint's `query()` over a scripted
//! WebSocket (`browser_query_goldens.mjs`), and against a local SSE server for the query accessors.

use std::{error::Error as StdError, sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt, stream};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc, oneshot},
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message as Frame};
use trogon_claude_agent_sdk::{
    BrowserQueryOptions, BrowserTransport, DialogKind, ElicitationHandler, ElicitationRequest, ElicitationResponse,
    Error, HookEvent, HookHandler, HookOutput, HookRegistration, HookRequest, JsonObject, LocalMcpServer,
    LocalMcpServerOptions, McpServerDialect, McpServerName, Message, OutputSchema, PermissionDecision, Prompt,
    SdkMcpServer, SdkTools, SseOptions, Timeout, ToolPermissionHandler, ToolPermissionRequest, UserDialogHandler,
    UserDialogRequest, UserDialogResponse, UserInput, WebSocketOptions, browser_query, get_ccr_event,
    get_sse_drop_counts, get_sse_last_sequence_num, resume_sse_stream, suspend_sse_stream,
};

type TestResult = Result<(), Box<dyn StdError>>;

const FIXTURE: &str = include_str!("fixtures/browser_query/cases.json");
const WAIT: Duration = Duration::from_secs(5);

fn line(value: &Value) -> Frame {
    Frame::text(format!("{value}\n"))
}

fn result_message() -> Value {
    json!({
        "type": "result", "subtype": "success", "is_error": false, "duration_ms": 1, "duration_api_ms": 1,
        "num_turns": 1, "session_id": "remote-session", "result": "done", "total_cost_usd": 0.0,
    })
}

/// A fake CLI over WebSocket: answers control requests, answers a user message with a result, and reports every
/// document it receives. With `hang_up` it closes the socket once it answered the first control request.
async fn fake_cli(listener: TcpListener, seen: mpsc::UnboundedSender<Value>, hang_up: bool) {
    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let Ok(mut socket) = accept_async(stream).await else {
        return;
    };
    while let Some(Ok(Frame::Text(text))) = socket.next().await {
        for document in text.lines().filter(|line| !line.is_empty()) {
            let Ok(value) = serde_json::from_str::<Value>(document) else {
                continue;
            };
            let _ = seen.send(value.clone());
            match value["type"].as_str() {
                Some("control_request") => {
                    let answer = json!({
                        "type": "control_response",
                        "response": {"subtype": "success", "request_id": value["request_id"], "response": {}},
                    });
                    let _ = socket.send(line(&answer)).await;
                    if hang_up {
                        let _ = socket.close(None).await;
                        return;
                    }
                }
                Some("user") => {
                    let _ = socket.send(line(&result_message())).await;
                }
                _ => {}
            }
        }
    }
}

async fn websocket_cli(hang_up: bool) -> Result<(String, mpsc::UnboundedReceiver<Value>), Box<dyn StdError>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}/", listener.local_addr()?);
    let (seen_tx, seen) = mpsc::unbounded_channel();
    tokio::spawn(fake_cli(listener, seen_tx, hang_up));
    Ok((url, seen))
}

fn permission() -> Arc<dyn ToolPermissionHandler> {
    Arc::new(|_: ToolPermissionRequest| async { PermissionDecision::allow() })
}

fn elicitation() -> Arc<dyn ElicitationHandler> {
    Arc::new(|_: ElicitationRequest| async { Ok(ElicitationResponse::Cancel) })
}

fn dialog() -> Arc<dyn UserDialogHandler> {
    Arc::new(|_: UserDialogRequest| async { Ok(UserDialogResponse::Cancelled) })
}

fn hook() -> Arc<dyn HookHandler> {
    Arc::new(|_: HookRequest| async { Ok(HookOutput::Async { timeout: None }) })
}

fn registration(
    event: HookEvent,
    matcher: Option<&str>,
    callbacks: usize,
    seconds: Option<u64>,
) -> Result<HookRegistration, Box<dyn StdError>> {
    Ok(HookRegistration {
        event,
        matcher: matcher.map(str::to_owned),
        callbacks: (0..callbacks).map(|_| hook()).collect(),
        timeout: seconds
            .map(|seconds| Timeout::checked(Duration::from_secs(seconds)))
            .transpose()?,
        on_failure: None,
    })
}

fn server(name: &str) -> Arc<dyn SdkMcpServer> {
    Arc::new(LocalMcpServer::from(LocalMcpServerOptions {
        name: McpServerName::from(name),
        version: None,
        tools: Some(SdkTools::default()),
        instructions: None,
        always_load: false,
        dialect: McpServerDialect::TypeScript,
    }))
}

fn schema(value: Value) -> Result<OutputSchema, Box<dyn StdError>> {
    Ok(OutputSchema::new(serde_json::from_value::<JsonObject>(value)?))
}

fn kinds(names: &[&str]) -> Option<Vec<DialogKind>> {
    Some(names.iter().map(|name| DialogKind::from(*name)).collect())
}

fn idle_options(url: &str) -> BrowserQueryOptions {
    BrowserQueryOptions::new(
        stream::pending::<UserInput>(),
        BrowserTransport::WebSocket(WebSocketOptions::new(url)),
    )
}

/// The options of the recorded case called `name`; the last case is `everything`.
fn recorded_options(name: &str, url: &str) -> Result<BrowserQueryOptions, Box<dyn StdError>> {
    let mut options = idle_options(url);
    match name {
        "bare" => {}
        "prompt suggestions on" => options.prompt_suggestions = Some(true),
        "prompt suggestions off" => options.prompt_suggestions = Some(false),
        "dialog kinds" => {
            options.supported_dialog_kinds = kinds(&["plan", "ask"]);
            options.user_dialog_handler = Some(dialog());
        }
        "empty dialog kinds" => options.supported_dialog_kinds = kinds(&[]),
        "dialog handler only" => options.user_dialog_handler = Some(dialog()),
        "json schema" => {
            options.json_schema = Some(schema(
                json!({"type": "object", "properties": {"answer": {"type": "string"}}}),
            )?);
        }
        "sdk servers" => {
            options
                .sdk_mcp_servers
                .insert(McpServerName::from("calc"), server("calc"));
            options
                .sdk_mcp_servers
                .insert(McpServerName::from("math"), server("math"));
        }
        "hooks" => {
            options.hooks = vec![
                registration(HookEvent::PreToolUse, Some("Bash"), 2, Some(7))?,
                registration(HookEvent::PreToolUse, None, 1, None)?,
                registration(HookEvent::Stop, None, 1, None)?,
            ];
        }
        "callbacks" => {
            options.permission_handler = Some(permission());
            options.elicitation_handler = Some(elicitation());
        }
        _ => {
            options.prompt_suggestions = Some(true);
            options.supported_dialog_kinds = kinds(&["plan"]);
            options.user_dialog_handler = Some(dialog());
            options.json_schema = Some(schema(json!({"type": "object"}))?);
            options
                .sdk_mcp_servers
                .insert(McpServerName::from("calc"), server("calc"));
            options.hooks = vec![registration(HookEvent::Stop, None, 1, None)?];
            options.permission_handler = Some(permission());
            options.elicitation_handler = Some(elicitation());
        }
    }
    Ok(options)
}

#[tokio::test]
async fn every_recorded_query_sends_the_recorded_initialize_request() -> TestResult {
    let golden: Value = serde_json::from_str(FIXTURE)?;
    let cases = golden["queries"].as_array().ok_or("queries")?;
    assert_eq!(cases.len(), 11);
    for case in cases {
        let name = case["name"].as_str().ok_or("name")?;
        let (url, mut seen) = websocket_cli(false).await?;
        assert_eq!(case["urls"], json!(["ws://127.0.0.1:1/"]), "{name}");
        let query = timeout(WAIT, browser_query(recorded_options(name, &url)?)).await??;
        let mut request = seen.try_recv()?;
        request["request_id"] = json!("REQUEST");
        assert_eq!(
            serde_json::to_string(&request)?,
            serde_json::to_string(&case["frames"][0])?,
            "{name}"
        );
        assert!(seen.try_recv().is_err(), "{name}");
        assert_eq!(case["frames"].as_array().map(Vec::len), Some(1), "{name}");
        query.close().await?;
    }
    Ok(())
}

#[tokio::test]
async fn dialog_kinds_without_a_handler_fail_before_connecting_with_the_recorded_text() -> TestResult {
    let golden: Value = serde_json::from_str(FIXTURE)?;
    let recorded = &golden["errors"][0];
    assert_eq!(recorded["name"], "dialog kinds without a handler");
    assert_eq!(
        (recorded["websockets"].as_u64(), recorded["fetches"].as_u64()),
        (Some(0), Some(0))
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let mut options = idle_options(&format!("ws://{}/", listener.local_addr()?));
    options.supported_dialog_kinds = kinds(&["plan"]);
    let error = browser_query(options).await.err().ok_or("expected an error")?;
    assert!(matches!(error, Error::DialogKindsWithoutHandler));
    assert_eq!(
        format!("Error: {error}"),
        recorded["error"].as_str().ok_or("error text")?
    );
    assert!(timeout(Duration::from_millis(100), listener.accept()).await.is_err());
    Ok(())
}

#[tokio::test]
async fn the_recorded_transport_errors_stay_unrepresentable_or_have_no_rust_counterpart() -> TestResult {
    let golden: Value = serde_json::from_str(FIXTURE)?;
    let names: Vec<&str> = golden["errors"]
        .as_array()
        .ok_or("errors")?
        .iter()
        .filter_map(|case| case["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "dialog kinds without a handler",
            "a server the browser cannot host",
            "an sdk type without an instance",
            "neither transport",
            "both transports",
        ]
    );
    assert_eq!(golden["errors"][4]["fetches"], 1);
    Ok(())
}

#[tokio::test]
async fn a_websocket_query_has_no_sse_state() -> TestResult {
    let (url, _seen) = websocket_cli(false).await?;
    let mut options = BrowserQueryOptions::new(
        stream::iter([UserInput::from(Prompt::from("hi"))]),
        BrowserTransport::WebSocket(WebSocketOptions::new(url)),
    );
    options.abort = Some(trogon_claude_agent_sdk::CancellationToken::new());
    let rendered = format!("{options:?}");
    assert!(rendered.contains("BrowserQueryOptions") && rendered.contains("hooks: 0"));
    let mut query = timeout(WAIT, browser_query(options)).await??;
    let message = timeout(WAIT, query.next_message()).await??.ok_or("a message")?;
    assert!(matches!(message, Message::Result(_)));
    assert_eq!(get_sse_drop_counts(&query), None);
    assert_eq!(get_sse_last_sequence_num(&query), None);
    assert_eq!(get_ccr_event(&query, &message), None);
    assert!(!suspend_sse_stream(&query));
    assert!(!resume_sse_stream(&query));
    query.close().await?;
    Ok(())
}

#[tokio::test]
async fn a_prompt_that_cannot_be_sent_aborts_the_query() -> TestResult {
    let (url, _seen) = websocket_cli(true).await?;
    let (release, released) = oneshot::channel::<()>();
    let prompt = stream::once(async move {
        let _ = released.await;
        UserInput::from(Prompt::from("late"))
    });
    let token = trogon_claude_agent_sdk::CancellationToken::new();
    let mut options = BrowserQueryOptions::new(prompt, BrowserTransport::WebSocket(WebSocketOptions::new(url)));
    options.abort = Some(token.clone());
    let mut query = timeout(WAIT, browser_query(options)).await??;
    let _ended = timeout(WAIT, query.next_message()).await?;
    release.send(()).map_err(|()| "the prompt stream is gone")?;
    timeout(WAIT, token.cancelled()).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// SSE

async fn read_request(socket: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(chunk.get(..read)?);
    };
    let head = String::from_utf8_lossy(buffer.get(..end)?).to_lowercase();
    let method = head.split_whitespace().next()?.to_owned();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < end + 4 + length {
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(chunk.get(..read)?);
    }
    Some((method, buffer.get(end + 4..)?.to_vec()))
}

fn client_event(sequence_num: u64, event_id: &str, payload: &Value) -> String {
    let data = json!({"event_id": event_id, "sequence_num": sequence_num, "source": "worker", "payload": payload});
    format!("event: client_event\nid: {sequence_num}\ndata: {data}\n\n")
}

type Events = Arc<Mutex<Option<mpsc::UnboundedReceiver<String>>>>;

/// Answers the stream request with the events pushed to `events`, and every POST with 200: a control request is
/// answered with a control response, a user message with a malformed delivery update and a result.
async fn sse_connection(mut socket: TcpStream, pushed: mpsc::UnboundedSender<String>, events: Events) {
    while let Some((method, body)) = read_request(&mut socket).await {
        if method == "POST" {
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let payload = &body["events"][0]["payload"];
            match payload["type"].as_str() {
                Some("control_request") => {
                    let answer = json!({
                        "type": "control_response",
                        "response": {"subtype": "success", "request_id": payload["request_id"], "response": {}},
                    });
                    let _ = pushed.send(client_event(1, "e1", &answer));
                }
                Some("user") => {
                    let _ = pushed.send("event: delivery_update\ndata: {\"event_id\":5}\n\n".to_owned());
                    let _ = pushed.send(client_event(2, "e2", &result_message()));
                }
                _ => {}
            }
            if socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            continue;
        }
        let Some(mut receiver) = events.lock().await.take() else {
            std::future::pending::<()>().await;
            return;
        };
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
        if socket.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        while let Some(text) = receiver.recv().await {
            let chunk = format!("{:x}\r\n{text}\r\n", text.len());
            if socket.write_all(chunk.as_bytes()).await.is_err() {
                return;
            }
        }
        return;
    }
}

async fn sse_origin() -> Result<String, Box<dyn StdError>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let (pushed, receiver) = mpsc::unbounded_channel();
    let events: Events = Arc::new(Mutex::new(Some(receiver)));
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(sse_connection(socket, pushed.clone(), Arc::clone(&events)));
        }
    });
    Ok(origin)
}

#[tokio::test]
async fn an_sse_query_exposes_the_transport_state_through_the_accessors() -> TestResult {
    let origin = sse_origin().await?;
    let options = BrowserQueryOptions::new(
        stream::iter([UserInput::from(Prompt::from("hi"))]),
        BrowserTransport::Sse(SseOptions::new(
            format!("{origin}/stream"),
            format!("{origin}/send"),
            "session",
        )),
    );
    let mut query = timeout(WAIT, browser_query(options)).await??;
    assert_eq!(get_sse_last_sequence_num(&query), Some(1));
    let message = timeout(WAIT, query.next_message()).await??.ok_or("a message")?;
    assert!(matches!(message, Message::Result(_)));
    let event = get_ccr_event(&query, &message).ok_or("an envelope")?;
    assert_eq!(
        (event.event_id.as_str(), event.sequence_num, event.source.as_deref()),
        ("e2", 2, Some("worker"))
    );
    assert_eq!(get_sse_last_sequence_num(&query), Some(2));
    let counts = get_sse_drop_counts(&query).ok_or("counts")?;
    assert_eq!(counts.get("malformed_delivery_update"), Some(&1));
    assert!(suspend_sse_stream(&query));
    assert!(!suspend_sse_stream(&query));
    assert!(resume_sse_stream(&query));
    assert!(!resume_sse_stream(&query));
    query.close().await?;
    Ok(())
}
