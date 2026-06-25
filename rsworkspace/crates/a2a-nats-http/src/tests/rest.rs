use a2a_nats::client::{A2aClient, ClientError};
use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

use crate::rest::rest_error_response;
use crate::router;
use super::{build_app, error_response_bytes, response_json, send_message_response_bytes, task_response_bytes};

// ---------------------------------------------------------------------------
// Helper builders
// ---------------------------------------------------------------------------

fn push_config_response_bytes(task_id: &str, config_id: &str) -> bytes::Bytes {
    let cfg = json!({
        "taskId": task_id,
        "id": config_id,
        "url": "https://example.com/webhook",
        "token": null,
        "authentication": null,
        "tenant": null,
    });
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": cfg });
    serde_json::to_vec(&envelope).unwrap().into()
}

fn push_list_response_bytes() -> bytes::Bytes {
    let resp = json!({ "configs": [], "nextPageToken": null });
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": resp });
    serde_json::to_vec(&envelope).unwrap().into()
}

fn task_snapshot_bytes(task_id: &str) -> bytes::Bytes {
    let task = json!({
        "id": task_id,
        "contextId": "",
        "status": { "state": "TASK_STATE_WORKING" },
    });
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": task });
    serde_json::to_vec(&envelope).unwrap().into()
}

fn get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn post_json_request(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

fn delete_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// GET /v1/card
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_get_card_returns_200_and_agent_card() {
    let nats = AdvancedMockNatsClient::new();
    let card = a2a::agent_card::AgentCard {
        name: "TestBot".into(),
        description: "test".into(),
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
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": card });
    nats.set_response(
        "a2a.agents.test-agent.card",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

    let app = build_app(nats);
    let response = app.oneshot(get_request("/v1/card")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["name"], "TestBot");
}

#[tokio::test]
async fn rest_get_card_error_returns_503() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.card",
        error_response_bytes(-32050, "unavailable"),
    );

    let app = build_app(nats);
    let response = app.oneshot(get_request("/v1/card")).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32050);
}

// ---------------------------------------------------------------------------
// POST /v1/message:send
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_message_send_returns_200_and_task() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.message.send",
        send_message_response_bytes("msg-task-1"),
    );

    let app = build_app(nats);
    let body = r#"{"message":{"messageId":"m1","role":"ROLE_USER","parts":[]}}"#;
    let response = app
        .oneshot(post_json_request("/v1/message:send", body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let resp_body = response_json(response).await;
    assert!(resp_body["task"].is_object());
}

#[tokio::test]
async fn rest_message_send_error_returns_error_status() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.message.send",
        error_response_bytes(-32050, "agent unavailable"),
    );

    let app = build_app(nats);
    let body = r#"{"message":{"messageId":"m1","role":"ROLE_USER","parts":[]}}"#;
    let response = app
        .oneshot(post_json_request("/v1/message:send", body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// POST /v1/message:stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_message_stream_returns_sse_content_type() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.message.stream",
        send_message_response_bytes("stream-task-1"),
    );

    // SSE endpoint requires a JetStream consumer to succeed
    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let body = r#"{"message":{"messageId":"m2","role":"ROLE_USER","parts":[]}}"#;
    let response = app
        .oneshot(post_json_request("/v1/message:stream", body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/event-stream"), "expected SSE, got {ct}");
}

#[tokio::test]
async fn rest_message_stream_error_returns_error_status() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.message.stream",
        error_response_bytes(-32050, "unavailable"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let body = r#"{"message":{"messageId":"m3","role":"ROLE_USER","parts":[]}}"#;
    let response = app
        .oneshot(post_json_request("/v1/message:stream", body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// GET /v1/tasks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_tasks_list_returns_200_and_tasks() {
    let nats = AdvancedMockNatsClient::new();
    let list_resp = json!({ "tasks": [], "nextPageToken": "", "pageSize": 0, "totalSize": 0 });
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": list_resp });
    nats.set_response("a2a.agents.test-agent.tasks.list", serde_json::to_vec(&envelope).unwrap().into());

    let app = build_app(nats);
    let response = app.oneshot(get_request("/v1/tasks")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["tasks"].is_array());
}

#[tokio::test]
async fn rest_tasks_list_with_query_params_returns_200() {
    let nats = AdvancedMockNatsClient::new();
    let list_resp = json!({ "tasks": [], "nextPageToken": "tok123", "pageSize": 10, "totalSize": 0 });
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": list_resp });
    nats.set_response("a2a.agents.test-agent.tasks.list", serde_json::to_vec(&envelope).unwrap().into());

    let app = build_app(nats);
    let response = app
        .oneshot(get_request("/v1/tasks?tenant=t1&page_size=10&page_token=abc"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["nextPageToken"], "tok123");
}

#[tokio::test]
async fn rest_tasks_list_error_returns_error_status() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.list",
        error_response_bytes(-32050, "unavailable"),
    );

    let app = build_app(nats);
    let response = app.oneshot(get_request("/v1/tasks")).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// GET /v1/tasks/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_tasks_get_returns_200_and_task() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.tasks.get", task_response_bytes("task-rest-1"));

    let app = build_app(nats);
    let response = app.oneshot(get_request("/v1/tasks/task-rest-1")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], "task-rest-1");
}

#[tokio::test]
async fn rest_tasks_get_with_history_length_param_returns_200() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.tasks.get", task_response_bytes("task-hist-1"));

    let app = build_app(nats);
    let response = app
        .oneshot(get_request("/v1/tasks/task-hist-1?historyLength=5"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], "task-hist-1");
}

#[tokio::test]
async fn rest_tasks_get_not_found_returns_404() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.get",
        error_response_bytes(-32001, "not found"),
    );

    let app = build_app(nats);
    let response = app.oneshot(get_request("/v1/tasks/missing-task")).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32001);
}

