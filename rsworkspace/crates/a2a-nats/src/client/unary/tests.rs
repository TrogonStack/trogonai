use std::time::Duration;

use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use serde::{Deserialize, Serialize};
use trogon_nats::AdvancedMockNatsClient;

use a2a_identity_types::MintedUserJwt;

use super::*;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Params {
    x: i32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Response {
    y: String,
}

fn req_id() -> ReqId {
    ReqId::from_test("test-req-1")
}

fn wire_success_response(y: &str) -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&Message::Success {
        id: ResponseId::String("test-req-1".into()),
        result: serde_json::json!({ "y": y }),
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

fn wire_error_response(code: i32, message: &str) -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&Message::Error {
        id: ResponseId::String("test-req-1".into()),
        code,
        message: message.to_string(),
        data: None,
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

#[tokio::test]
async fn success_response_deserializes_result() {
    let mock = AdvancedMockNatsClient::new();
    let (headers, body) = wire_success_response("hello");
    mock.set_response_wire("a2a.agents.bot.tasks.get", headers, body);

    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.agents.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        None,
    )
    .await;

    assert_eq!(result.unwrap().y, "hello");
}

#[tokio::test]
async fn task_not_found_error_code_maps_to_typed_error() {
    let mock = AdvancedMockNatsClient::new();
    let (headers, body) = wire_error_response(-32001, "Task not found");
    mock.set_response_wire("a2a.agents.bot.tasks.get", headers, body);

    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.agents.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        None,
    )
    .await;

    assert!(matches!(result, Err(ClientError::TaskNotFound)));
}

#[tokio::test]
async fn agent_unavailable_code_maps_to_typed_error() {
    let mock = AdvancedMockNatsClient::new();
    let (headers, body) = wire_error_response(-32050, "no responders");
    mock.set_response_wire("a2a.agents.bot.tasks.get", headers, body);

    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.agents.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        None,
    )
    .await;

    assert!(matches!(result, Err(ClientError::AgentUnavailable)));
}

#[tokio::test]
async fn transport_failure_returns_transport_error() {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_request();

    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.agents.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        None,
    )
    .await;

    assert!(matches!(result, Err(ClientError::Transport(_))));
}

#[tokio::test]
async fn hang_returns_timeout_error() {
    let mock = AdvancedMockNatsClient::new();
    mock.hang_next_request();

    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.agents.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_millis(10),
        None,
    )
    .await;

    assert!(matches!(result, Err(ClientError::Timeout { .. })));
}

#[tokio::test]
async fn malformed_response_returns_deserialize_error() {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response_wire("a2a.agents.bot.tasks.get", async_nats::HeaderMap::new(), Bytes::from_static(b"not json at all"));

    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.agents.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        None,
    )
    .await;

    assert!(matches!(result, Err(ClientError::Deserialize(_))));
}

#[tokio::test]
async fn unknown_error_code_maps_to_generic_jsonrpc_error() {
    let mock = AdvancedMockNatsClient::new();
    let (headers, body) = wire_error_response(-32099, "custom");
    mock.set_response_wire("a2a.agents.bot.tasks.get", headers, body);

    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.agents.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        None,
    )
    .await;

    assert!(matches!(result, Err(ClientError::JsonRpc { code: -32099, .. })));
}

#[tokio::test]
async fn gateway_jwt_attaches_caller_jwt_header() {
    let mock = AdvancedMockNatsClient::new();
    let (headers, body) = wire_success_response("ok");
    mock.set_response_wire("a2a.gateway.bot.tasks.get", headers, body);

    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjk5OTk5OTk5OTl9.signature").unwrap();
    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.gateway.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        Some(&jwt),
    )
    .await;

    assert_eq!(result.unwrap().y, "ok");
}

#[tokio::test]
async fn gateway_expired_jwt_returns_expired_error() {
    let mock = AdvancedMockNatsClient::new();
    let jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjF9.signature").unwrap();
    let result: Result<Response, _> = send_unary(
        &mock,
        "a2a.gateway.bot.tasks.get",
        "tasks/get",
        &Params { x: 1 },
        &req_id(),
        Duration::from_secs(5),
        Some(&jwt),
    )
    .await;

    assert!(matches!(result, Err(ClientError::GatewayCallerJwtExpired(_))));
}
