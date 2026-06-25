use std::sync::Arc;

use a2a_nats::client::A2aClient;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use trogon_nats::AdvancedMockNatsClient;
use trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory;

use crate::headers::{A2A_EXTENSIONS_HEADER, A2A_MEDIA_TYPE, A2A_VERSION_HEADER, SpecNegotiationConfig};
use crate::router;

use super::{send_message_response_bytes, test_agent_id, test_config};

fn build_app_with_negotiation(config: SpecNegotiationConfig) -> axum::Router {
    let nats = AdvancedMockNatsClient::new();
    let (headers, body) = send_message_response_bytes("t1");
    nats.set_response_wire("a2a.agents.test-agent.message.send", headers, body);
    let js = MockJetStreamConsumerFactory::new();
    let client = A2aClient::new(test_config(), test_agent_id(), nats, js);
    router::build_with_negotiation(client, Arc::new(config))
}

fn message_send_body() -> &'static str {
    r#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{"message":{"role":"ROLE_USER","messageId":"m1","parts":[]}}}"#
}

#[tokio::test]
async fn default_version_echoed_when_request_omits_header() {
    let app = build_app_with_negotiation(SpecNegotiationConfig::default());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .body(Body::from(message_send_body()))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let version = response
        .headers()
        .get(&A2A_VERSION_HEADER)
        .expect("a2a-version header echoed")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(version, "0.3.0");
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        content_type.starts_with(A2A_MEDIA_TYPE),
        "content-type was {content_type}"
    );
}

#[tokio::test]
async fn unknown_version_rejected_with_jsonrpc_error() {
    let app = build_app_with_negotiation(SpecNegotiationConfig::default());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(&A2A_VERSION_HEADER, "9.9.9")
        .body(Body::from(message_send_body()))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], a2a_nats::error::VERSION_NOT_SUPPORTED);
}

#[tokio::test]
async fn required_extension_mismatch_rejected_with_jsonrpc_error() {
    let app = build_app_with_negotiation(SpecNegotiationConfig::default());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(&A2A_EXTENSIONS_HEADER, "https://example.com/required-ext")
        .body(Body::from(message_send_body()))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], a2a_nats::error::EXTENSION_SUPPORT_REQUIRED);
}

#[tokio::test]
async fn optional_extension_unknown_is_ignored() {
    let app = build_app_with_negotiation(SpecNegotiationConfig::default());
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(&A2A_EXTENSIONS_HEADER, "?https://example.com/optional-ext")
        .body(Body::from(message_send_body()))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(&A2A_EXTENSIONS_HEADER).is_none());
}

#[tokio::test]
async fn supported_extension_echoed_on_response() {
    let config = SpecNegotiationConfig::default().with_extension("https://example.com/supported");
    let app = build_app_with_negotiation(config);
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(&A2A_EXTENSIONS_HEADER, "https://example.com/supported")
        .body(Body::from(message_send_body()))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let activated = response
        .headers()
        .get(&A2A_EXTENSIONS_HEADER)
        .expect("activated extension echoed")
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(activated, "https://example.com/supported");
}