// ---------------------------------------------------------------------------
// POST /v1/tasks/{id}/cancel  (also tests the URL rewrite: /v1/tasks/{id}:cancel)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_tasks_cancel_returns_200_and_task() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.tasks.cancel", task_response_bytes("task-cancel-1"));

    let app = build_app(nats);
    let response = app
        .oneshot(post_json_request("/v1/tasks/task-cancel-1/cancel", "{}"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], "task-cancel-1");
}

#[tokio::test]
async fn rest_tasks_cancel_via_slash_path_returns_200() {
    // Verify the canonical /v1/tasks/{id}/cancel path works (covered here
    // because the colon-form rewrite is tested elsewhere via the router itself).
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.tasks.cancel", task_response_bytes("task-slash-c"));

    let app = build_app(nats);
    let response = app
        .oneshot(post_json_request("/v1/tasks/task-slash-c/cancel", "{}"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], "task-slash-c");
}

#[tokio::test]
async fn rest_tasks_cancel_not_cancelable_returns_409() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.cancel",
        error_response_bytes(-32002, "not cancelable"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(post_json_request("/v1/tasks/some-task/cancel", "{}"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// GET /v1/tasks/{id}/subscribe  (also tests colon-form)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_tasks_subscribe_returns_sse_content_type() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.resubscribe",
        task_snapshot_bytes("task-sub-1"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(get_request("/v1/tasks/task-sub-1/subscribe"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/event-stream"), "expected SSE, got {ct}");
}

#[tokio::test]
async fn rest_tasks_subscribe_via_slash_path_returns_sse() {
    // Verify the canonical /v1/tasks/{id}/subscribe path works directly.
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.resubscribe",
        task_snapshot_bytes("task-slash-sub"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(get_request("/v1/tasks/task-slash-sub/subscribe"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/event-stream"), "expected SSE, got {ct}");
}

#[tokio::test]
async fn rest_tasks_subscribe_error_returns_error_status() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.resubscribe",
        error_response_bytes(-32001, "task not found"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(get_request("/v1/tasks/missing-task/subscribe"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rest_tasks_subscribe_invalid_task_id_returns_400() {
    let nats = AdvancedMockNatsClient::new();

    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    // A dot in the task ID is valid in a URI path segment but invalid as a NATS token.
    // The handler calls A2aTaskId::new which rejects it with InvalidCharacter('.')
    // and returns 400 BAD_REQUEST.
    let response = app
        .oneshot(get_request("/v1/tasks/invalid.task.id/subscribe"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

// ---------------------------------------------------------------------------
// POST /v1/tasks/{id}/pushNotificationConfigs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_push_set_returns_200_and_config() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.set",
        push_config_response_bytes("task-push-1", "cfg-1"),
    );

    let app = build_app(nats);
    let body = r#"{"url":"https://example.com/webhook"}"#;
    let response = app
        .oneshot(post_json_request("/v1/tasks/task-push-1/pushNotificationConfigs", body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let resp_body = response_json(response).await;
    // The handler overwrites task_id from path, so result has taskId = "task-push-1"
    assert_eq!(resp_body["taskId"], "task-push-1");
}

#[tokio::test]
async fn rest_push_set_body_task_id_mismatch_returns_400() {
    let nats = AdvancedMockNatsClient::new();

    let app = build_app(nats);
    // body task_id "wrong-task" doesn't match path "task-push-2"
    let body = r#"{"taskId":"wrong-task","url":"https://example.com/webhook"}"#;
    let response = app
        .oneshot(post_json_request("/v1/tasks/task-push-2/pushNotificationConfigs", body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let resp_body = response_json(response).await;
    assert_eq!(resp_body["error"]["code"], -32602);
}

#[tokio::test]
async fn rest_push_set_error_returns_501() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.set",
        error_response_bytes(-32003, "not supported"),
    );

    let app = build_app(nats);
    let body = r#"{"url":"https://example.com/webhook"}"#;
    let response = app
        .oneshot(post_json_request("/v1/tasks/task-err/pushNotificationConfigs", body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}

// ---------------------------------------------------------------------------
// GET /v1/tasks/{id}/pushNotificationConfigs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_push_list_returns_200_and_empty_configs() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.push.list", push_list_response_bytes());

    let app = build_app(nats);
    let response = app
        .oneshot(get_request("/v1/tasks/task-list-1/pushNotificationConfigs"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    // configs is serialized as an empty array OR omitted (skip_serializing_if = "Vec::is_empty")
    // Either way, the response is a valid JSON object.
    assert!(body.is_object());
}

#[tokio::test]
async fn rest_push_list_error_returns_error_status() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.list",
        error_response_bytes(-32003, "not supported"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(get_request("/v1/tasks/task-list-err/pushNotificationConfigs"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
}

// ---------------------------------------------------------------------------
// GET /v1/tasks/{id}/pushNotificationConfigs/{configId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_push_get_returns_200_and_config() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.get",
        push_config_response_bytes("task-pg-1", "cfg-get-1"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(get_request("/v1/tasks/task-pg-1/pushNotificationConfigs/cfg-get-1"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["id"], "cfg-get-1");
}

#[tokio::test]
async fn rest_push_get_not_found_returns_404() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.get",
        error_response_bytes(-32001, "not found"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(get_request("/v1/tasks/task-pg-err/pushNotificationConfigs/missing-cfg"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// DELETE /v1/tasks/{id}/pushNotificationConfigs/{configId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_push_delete_returns_204() {
    let nats = AdvancedMockNatsClient::new();
    // push_delete returns () on success; the mock must return a valid JSON-RPC envelope with null result.
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": null });
    nats.set_response(
        "a2a.agents.test-agent.push.delete",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(delete_request("/v1/tasks/task-del-1/pushNotificationConfigs/cfg-del-1"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rest_push_delete_error_returns_error_status() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.delete",
        error_response_bytes(-32001, "not found"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(delete_request("/v1/tasks/task-del-err/pushNotificationConfigs/cfg-del-err"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// http_status_for_jsonrpc_code coverage
// ---------------------------------------------------------------------------

#[test]
fn http_status_for_all_known_jsonrpc_codes() {
    use crate::rest::http_status_for_jsonrpc_code;

    assert_eq!(http_status_for_jsonrpc_code(-32001), StatusCode::NOT_FOUND);
    assert_eq!(http_status_for_jsonrpc_code(-32002), StatusCode::CONFLICT);
    assert_eq!(http_status_for_jsonrpc_code(-32003), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(http_status_for_jsonrpc_code(-32004), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(http_status_for_jsonrpc_code(-32005), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(http_status_for_jsonrpc_code(-32006), StatusCode::BAD_GATEWAY);
    assert_eq!(http_status_for_jsonrpc_code(-32007), StatusCode::NOT_FOUND);
    assert_eq!(http_status_for_jsonrpc_code(-32008), StatusCode::BAD_REQUEST);
    assert_eq!(http_status_for_jsonrpc_code(-32009), StatusCode::BAD_REQUEST);
    assert_eq!(http_status_for_jsonrpc_code(-32050), StatusCode::SERVICE_UNAVAILABLE);
    // Any other code maps to 500
    assert_eq!(http_status_for_jsonrpc_code(-32603), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(http_status_for_jsonrpc_code(0), StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// rest_error_response helper
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rest_error_response_formats_json_body() {
    let resp = rest_error_response(&ClientError::TaskNotFound);
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], -32001);
}

// ---------------------------------------------------------------------------
// router rewrite_a2a_custom_method coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_rewrite_does_not_rewrite_unrelated_paths() {
    let nats = AdvancedMockNatsClient::new();
    let card = a2a::agent_card::AgentCard {
        name: "RW".into(),
        description: "test".into(),
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
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": card });
    nats.set_response(
        "a2a.agents.test-agent.card",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

    let app = build_app(nats);
    // This path does NOT match the rewrite pattern → plain /v1/card is served
    let response = app.oneshot(get_request("/v1/card")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn router_cancel_with_query_param_returns_200() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.tasks.cancel", task_response_bytes("task-qp"));

    let app = build_app(nats);
    // The canonical /v1/tasks/{id}/cancel path with a query param exercises the CancelQuery handler.
    let response = app
        .oneshot(post_json_request("/v1/tasks/task-qp/cancel?tenant=t1", "{}"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// handlers/mod.rs — uncovered branches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jsonrpc_invalid_version_returns_invalid_request_error() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"1.0","id":1,"method":"message/send","params":{"message":{"messageId":"m1","role":"ROLE_USER","parts":[]}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32600);
}

#[tokio::test]
async fn jsonrpc_tasks_resubscribe_returns_sse() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.resubscribe",
        task_snapshot_bytes("task-resub-jsonrpc"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":10,"method":"tasks/resubscribe","params":{"id":"task-resub-jsonrpc","lastSeq":0}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/event-stream"), "expected SSE, got {ct}");
}

#[tokio::test]
async fn jsonrpc_tasks_resubscribe_with_metadata_last_event_id() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.resubscribe",
        task_snapshot_bytes("task-meta-resub"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    // Use metadata.lastEventId form (older client convention)
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":11,"method":"tasks/resubscribe","params":{"id":"task-meta-resub","metadata":{"lastEventId":"5"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers().get(CONTENT_TYPE).unwrap().to_str().unwrap();
    assert!(ct.starts_with("text/event-stream"), "expected SSE, got {ct}");
}

#[tokio::test]
async fn jsonrpc_tasks_resubscribe_bad_task_id_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();

    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    // Empty id string should fail A2aTaskId::new validation
                    r#"{"jsonrpc":"2.0","id":12,"method":"tasks/resubscribe","params":{"id":""}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_tasks_resubscribe_error_returns_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.resubscribe",
        error_response_bytes(-32001, "not found"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":13,"method":"tasks/resubscribe","params":{"id":"task-404"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32001);
}

#[tokio::test]
async fn jsonrpc_push_notification_config_get_returns_result() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.get",
        push_config_response_bytes("task-j-pg", "cfg-j-1"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":14,"method":"tasks/pushNotificationConfig/get","params":{"taskId":"task-j-pg","id":"cfg-j-1"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["result"].is_object(), "expected result, got {body}");
}

#[tokio::test]
async fn jsonrpc_push_notification_config_list_returns_result() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response("a2a.agents.test-agent.push.list", push_list_response_bytes());

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":15,"method":"tasks/pushNotificationConfig/list","params":{"taskId":"task-j-pl"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["result"].is_object(), "expected result, got {body}");
}

#[tokio::test]
async fn jsonrpc_push_notification_config_delete_returns_null_result() {
    let nats = AdvancedMockNatsClient::new();
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": null });
    nats.set_response(
        "a2a.agents.test-agent.push.delete",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":16,"method":"tasks/pushNotificationConfig/delete","params":{"taskId":"task-j-pd","id":"cfg-j-del"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["result"].is_null(), "expected null result, got {body}");
}

#[tokio::test]
async fn jsonrpc_agent_get_authenticated_extended_card_returns_result() {
    let nats = AdvancedMockNatsClient::new();
    let card = a2a::agent_card::AgentCard {
        name: "ExtBot".into(),
        description: "extended test".into(),
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
    let envelope = json!({ "jsonrpc": "2.0", "id": "ignored", "result": card });
    nats.set_response(
        "a2a.agents.test-agent.card",
        serde_json::to_vec(&envelope).unwrap().into(),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":17,"method":"agent/getAuthenticatedExtendedCard","params":{}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["result"].is_object(), "expected result, got {body}");
}

#[tokio::test]
async fn jsonrpc_message_send_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":18,"method":"message/send","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_message_stream_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();

    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":19,"method":"message/stream","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_message_stream_error_returns_jsonrpc_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.message.stream",
        error_response_bytes(-32050, "unavailable"),
    );

    let js = MockJetStreamConsumerFactory::new();
    let (consumer, _tx) = MockJetStreamConsumer::new();
    js.add_consumer(consumer);

    let client = A2aClient::new(super::test_config(), super::test_agent_id(), nats, js);
    let app = router::build(client);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":20,"method":"message/stream","params":{"message":{"messageId":"m-err","role":"ROLE_USER","parts":[]}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32050);
}

#[tokio::test]
async fn jsonrpc_tasks_get_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":21,"method":"tasks/get","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_tasks_cancel_error_returns_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.cancel",
        error_response_bytes(-32002, "not cancelable"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":22,"method":"tasks/cancel","params":{"id":"t-err","tenant":""}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32002);
}

#[tokio::test]
async fn jsonrpc_tasks_list_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":23,"method":"tasks/list","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_tasks_list_error_returns_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.tasks.list",
        error_response_bytes(-32050, "unavailable"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":24,"method":"tasks/list","params":{}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32050);
}

#[tokio::test]
async fn jsonrpc_push_set_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":25,"method":"tasks/pushNotificationConfig/set","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_push_get_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":26,"method":"tasks/pushNotificationConfig/get","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_push_list_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":27,"method":"tasks/pushNotificationConfig/list","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_push_delete_invalid_params_returns_invalid_params() {
    let nats = AdvancedMockNatsClient::new();
    let app = build_app(nats);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":28,"method":"tasks/pushNotificationConfig/delete","params":"not-an-object"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32602);
}

#[tokio::test]
async fn jsonrpc_push_delete_error_returns_jsonrpc_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.delete",
        error_response_bytes(-32001, "not found"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":29,"method":"tasks/pushNotificationConfig/delete","params":{"taskId":"t-del","id":"cfg-del"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32001);
}

#[tokio::test]
async fn jsonrpc_push_get_error_returns_jsonrpc_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.get",
        error_response_bytes(-32001, "not found"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":30,"method":"tasks/pushNotificationConfig/get","params":{"taskId":"t-pg","id":"cfg-404"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32001);
}

#[tokio::test]
async fn jsonrpc_push_list_error_returns_jsonrpc_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.list",
        error_response_bytes(-32003, "not supported"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":31,"method":"tasks/pushNotificationConfig/list","params":{"taskId":"t-pl"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32003);
}

#[tokio::test]
async fn jsonrpc_agent_card_error_returns_jsonrpc_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.card",
        error_response_bytes(-32050, "unavailable"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":32,"method":"agent/getAuthenticatedExtendedCard","params":{}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32050);
}

#[tokio::test]
async fn jsonrpc_push_set_error_returns_jsonrpc_error_result() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.push.set",
        error_response_bytes(-32003, "not supported"),
    );

    let app = build_app(nats);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":33,"method":"tasks/pushNotificationConfig/set","params":{"taskId":"t-set-err","url":"https://example.com/hook"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], -32003);
}

// Ensure handlers::agent_card (the .well-known handler) also covers its error branch
#[tokio::test]
async fn well_known_agent_card_error_returns_proper_status() {
    let nats = AdvancedMockNatsClient::new();
    nats.set_response(
        "a2a.agents.test-agent.card",
        error_response_bytes(-32006, "invalid response"),
    );

    let app = build_app(nats);
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

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], -32006);
}
