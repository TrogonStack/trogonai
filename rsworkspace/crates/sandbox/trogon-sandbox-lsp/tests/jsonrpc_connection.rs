mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use support::{FakeTransport, TestResult, eventually};
use trogon_sandbox_lsp::{ConfigurationProvider, JsonRpcConnection, LspError, LspFramer, encode_message};

fn connection(transport: &Arc<FakeTransport>) -> Arc<JsonRpcConnection> {
    JsonRpcConnection::new(transport.clone(), Duration::from_secs(30), None)
}

fn feed(conn: &JsonRpcConnection, message: &Value) -> TestResult {
    let mut framer = LspFramer::new();
    for body in framer.push(&encode_message(message)?)? {
        conn.handle_body(&body)?;
    }
    Ok(())
}

fn spawn_request(
    conn: &Arc<JsonRpcConnection>,
    method: &'static str,
    params: Option<Value>,
) -> tokio::task::JoinHandle<Result<Value, LspError>> {
    let conn = Arc::clone(conn);
    tokio::spawn(async move { conn.request(method, params, None).await })
}

#[tokio::test]
async fn matches_responses_to_requests_with_counter_ids_from_one() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let first = spawn_request(&conn, "a", Some(json!({ "x": 1 })));
    eventually(|| transport.written().len() == 1).await?;
    let second = spawn_request(&conn, "b", None);
    eventually(|| transport.written().len() == 2).await?;

    let written = transport.written();
    assert_eq!(written[0]["id"], 1);
    assert_eq!(written[0]["params"], json!({ "x": 1 }));
    assert_eq!(written[1]["id"], 2);
    assert!(written[1].get("params").is_none());

    feed(&conn, &json!({ "jsonrpc": "2.0", "id": 2, "result": "two" }))?;
    feed(&conn, &json!({ "jsonrpc": "2.0", "id": 1, "result": { "ok": true } }))?;
    assert_eq!(second.await??, json!("two"));
    assert_eq!(first.await??, json!({ "ok": true }));
    Ok(())
}

#[tokio::test]
async fn a_response_without_result_or_error_is_a_protocol_error_and_unknown_ids_are_ignored() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let request = spawn_request(&conn, "a", None);
    eventually(|| transport.written().len() == 1).await?;
    feed(&conn, &json!({ "jsonrpc": "2.0", "id": 99, "result": 1 }))?;
    feed(&conn, &json!({ "jsonrpc": "2.0", "id": "text", "result": 1 }))?;
    feed(&conn, &json!({ "jsonrpc": "2.0", "id": 1 }))?;
    let err = request.await?.err().ok_or("request succeeded")?;
    assert!(matches!(err, LspError::Protocol(_)));
    Ok(())
}

#[tokio::test]
async fn error_responses_carry_code_message_and_data() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let request = spawn_request(&conn, "a", None);
    eventually(|| transport.written().len() == 1).await?;
    feed(
        &conn,
        &json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32603, "message": "boom", "data": { "k": 1 } } }),
    )?;
    let err = request.await?.err().ok_or("request succeeded")?;
    assert!(matches!(
        err,
        LspError::Response { code: -32603, ref message, data: Some(ref data) }
            if message == "boom" && *data == json!({ "k": 1 })
    ));
    Ok(())
}

#[tokio::test]
async fn an_error_without_details_gets_defaults() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let request = spawn_request(&conn, "a", None);
    eventually(|| transport.written().len() == 1).await?;
    feed(&conn, &json!({ "jsonrpc": "2.0", "id": 1, "error": {} }))?;
    let err = request.await?.err().ok_or("request succeeded")?;
    assert!(matches!(err, LspError::Response { code: 0, ref message, data: None } if message.is_empty()));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_timeout_sends_cancel_request_and_ignores_the_late_response() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let err = conn
        .request("textDocument/hover", None, Some(Duration::from_millis(250)))
        .await
        .err()
        .ok_or("request succeeded")?;
    assert!(matches!(
        &err,
        LspError::RequestTimeout { method, timeout }
            if method == "textDocument/hover" && *timeout == Duration::from_millis(250)
    ));
    let written = transport.written();
    assert_eq!(written[1]["method"], "$/cancelRequest");
    assert_eq!(written[1]["params"], json!({ "id": 1 }));
    feed(&conn, &json!({ "jsonrpc": "2.0", "id": 1, "result": 1 }))?;
    Ok(())
}

#[tokio::test]
async fn close_rejects_every_pending_request() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let request = spawn_request(&conn, "a", None);
    eventually(|| transport.written().len() == 1).await?;
    conn.close(LspError::NotRunning("gone".into()));
    let err = request.await?.err().ok_or("request succeeded")?;
    assert!(matches!(err, LspError::NotRunning(m) if m == "gone"));
    Ok(())
}

#[tokio::test]
async fn a_request_whose_reply_channel_is_dropped_reports_a_closed_connection() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let request = spawn_request(&conn, "a", None);
    eventually(|| transport.written().len() == 1).await?;
    // a response for the request that carries no id of ours drops nothing; closing twice is harmless
    conn.close(LspError::ServerExited(None));
    conn.close(LspError::ServerExited(None));
    let err = request.await?.err().ok_or("request succeeded")?;
    assert!(matches!(err, LspError::ServerExited(None)));
    Ok(())
}

