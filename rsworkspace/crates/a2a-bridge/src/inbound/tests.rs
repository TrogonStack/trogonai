use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use bytes::Bytes;
use futures_util::stream::{self, Stream};
use serde_json::json;

use a2a_auth_callout::CALLER_JWT_HEADER_NAME;
use a2a_nats::ReqId;
use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_CALLER_ID_HTTP, REQ_ID_HEADER};

use crate::auth::StubAuthCalloutMint;
use crate::error::BridgeError;
use crate::identity::BridgeUserJwt;

use super::*;

#[test]
fn default_a2a_prefix_constructs() {
    assert_eq!(default_a2a_prefix().as_str(), "a2a");
}

#[test]
fn gateway_method_to_subject_dots_replaces_slashes() {
    assert_eq!(gateway_method_to_subject_dots("message/stream"), "message.stream");
    assert_eq!(gateway_method_to_subject_dots("tasks/get"), "tasks.get");
}

#[test]
fn build_gateway_subject_formats_expected_subject() {
    let prefix = default_a2a_prefix();
    assert_eq!(
        build_gateway_subject(&prefix, "planner", "message/send"),
        "a2a.gateway.planner.message.send"
    );
}

#[test]
fn task_events_wild_subject_includes_prefix_and_task_id() {
    assert_eq!(
        task_events_wild_subject(&default_a2a_prefix(), "task-1"),
        "a2a.task.task-1.events.>"
    );
}

#[test]
fn is_sse_jsonrpc_method_recognizes_streaming_methods() {
    assert!(is_sse_jsonrpc_method("message/stream"));
    assert!(is_sse_jsonrpc_method("tasks/resubscribe"));
    assert!(!is_sse_jsonrpc_method("message/send"));
}

#[test]
fn extract_last_sequence_reads_canonical_and_legacy_keys() {
    let last_seq = serde_json::json!({"lastSeq": 7});
    assert_eq!(extract_last_sequence(&last_seq), Some(7));

    let metadata_last_event_id = serde_json::json!({"metadata": {"lastEventId": "42"}});
    assert_eq!(extract_last_sequence(&metadata_last_event_id), Some(42));

    let resume_string = serde_json::json!({"resume_from_sequence": "13"});
    assert_eq!(extract_last_sequence(&resume_string), Some(13));

    let missing = serde_json::json!({"unrelated": 99});
    assert_eq!(extract_last_sequence(&missing), None);
}

#[test]
fn json_rpc_corr_id_coerces_json_rpc_id_shapes() {
    assert_eq!(json_rpc_corr_id(&json!({"id": "abc"})).as_str(), "abc");
    assert_eq!(json_rpc_corr_id(&json!({"id": 42})).as_str(), "42");
    assert_eq!(json_rpc_corr_id(&json!({"id": true})).as_str(), "true");
    assert!(!json_rpc_corr_id(&json!({})).as_str().is_empty());
}

