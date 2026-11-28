mod support;

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use serde_json::json;
use support::{
    MockSandboxBackend, TestResult, WAIT, api_key, call, call_ok, lease_request, open_socket, read_next, test_state,
};
use tokio_tungstenite::tungstenite::Message;
use trogon_sandbox_ws_gateway::{parse_cors_origins, router};

#[tokio::test]
async fn a_ping_is_answered_with_a_pong_and_binary_frames_are_ignored() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("ping"), "tenant-test").await?;
    ws.send(Message::Binary(vec![1, 2, 3].into())).await?;
    ws.send(Message::Ping(vec![9].into())).await?;
    loop {
        let message = tokio::time::timeout(WAIT, ws.next()).await?.ok_or("socket closed")??;
        if let Message::Pong(payload) = message {
            assert_eq!(payload.as_ref(), &[9]);
            return Ok(());
        }
    }
}

#[tokio::test]
async fn a_frame_that_is_not_a_request_is_refused() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("frames"), "tenant-test").await?;
    let event = json!({"kind": "event", "stream_id": "s", "event": {}});
    ws.send(Message::Text(event.to_string().into())).await?;
    let frame = read_next(&mut ws).await?;
    assert_eq!(frame["error"]["code"], "invalid_frame");
    assert_eq!(frame["id"], "unknown");
    Ok(())
}

#[tokio::test]
async fn a_close_frame_ends_the_connection() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("close"), "tenant-test").await?;
    ws.send(Message::Close(None)).await?;
    while tokio::time::timeout(WAIT, ws.next())
        .await?
        .is_some_and(|message| message.is_ok())
    {}
    Ok(())
}

#[tokio::test]
async fn lease_suspension_entities_and_the_dashboard_subscription_work_through_the_mock() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("misc"), "tenant-test").await?;
    let lease = call_ok(&mut ws, "sandbox_lease", lease_request("tenant-test")).await?;
    let lease_id = lease["lease_id"].clone();
    let suspended = call_ok(
        &mut ws,
        "sandbox_suspend_lease",
        json!({"lease_id": lease_id, "level": "disk"}),
    )
    .await?;
    assert_eq!(suspended["outcome"], "suspended");
    let unreadable = call(
        &mut ws,
        "sandbox_read_file",
        json!({"lease_id": lease_id, "path": "/unreadable"}),
    )
    .await?;
    assert!(unreadable["error"]["code"].is_string(), "{unreadable}");
    let started = call_ok(
        &mut ws,
        "sandbox_run_command",
        json!({"lease_id": lease_id, "command": ["sleep", "1"], "detached": true, "process": {"name": "web"}}),
    )
    .await?;
    assert_eq!(started["outcome"], "started");
    let entity = call_ok(&mut ws, "sandbox_get_entity", json!({"name": "web"})).await?;
    assert_eq!(entity["outcome"], "not_found");
    let by_id = call_ok(
        &mut ws,
        "sandbox_get_entity",
        json!({"entity_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"}),
    )
    .await?;
    assert_eq!(by_id["outcome"], "not_found");
    let completed = call_ok(
        &mut ws,
        "sandbox_run_command",
        json!({"lease_id": lease_id, "command": ["true"]}),
    )
    .await?;
    assert_eq!(completed["outcome"], "completed");
    let subscribed = call_ok(&mut ws, "sandbox_subscribe_dashboard", json!({})).await?;
    assert_eq!(subscribed["subscribed"], true);
    Ok(())
}

#[tokio::test]
async fn configured_cors_origins_are_allowed() -> TestResult {
    let key = api_key("cors");
    let state = test_state(&key, "tenant-test", Arc::new(MockSandboxBackend::new())).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = router(state, parse_cors_origins(&["https://app.example".to_string()])?);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let response = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, format!("http://{addr}/v1/sessions"))
        .header("origin", "https://app.example")
        .header("access-control-request-method", "POST")
        .send()
        .await?;
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .map(|value| value.as_bytes()),
        Some("https://app.example".as_bytes())
    );
    Ok(())
}

#[test]
fn an_origin_that_is_not_a_header_value_is_refused() {
    let refused = parse_cors_origins(&["https://app.example".to_string(), "not an origin\n".to_string()]);
    assert!(refused.is_err_and(|error| error.to_string().contains("not an origin")));
}
