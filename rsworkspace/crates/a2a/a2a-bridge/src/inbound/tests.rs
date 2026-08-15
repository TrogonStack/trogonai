use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
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

/// Bootstrap replies are built from the SDK response type rather than hand-written
/// JSON: `SendMessageResponse` nests the payload under a variant key, and a fixture
/// that spells the shape by hand can drift from the wire without failing.
fn bootstrap_reply(response: a2a::types::SendMessageResponse) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": "corr-1",
        "result": serde_json::to_value(response).unwrap(),
    }))
    .unwrap()
}

fn bootstrap_task(task_id: &str) -> Vec<u8> {
    bootstrap_reply(a2a::types::SendMessageResponse::Task(a2a::types::Task {
        id: task_id.to_string(),
        context_id: String::new(),
        status: a2a::types::TaskStatus {
            state: a2a::types::TaskState::Working,
            message: None,
            timestamp: None,
        },
        artifacts: None,
        history: None,
        metadata: None,
    }))
}

fn bootstrap_bare_message() -> Vec<u8> {
    bootstrap_reply(a2a::types::SendMessageResponse::Message(a2a::types::Message::new(
        a2a::types::Role::Agent,
        vec![a2a::types::Part::text("done")],
    )))
}

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
        "a2a.v1.gateway.planner.message.send"
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
    let stream_body = json!({"method": "message/stream", "params": {}});
    let bootstrap = bootstrap_task("task-7");
    let plan = sse_plan("message/stream", &stream_body, &bootstrap, &ReqId::from_header("req-9"))
        .unwrap()
        .expect("a task means a consumer");
    match plan {
        SseConsumePlan::MessageStreamBootstrap { task_id, req_id } => {
            assert_eq!(task_id.as_str(), "task-7");
            assert_eq!(req_id.as_str(), "req-9");
        }
        other => panic!("expected a bootstrap plan, got {other:?}"),
    }

    let resub_body = json!({
        "method": "tasks/resubscribe",
        "params": { "taskId": "task-1", "lastSequence": 3 }
    });
    let plan = sse_plan("tasks/resubscribe", &resub_body, b"", &ReqId::from_header("req-9")).unwrap();
    assert!(matches!(
        plan,
        Some(SseConsumePlan::TasksResubscribe { last_seq: 3, .. })
    ));
}

#[test]
fn plan_consumer_demuxes_a_live_subscription_but_not_a_resume() {
    let prefix = default_a2a_prefix();
    let task_id = A2aTaskId::new("task-7").unwrap();

    let (_cfg, demux) = SseConsumePlan::MessageStreamBootstrap {
        task_id: task_id.clone(),
        req_id: ReqId::from_header("req-9"),
    }
    .consumer(&prefix);
    assert_eq!(demux.map(|r| r.as_str().to_owned()), Some("req-9".to_owned()));

    let (_cfg, demux) = SseConsumePlan::TasksResubscribe { task_id, last_seq: 4 }.consumer(&prefix);
    assert!(demux.is_none(), "a resume replays events of the original request");
}

#[test]
fn event_forwards_only_when_it_carries_the_subscriptions_req_id() {
    let req_id = ReqId::from_header("req-9");
    let mut mine = async_nats::HeaderMap::new();
    mine.insert(REQ_ID_HEADER, "req-9");
    let mut theirs = async_nats::HeaderMap::new();
    theirs.insert(REQ_ID_HEADER, "req-other");

    assert!(event_forwards_to_caller(Some(&req_id), Some(&mine)));
    assert!(!event_forwards_to_caller(Some(&req_id), Some(&theirs)));
    assert!(!event_forwards_to_caller(Some(&req_id), None));
    assert!(event_forwards_to_caller(None, Some(&theirs)));
}