#[test]
fn gateway_reply_is_jsonrpc_error_detects_error_envelope() {
    assert!(gateway_reply_is_jsonrpc_error(
        br#"{"jsonrpc":"2.0","error":{"code":1,"message":"nope"}}"#
    ));
    assert!(!gateway_reply_is_jsonrpc_error(br#"{"jsonrpc":"2.0","result":{}}"#));
    assert!(!gateway_reply_is_jsonrpc_error(b"not-json"));
}

#[test]
fn sse_plan_builds_message_stream_and_resubscribe_plans() {
    let req_id = ReqId::from_header("corr-1");
    let stream_body = json!({"method": "message/stream", "params": {}});
    assert!(matches!(
        sse_plan("message/stream", &stream_body, req_id.clone()).unwrap(),
        SseConsumePlan::MessageStreamBootstrap { .. }
    ));

    let resub_body = json!({
        "method": "tasks/resubscribe",
        "params": { "taskId": "task-1", "lastSequence": 3 }
    });
    let plan = sse_plan("tasks/resubscribe", &resub_body, req_id).unwrap();
    assert!(matches!(plan, SseConsumePlan::TasksResubscribe { last_seq: 3, .. }));
}

#[test]
fn resub_task_and_seq_accepts_task_id_aliases() {
    let body = json!({
        "params": { "task_id": "task-alias", "last_seq": 9 }
    });
    let (task_id, last_seq) = resub_task_and_seq(&body).unwrap();
    assert_eq!(task_id.as_str(), "task-alias");
    assert_eq!(last_seq, 9);
}

#[test]
fn gateway_req_headers_propagate_correlation_and_caller_id() {
    let correlation = ReqId::from_header("req-1");
    let headers = gateway_req_headers(correlation.clone(), Some("caller-abc")).unwrap();
    assert_eq!(headers.get(REQ_ID_HEADER).unwrap().as_str(), correlation.as_str());
    assert_eq!(headers.get(GATEWAY_CALLER_ID_HEADER).unwrap().as_str(), "caller-abc");
}

#[test]
fn gateway_publish_headers_include_minted_user_jwt() {
    let jwt = BridgeUserJwt::new("eyJhbGciOiJub25lIn0.eyJzdWIiOiJoYXJuZXNzIn0.sig").unwrap();
    let headers = gateway_publish_headers(ReqId::from_header("req-2"), &jwt, None).unwrap();
    assert!(headers.get(CALLER_JWT_HEADER_NAME).is_some());
}

struct EmptyTaskJetStream;

#[async_trait]
impl TaskJetStreamPort for EmptyTaskJetStream {
    async fn task_event_payload_stream(
        &self,
        _caller_jwt: &BridgeUserJwt,
        _prefix: &A2aPrefix,
        _plan: SseConsumePlan,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes, BridgeError>> + Send>>, BridgeError> {
        Ok(Box::pin(stream::empty()))
    }
}

fn caller_headers(agent_id: &str, caller_id: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-token"),
    );
    headers.insert(
        axum::http::HeaderName::from_static(AGENT_ID_HEADER),
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

fn test_state(publisher: Arc<dyn InboundGatewayPublish>) -> AppState {
    AppState::new(
        Arc::new(StubAuthCalloutMint::fixture().unwrap()),
        publisher,
        Arc::new(EmptyTaskJetStream),
        default_a2a_prefix(),
    )
}

#[tokio::test]
async fn handle_jsonrpc_missing_authorization_errors() {
    let state = test_state(Arc::new(RecordingInboundPublisher::new()));
    let err = handle_jsonrpc(HeaderMap::new(), Bytes::new(), &state)
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::MissingAuthorization));
}

#[tokio::test]
async fn handle_jsonrpc_missing_agent_header_errors() {
    let state = test_state(Arc::new(RecordingInboundPublisher::new()));
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-token"),
    );
    let err = handle_jsonrpc(headers, Bytes::new(), &state).await.unwrap_err();
    assert!(matches!(err, BridgeError::MissingAgentHeader));
}

#[tokio::test]
async fn handle_jsonrpc_unary_publish_records_gateway_subject() {
    let publisher = Arc::new(RecordingInboundPublisher::new());
    let state = test_state(publisher.clone());
    let body = Bytes::from(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tasks/get",
            "params": { "id": "task-1" }
        })
        .to_string(),
    );
    let response = handle_jsonrpc(caller_headers("planner", None), body, &state)
        .await
        .expect("tasks/get should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        publisher.peek_subject().as_deref(),
        Some("a2a.gateway.planner.tasks.get")
    );
}

#[test]
fn json_rpc_corr_id_null_and_complex_ids_mint_fresh() {
    for body in [json!({"id": null}), json!({"id": []}), json!({"id": {}})] {
        assert!(!json_rpc_corr_id(&body).as_str().is_empty());
    }
}

#[test]
fn resub_task_and_seq_errors_on_missing_params_or_task_id() {
    assert!(matches!(
        resub_task_and_seq(&json!({})).unwrap_err(),
        BridgeError::StreamingParams(_)
    ));
    assert!(matches!(
        resub_task_and_seq(&json!({"params": {}})).unwrap_err(),
        BridgeError::StreamingParams(_)
    ));
    assert!(matches!(
        resub_task_and_seq(&json!({"params": {"id": "bad id"}})).unwrap_err(),
        BridgeError::StreamingParams(_)
    ));
}

#[test]
fn resub_task_and_seq_defaults_last_seq_to_zero() {
    let (task_id, last_seq) = resub_task_and_seq(&json!({"params": {"taskId": "task-1"}})).unwrap();
    assert_eq!(task_id.as_str(), "task-1");
    assert_eq!(last_seq, 0);
}

#[test]
fn sse_plan_rejects_unsupported_streaming_method() {
    let err = sse_plan("tasks/subscribe", &json!({}), ReqId::from_header("corr")).unwrap_err();
    assert!(matches!(err, BridgeError::StreamingParams(_)));
}

#[test]
fn gateway_req_headers_omits_caller_id_when_none() {
    let headers = gateway_req_headers(ReqId::from_header("req-1"), None).unwrap();
    assert!(headers.get(GATEWAY_CALLER_ID_HEADER).is_none());
}

#[tokio::test]
async fn handle_jsonrpc_invalid_json_returns_deserialize() {
    let state = test_state(Arc::new(RecordingInboundPublisher::new()));
    let err = handle_jsonrpc(caller_headers("planner", None), Bytes::from("not-json"), &state)
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::Deserialize(_)));
}

#[tokio::test]
async fn handle_jsonrpc_missing_method_returns_error() {
    let state = test_state(Arc::new(RecordingInboundPublisher::new()));
    let err = handle_jsonrpc(caller_headers("planner", None), Bytes::from("{}"), &state)
        .await
        .unwrap_err();
    assert!(matches!(err, BridgeError::MissingJsonRpcMethod));
}

#[tokio::test]
async fn handle_jsonrpc_invalid_agent_header_errors() {
    let state = test_state(Arc::new(RecordingInboundPublisher::new()));
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer fixture-token"),
    );
    headers.insert(
        axum::http::HeaderName::from_static(AGENT_ID_HEADER),
        HeaderValue::from_static("bad agent"),
    );
    let err = handle_jsonrpc(headers, Bytes::new(), &state).await.unwrap_err();
    assert!(matches!(err, BridgeError::InvalidAgent(_)));
}
