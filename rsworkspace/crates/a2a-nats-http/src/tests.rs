use std::time::{Duration, SystemTime, UNIX_EPOCH};

use a2a_identity_types::MintedUserJwt;
use a2a_nats::client::A2aClient;
use a2a_nats::{A2aAgentId, A2aPrefix};
use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use base64::Engine;
use bytes::Bytes;
use jsonrpc_nats::{Message, ResponseId, encode};
use serde_json::{Value, json};
use tower::ServiceExt;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory;

use crate::router;
use crate::runtime::RuntimeError;
use crate::sse::client_error_to_jsonrpc_code;
use a2a_nats::client::ClientError;

pub(super) fn test_config() -> A2aPrefix {
    A2aPrefix::new("a2a".to_string()).unwrap()
}

pub(super) fn test_agent_id() -> A2aAgentId {
    A2aAgentId::new("test-agent").unwrap()
}

fn mint_test_user_jwt(_agent_id: &str, _prefix: &str, ttl: Duration) -> MintedUserJwt {
    // The HTTP runtime only inspects shape + `exp`; minting via the real auth-callout
    // crate would pull in jsonwebtoken/nkeys here, so we hand-craft a token with a
    // future `exp` and an opaque signature segment that validate_compact_jwt_shape
    // accepts.
    let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + ttl.as_secs();
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"test-signature");
    MintedUserJwt::new(format!("{header}.{payload}.{signature}")).unwrap()
}

fn gateway_test_caller_jwt() -> MintedUserJwt {
    mint_test_user_jwt("test-agent", "a2a", Duration::from_secs(3600))
}

pub(super) fn build_app(nats: AdvancedMockNatsClient) -> axum::Router {
    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(test_config(), test_agent_id(), nats, js);
    router::build(client)
}

fn jsonrpc_request(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

pub(super) async fn response_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) fn task_response_bytes(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
    let task = json!({ "id": task_id, "contextId": "", "status": { "state": "TASK_STATE_COMPLETED" } });
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: task,
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

pub(super) fn send_message_response_bytes(task_id: &str) -> (async_nats::HeaderMap, Bytes) {
    let task = json!({ "id": task_id, "contextId": "", "status": { "state": "TASK_STATE_SUBMITTED" } });
    let resp = json!({ "task": task });
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: resp,
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

pub(super) fn error_response_bytes(code: i32, msg: &str) -> (async_nats::HeaderMap, Bytes) {
    let encoded = encode(&Message::Error {
        id: ResponseId::String("any".into()),
        code,
        message: msg.to_string(),
        data: None,
    })
    .unwrap();
    (encoded.headers, encoded.body)
}

#[tokio::test]
async fn message_send_returns_jsonrpc_result() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response_bytes("t1");
    nats.set_response_wire("a2a.agents.test-agent.message.send", headers, body);

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"messageId":"m1","role":"ROLE_USER","parts":[]}}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], 1);
    eprintln!("RESPONSE: {body}");
    assert!(body["result"].is_object());
}

#[tokio::test]
async fn tasks_get_returns_jsonrpc_result() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response_bytes("task-1");
    nats.set_response_wire("a2a.agents.test-agent.tasks.get", headers, body);

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"tasks/get","params":{"id":"task-1","tenant":""}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], 2);
    assert_eq!(body["result"]["id"], "task-1");
}

#[tokio::test]
async fn tasks_get_not_found_returns_jsonrpc_error() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response_bytes(-32001, "not found");
    nats.set_response_wire("a2a.agents.test-agent.tasks.get", headers, body);

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":3,"method":"tasks/get","params":{"id":"missing","tenant":""}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], 3);
    assert_eq!(body["error"]["code"], -32001);
}

#[tokio::test]
async fn tasks_cancel_returns_jsonrpc_result() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response_bytes("task-c");
    nats.set_response_wire("a2a.agents.test-agent.tasks.cancel", headers, body);

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":4,"method":"tasks/cancel","params":{"id":"task-c","tenant":""}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], 4);
    eprintln!("RESPONSE: {body}");
    assert!(body["result"].is_object());
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":5,"method":"no/such/method","params":{}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32601);
}

#[tokio::test]
async fn invalid_params_returns_invalid_params_error() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":6,"method":"tasks/get","params":"not-an-object"}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn tasks_list_returns_jsonrpc_result() {
    let nats = AdvancedMockNatsClient::new();
    let list_resp = json!({
        "tasks": [],
        "nextPageToken": "",
        "pageSize": 0,
        "totalSize": 0
    });
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: list_resp,
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.test-agent.tasks.list", encoded.headers, encoded.body);

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":7,"method":"tasks/list","params":{}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], 7);
    eprintln!("RESPONSE: {body}");
    assert!(body["result"].is_object());
}

