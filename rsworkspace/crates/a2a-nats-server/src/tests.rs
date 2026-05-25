use std::time::Duration;

use a2a_auth_callout::test_support::mint_test_user_jwt;
use a2a_nats::client::Client;
use a2a_nats::{A2aAgentId, Config, NatsConfig};
use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory;

use crate::router;

fn test_config() -> Config {
    Config::new(
        a2a_nats::A2aPrefix::new("a2a".to_string()).unwrap(),
        NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        },
    )
}

fn test_agent_id() -> A2aAgentId {
    A2aAgentId::new("test-agent").unwrap()
}

fn gateway_test_caller_jwt() -> a2a_nats::client::MintedUserJwt {
    mint_test_user_jwt("test-agent", "a2a", Duration::from_secs(3600))
}

fn build_app(nats: AdvancedMockNatsClient) -> axum::Router {
    let js = MockJetStreamConsumerFactory::new();
    let client = Client::new(test_config(), test_agent_id(), nats, js);
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

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn task_response_bytes(task_id: &str) -> bytes::Bytes {
    let task = json!({ "id": task_id, "status": { "state": 3 } });
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": task });
    serde_json::to_vec(&envelope).unwrap().into()
}

fn send_message_response_bytes(task_id: &str) -> bytes::Bytes {
    let task = json!({ "id": task_id, "status": { "state": 1 } });
    let resp = json!({ "task": task });
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": resp });
    serde_json::to_vec(&envelope).unwrap().into()
}

fn error_response_bytes(code: i32, msg: &str) -> bytes::Bytes {
    let envelope = json!({
        "jsonrpc": "2.0",
        "id": "ignored",
        "error": { "code": code, "message": msg }
    });
    serde_json::to_vec(&envelope).unwrap().into()
}

#[tokio::test]
async fn message_send_returns_jsonrpc_result() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agent.test-agent.message.send", send_message_response_bytes("t1"));

    let app = build_app(nats);
    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"messageId":"m1","role":1,"parts":[]}}}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], 1);
    assert!(body["result"].is_object());
}

#[tokio::test]
async fn tasks_get_returns_jsonrpc_result() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agent.test-agent.tasks.get", task_response_bytes("task-1"));

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
    nats.set_response(
        "a2a.agent.test-agent.tasks.get",
        error_response_bytes(-32001, "not found"),
    );

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
    nats.set_response("a2a.agent.test-agent.tasks.cancel", task_response_bytes("task-c"));

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
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": list_resp });
    nats.set_response(
        "a2a.agent.test-agent.tasks.list",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

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
    assert!(body["result"].is_object());
}

#[tokio::test]
async fn agent_card_endpoint_returns_json() {
    let nats = AdvancedMockNatsClient::new();
    let card = a2a_types::AgentCard {
        name: "TestBot".into(),
        description: "A test agent".into(),
        version: "1.0.0".into(),
        capabilities: Some(a2a_types::AgentCapabilities::default()),
        ..Default::default()
    };
    let envelope = serde_json::json!({ "jsonrpc": "2.0", "id": "ignored", "result": card });
    nats.set_response(
        "a2a.agent.test-agent.agent.card",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

    let js = MockJetStreamConsumerFactory::new();
    let client = Client::new(test_config(), test_agent_id(), nats, js);
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
    nats.set_response(
        "a2a.agent.test-agent.message.stream",
        send_message_response_bytes("ts1"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = trogon_nats::jetstream::mocks::MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = Client::new(test_config(), test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":8,"method":"message/stream","params":{"message":{"messageId":"m2","role":1,"parts":[]}}}"#,
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
    let envelope = json!({
        "jsonrpc": "2.0",
        "id": "ignored",
        "error": { "code": -32050, "message": "unavailable" }
    });
    nats.set_response(
        "a2a.agent.test-agent.agent.card",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

    let js = MockJetStreamConsumerFactory::new();
    let client = Client::new(test_config(), test_agent_id(), nats, js);
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
    nats.set_response("a2a.agent.test-agent.tasks.get", task_response_bytes("t99"));

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
    nats.set_response(
        "a2a.agent.test-agent.tasks.push_notification_config.set",
        error_response_bytes(-32003, "not supported"),
    );

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
    use crate::runtime::RuntimeError;
    assert!(RuntimeError::MissingAgentId.to_string().contains("A2A_AGENT_ID"));
}

#[tokio::test]
async fn gateway_routed_message_send_targets_gateway_subject() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.gateway.test-agent.message.send",
        send_message_response_bytes("t-gw"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let client = Client::new(test_config(), test_agent_id(), nats, js)
        .routing_via_gateway_ingress(gateway_test_caller_jwt());
    let app = router::build(client);

    let response = app
        .oneshot(jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"messageId":"m1","role":1,"parts":[]}}}"#,
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
    nats.set_response(
        "a2a.agent.test-agent.tasks.get",
        task_response_bytes("should-not-be-hit"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let client = Client::new(test_config(), test_agent_id(), nats, js)
        .routing_via_gateway_ingress(gateway_test_caller_jwt());
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
    use crate::sse::client_error_to_jsonrpc_code;
    use a2a_nats::client::ClientError;

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
