use axum::body::to_bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use futures_util::StreamExt;
use trogon_nats::AdvancedMockNatsClient;
use trogon_std::env::{ReadEnv, SystemEnv};

use a2a_nats::constants::GATEWAY_CALLER_ID_HTTP;

use super::*;
use crate::inbound::handle_jsonrpc;

fn caller_headers(agent_id: &str, caller_id: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-token"),
    );
    headers.insert(
        axum::http::HeaderName::from_static("x-a2a-agent-id"),
        HeaderValue::from_str(agent_id).unwrap(),
    );
    if let Some(caller_id) = caller_id {
        headers.insert(
            axum::http::HeaderName::from_static(GATEWAY_CALLER_ID_HTTP),
            HeaderValue::from_str(caller_id).unwrap(),
        );
    }
    headers
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
}

#[tokio::test]
async fn nats_transport_callout_mint_per_request_and_jwt_caller_id() {
    let nats = AdvancedMockNatsClient::new();
    let (state, harness, mint_wire) = build_nats_transport_app_state(nats.clone(), "planner");
    assert_eq!(mint_wire.mint_count(), 0);

    let body = Bytes::from(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "message/send",
            "params": { "message": { "messageId": "m1", "role": 1, "parts": [] } }
        })
        .to_string(),
    );

    let response = handle_jsonrpc(caller_headers("planner", None), body, &state)
        .await
        .expect("message/send should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(mint_wire.mint_count(), 1);
    assert!(harness.last_caller_jwt_present());
}

#[tokio::test]
async fn nats_transport_message_send_round_trips_caller_jwt_and_audit() {
    let nats = AdvancedMockNatsClient::new();
    let (state, harness, mint_wire) = build_nats_transport_app_state(nats.clone(), "planner");
    let body = Bytes::from(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "message/send",
            "params": { "message": { "messageId": "m1", "role": 1, "parts": [] } }
        })
        .to_string(),
    );

    let response = handle_jsonrpc(caller_headers("planner", Some("caller-abc")), body, &state)
        .await
        .expect("message/send should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(mint_wire.mint_count(), 1);

    let payload = response_bytes(response).await;
    let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert!(parsed.get("result").is_some());

    assert!(harness.last_caller_jwt_present());
    assert_eq!(
        harness.last_subject().as_deref(),
        Some("a2a.gateway.planner.message.send")
    );

    let audit_subject = nats
        .published_messages()
        .into_iter()
        .find(|subject| subject.contains(".audit.ok.message.send"));
    assert!(audit_subject.is_some(), "expected gateway audit publish");
}

#[tokio::test]
async fn nats_transport_tasks_resubscribe_bootstraps_sse_stream() {
    let nats = AdvancedMockNatsClient::new();
    let (state, harness, mint_wire) = build_nats_transport_app_state(nats.clone(), "planner");
    let body = Bytes::from(
        json!({
            "jsonrpc": "2.0",
            "id": "corr-1",
            "method": "tasks/resubscribe",
            "params": { "id": "task-sse-1", "last_seq": 0 }
        })
        .to_string(),
    );

    let response = handle_jsonrpc(caller_headers("planner", Some("caller-sse")), body, &state)
        .await
        .expect("tasks/resubscribe should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(mint_wire.mint_count(), 1);
    assert_eq!(
        response.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );

    assert!(harness.last_caller_jwt_present());
    assert_eq!(
        harness.last_subject().as_deref(),
        Some("a2a.gateway.planner.tasks.resubscribe")
    );

    let mut stream = response.into_body().into_data_stream();
    let mut saw_bootstrap = false;
    let mut saw_task_event = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        let text = String::from_utf8_lossy(&chunk);
        if text.contains("gateway-bootstrap") {
            saw_bootstrap = true;
        }
        if text.contains("task-event") {
            saw_task_event = true;
        }
    }
    assert!(saw_bootstrap, "expected SSE gateway bootstrap line");
    assert!(saw_task_event, "expected SSE JetStream task event line");

    let audit_subject = nats
        .published_messages()
        .into_iter()
        .find(|subject| subject.contains(".audit.ok.tasks.resubscribe"));
    assert!(audit_subject.is_some(), "expected resubscribe audit publish");
}

/// Live NATS smoke — run with `A2A_SMOKE_COMPOSE=1 cargo test -p a2a-bridge -- --ignored nats_transport_live`.
#[tokio::test]
#[ignore = "requires compose stack: A2A_SMOKE_COMPOSE=1 and NATS_URL (see devops/docker/compose/compose.a2a.smoke.yml)"]
async fn nats_transport_live_requires_nats_server() {
    let env = SystemEnv;
    if env.var("A2A_SMOKE_COMPOSE").as_deref() != Ok("1") {
        panic!("set A2A_SMOKE_COMPOSE=1 to run live compose-network bridge smoke");
    }
    let url = env.var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let client = async_nats::connect(url).await.expect("live NATS connect");
    let _ = client;
    let bridge_addr = env
        .var("BRIDGE_LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:7443".into());
    let response = reqwest::get(format!("http://{bridge_addr}/"))
        .await
        .expect("bridge HTTP reachable");
    assert!(
        response.status().is_client_error() || response.status().is_success(),
        "bridge HTTP should respond on {bridge_addr}"
    );
}