#[test]
fn sse_plan_returns_no_consumer_when_the_reply_carries_no_task() {
    // `message/stream` may answer with a bare `Message`; no task means no
    // events, so opening a consumer would just leak one.
    let body = json!({"method": "message/stream", "params": {}});
    let bootstrap = bootstrap_bare_message();
    assert!(
        sse_plan("message/stream", &body, &bootstrap, &ReqId::from_header("req-9"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn sse_plan_rejects_a_task_id_that_is_not_a_nats_token() {
    let body = json!({"method": "message/stream", "params": {}});
    let bootstrap = bootstrap_task("task.*");
    let err = sse_plan("message/stream", &body, &bootstrap, &ReqId::from_header("req-9")).unwrap_err();
    assert!(matches!(err, BridgeError::StreamingParams(_)));
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
        Some("a2a.v1.gateway.planner.tasks.get")
    );
}

#[tokio::test]
async fn handle_jsonrpc_rejects_a_streaming_request_without_a_usable_id() {
    for id in [json!(null), json!({})] {
        let publisher = Arc::new(RecordingInboundPublisher::new());
        let state = test_state(publisher.clone());
        let body =
            Bytes::from(json!({ "jsonrpc": "2.0", "id": id, "method": "message/stream", "params": {} }).to_string());
        let err = handle_jsonrpc(caller_headers("planner", None), body, &state)
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::MissingJsonRpcId), "id {id} was accepted");
        assert!(
            publisher.peek_subject().is_none(),
            "the gateway must not open a stream nobody can correlate"
        );
    }
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
    let err = sse_plan("tasks/subscribe", &json!({}), b"", &ReqId::from_header("req-9")).unwrap_err();
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

/// Reads an SSE stream the way a caller does: every frame is an unnamed
/// `data:` line, so the JSON bodies are what the assertions look at.
async fn sse_frames(stream: BoxStream<'static, Result<Event, Infallible>>) -> Vec<String> {
    let response = Sse::new(stream).into_response();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(|d| d.trim().to_owned()))
        .collect()
}

#[tokio::test]
async fn every_sse_frame_is_a_json_rpc_response_carrying_the_caller_id() {
    let bootstrap = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"transport-9","result":{"taskId":"t-1"}})).unwrap();
    let tail = stream::iter(vec![
        Ok(Bytes::from_static(b"not-an-envelope")),
        Err(BridgeError::JetStreamConsume("consumer gone".into())),
    ]);
    let frames = sse_frames(sse_from_bootstrap_and_payloads(
        bootstrap,
        Box::pin(tail),
        json!("corr-1"),
    ))
    .await;

    assert_eq!(frames.len(), 3);
    let bootstrap: serde_json::Value = serde_json::from_str(&frames[0]).unwrap();
    assert_eq!(bootstrap["id"], "corr-1");
    assert_eq!(bootstrap["result"]["taskId"], "t-1");

    let undecodable: serde_json::Value = serde_json::from_str(&frames[1]).unwrap();
    assert_eq!(undecodable["id"], "corr-1");
    assert_eq!(undecodable["error"]["code"], INTERNAL_ERROR);

    let failure: serde_json::Value = serde_json::from_str(&frames[2]).unwrap();
    assert_eq!(failure["id"], "corr-1");
    assert_eq!(failure["error"]["code"], INTERNAL_ERROR);
    assert!(
        failure["error"]["message"].as_str().unwrap().contains("consumer gone"),
        "the caller keeps the only diagnostic the stream produced: {failure}"
    );
}

#[tokio::test]
async fn an_event_an_older_agent_published_reaches_the_caller_as_a_response() {
    // The events stream retains by limits and a rolling upgrade runs both agent
    // releases at once, so this edge still meets bare events. Stamping an id onto
    // one and forwarding it would emit a `data:` line that is no JSON-RPC response.
    let legacy = serde_json::to_vec(&a2a::event::StreamResponse::StatusUpdate(
        a2a::event::TaskStatusUpdateEvent {
            task_id: "t-1".to_owned(),
            context_id: "ctx".to_owned(),
            status: a2a::types::TaskStatus {
                state: a2a::types::TaskState::Working,
                message: None,
                timestamp: None,
            },
            metadata: None,
        },
    ))
    .unwrap();
    let frames = sse_frames(sse_from_bootstrap_and_payloads(
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"transport-9","result":{"taskId":"t-1"}})).unwrap(),
        Box::pin(stream::iter(vec![Ok(Bytes::from(legacy))])),
        json!("corr-1"),
    ))
    .await;

    let event: serde_json::Value = serde_json::from_str(&frames[1]).unwrap();
    assert_eq!(event["jsonrpc"], "2.0");
    assert_eq!(event["id"], "corr-1");
    assert_eq!(event["result"]["statusUpdate"]["taskId"], "t-1");
}