#[tokio::test]
async fn agent_card_endpoint_returns_json() {
    let nats = AdvancedMockNatsClient::new();
    let card = a2a::agent_card::AgentCard {
        name: "TestBot".into(),
        description: "A test agent".into(),
        version: "1.0.0".into(),
        supported_interfaces: vec![],
        capabilities: a2a::agent_card::AgentCapabilities::default(),
        default_input_modes: vec![],
        default_output_modes: vec![],
        skills: vec![],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    };
    let encoded = encode(&Message::Success {
        id: ResponseId::String("any".into()),
        result: serde_json::json!(card),
    })
    .unwrap();
    nats.set_response_wire("a2a.agents.test-agent.card", encoded.headers, encoded.body);

    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(test_config(), test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/agent-card.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body.is_object());
}

#[tokio::test]
async fn message_stream_returns_sse_content_type() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response_bytes("ts1");
    nats.set_response_wire("a2a.agents.test-agent.message.stream", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = trogon_nats::jetstream::mocks::MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(test_config(), test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":8,"method":"message/stream","params":{"message":{"messageId":"m2","role":"ROLE_USER","parts":[]}}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/event-stream"), "expected SSE, got {ct}");
}

#[tokio::test]
async fn agent_card_unavailable_returns_503() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response_bytes(-32050, "unavailable");
    nats.set_response_wire("a2a.agents.test-agent.card", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(test_config(), test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/agent-card.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn jsonrpc_string_id_is_forwarded() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response_bytes("t99");
    nats.set_response_wire("a2a.agents.test-agent.tasks.get", headers, body);

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":"my-string-id","method":"tasks/get","params":{"id":"t99","tenant":""}}"#,
        ))
        .await
        .unwrap();

    let body = response_json(response).await;
    assert_eq!(body["id"], "my-string-id");
}

#[tokio::test]
async fn push_set_not_supported_maps_to_correct_error_code() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = error_response_bytes(-32003, "not supported");
    nats.set_response_wire("a2a.agents.test-agent.push.set", headers, body);

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":9,"method":"tasks/pushNotificationConfig/set","params":{"taskId":"t1","url":"https://example.com/hook"}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32003);
}

#[test]
fn runtime_error_display_shows_env_var_name() {
    assert_eq!(
        RuntimeError::MissingAgentId.to_string(),
        "A2A_AGENT_ID environment variable is required"
    );
}

#[tokio::test]
async fn gateway_routed_message_send_targets_gateway_subject() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response_bytes("t-gw");
    nats.set_response_wire("a2a.gateway.test-agent.message.send", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let client =
        A2aClient::new(test_config(), test_agent_id(), nats, js).routing_via_gateway_ingress(gateway_test_caller_jwt());
    let app = router::build(client);

    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"messageId":"m1","role":"ROLE_USER","parts":[]}}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["result"].is_object(), "expected result, got {body}");
}

#[tokio::test]
async fn agent_routed_subject_unanswered_when_gateway_routing_enabled() {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = task_response_bytes("should-not-be-hit");
    nats.set_response_wire("a2a.agents.test-agent.tasks.get", headers, body);

    let js = MockJetStreamConsumerFactory::new();
    let client =
        A2aClient::new(test_config(), test_agent_id(), nats, js).routing_via_gateway_ingress(gateway_test_caller_jwt());
    let app = router::build(client);

    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":3,"method":"tasks/get","params":{"id":"x","tenant":""}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(
        body.get("error").is_some(),
        "agent-routed mock should not satisfy gateway-routed request; got {body}"
    );
}

#[test]
fn client_error_to_jsonrpc_code_maps_known_errors() {
    assert_eq!(client_error_to_jsonrpc_code(&ClientError::TaskNotFound).0, -32001);
    assert_eq!(client_error_to_jsonrpc_code(&ClientError::TaskNotCancelable).0, -32002);
    assert_eq!(
        client_error_to_jsonrpc_code(&ClientError::PushNotificationNotSupported).0,
        -32003
    );
    assert_eq!(client_error_to_jsonrpc_code(&ClientError::AgentUnavailable).0, -32050);
    assert_eq!(
        client_error_to_jsonrpc_code(&ClientError::JsonRpc {
            code: -32099,
            message: "x".into()
        })
        .0,
        -32099
    );
    assert_eq!(client_error_to_jsonrpc_code(&ClientError::StreamClosed).0, -32603);
}

mod rest_tests;
mod spec_negotiation_tests;