#[tokio::test]
async fn a_failed_send_rejects_and_forgets_the_request() -> TestResult {
    let transport = FakeTransport::new();
    transport.fail_writes();
    let conn = connection(&transport);
    let err = conn.request("a", None, None).await.err().ok_or("request succeeded")?;
    assert!(matches!(err, LspError::Transport(_)));
    // the failed request left nothing behind to reject
    conn.close(LspError::NotRunning("closed".into()));
    Ok(())
}

#[tokio::test]
async fn notify_writes_a_notification_without_an_id() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    conn.notify("initialized", Some(json!({}))).await?;
    let written = transport.written();
    assert_eq!(written[0]["method"], "initialized");
    assert!(written[0].get("id").is_none());
    assert_eq!(written[0]["jsonrpc"], "2.0");
    Ok(())
}

#[tokio::test]
async fn answers_server_requests_with_fixed_replies() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    feed(
        &conn,
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "workspace/configuration",
                 "params": { "items": [{ "section": "a" }, { "section": "b" }] } }),
    )?;
    feed(
        &conn,
        &json!({ "jsonrpc": "2.0", "id": "r", "method": "client/registerCapability", "params": {} }),
    )?;
    feed(
        &conn,
        &json!({ "jsonrpc": "2.0", "id": 9, "method": "window/workDoneProgress/create", "params": {} }),
    )?;
    feed(
        &conn,
        &json!({ "jsonrpc": "2.0", "id": 10, "method": "workspace/applyEdit", "params": {} }),
    )?;
    eventually(|| transport.written().len() == 4).await?;

    let replies = transport.written();
    let by_id = |id: Value| {
        replies
            .iter()
            .find(|r| r["id"] == id)
            .cloned()
            .ok_or("no reply with that id")
    };
    assert_eq!(by_id(json!(7))?["result"], json!([null, null]));
    assert_eq!(by_id(json!("r"))?["result"], Value::Null);
    assert_eq!(by_id(json!(9))?["result"], Value::Null);
    let unknown = by_id(json!(10))?;
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(unknown["error"]["message"], "method not found: workspace/applyEdit");
    Ok(())
}

#[tokio::test]
async fn a_malformed_configuration_request_is_refused_as_invalid_params() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let bad = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "workspace/configuration" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "workspace/configuration", "params": { "items": "x" } }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "workspace/configuration",
                 "params": { "items": [{ "section": 5 }] } }),
    ];
    for message in &bad {
        feed(&conn, message)?;
    }
    eventually(|| transport.written().len() == 3).await?;
    for reply in transport.written() {
        assert_eq!(reply["error"]["code"], -32602);
        assert!(reply.get("result").is_none());
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_reply_write_is_dropped() -> TestResult {
    let transport = FakeTransport::new();
    transport.fail_writes();
    let conn = connection(&transport);
    feed(
        &conn,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "client/registerCapability" }),
    )?;
    tokio::task::yield_now().await;
    assert!(transport.written().is_empty());
    Ok(())
}

#[tokio::test]
async fn configuration_provider_answers_items() -> TestResult {
    let transport = FakeTransport::new();
    let provider: ConfigurationProvider = Arc::new(|item| json!({ "section": item.section }));
    let conn = JsonRpcConnection::new(transport.clone(), Duration::from_secs(1), Some(provider));
    feed(
        &conn,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "workspace/configuration",
                 "params": { "items": [{ "section": "ts" }] } }),
    )?;
    eventually(|| transport.written().len() == 1).await?;
    assert_eq!(transport.written()[0]["result"], json!([{ "section": "ts" }]));
    Ok(())
}

#[tokio::test]
async fn notifications_reach_listeners_until_detached_and_panics_are_contained() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let hits = Arc::new(AtomicUsize::new(0));
    conn.on_notification(
        "boom",
        Arc::new(|_| std::panic::resume_unwind(Box::new("listener bug"))),
    );
    let handle = conn.on_notification("boom", {
        let hits = hits.clone();
        Arc::new(move |params| {
            if params["n"] == 1 {
                hits.fetch_add(1, Ordering::SeqCst);
            }
        })
    });
    let note = json!({ "jsonrpc": "2.0", "method": "boom", "params": { "n": 1 } });
    feed(&conn, &note)?;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    handle.detach();
    feed(&conn, &note)?;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_notification_without_params_reaches_listeners_as_null() -> TestResult {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    conn.on_notification("ping", {
        let seen = seen.clone();
        Arc::new(move |params| support::lock(&seen).push(params.clone()))
    });
    conn.on_notification("other", Arc::new(|_| {}));
    feed(&conn, &json!({ "jsonrpc": "2.0", "method": "ping" }))?;
    feed(&conn, &json!({ "jsonrpc": "2.0", "method": "ping", "id": null }))?;
    feed(&conn, &json!({ "jsonrpc": "2.0", "method": "unheard" }))?;
    assert_eq!(*support::lock(&seen), vec![Value::Null, Value::Null]);
    Ok(())
}

#[tokio::test]
async fn rejects_malformed_bodies() {
    let transport = FakeTransport::new();
    let conn = connection(&transport);
    assert!(matches!(conn.handle_body("{nope"), Err(LspError::Protocol(m)) if m.contains("JSON")));
    assert!(matches!(conn.handle_body("[1]"), Err(LspError::Protocol(m)) if m.contains("object")));
}
