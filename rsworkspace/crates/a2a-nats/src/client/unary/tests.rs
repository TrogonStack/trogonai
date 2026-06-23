use serde::{Deserialize, Serialize};
use trogon_nats::AdvancedMockNatsClient;

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

fn success_response(y: &str) -> Bytes {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "test-req-1",
        "result": { "y": y }
    });
    serde_json::to_vec(&json).unwrap().into()
}

fn error_response(code: i32, message: &str) -> Bytes {
    let json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "test-req-1",
        "error": { "code": code, "message": message }
    });
    serde_json::to_vec(&json).unwrap().into()
}

#[tokio::test]
async fn success_response_deserializes_result() {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response("a2a.agents.bot.tasks.get", success_response("hello"));

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
    mock.set_response("a2a.agents.bot.tasks.get", error_response(-32001, "Task not found"));

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
    mock.set_response("a2a.agents.bot.tasks.get", error_response(-32050, "no responders"));

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
    mock.set_response("a2a.agents.bot.tasks.get", Bytes::from_static(b"not json at all"));

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
    mock.set_response("a2a.agents.bot.tasks.get", error_response(-32099, "custom"));

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
    mock.set_response("a2a.gateway.bot.tasks.get", success_response("ok"));

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
