use crate::AgentHandler;
use crate::agent::test_support::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_js_response};
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{DeleteSessionRequest, DeleteSessionResponse};

#[tokio::test]
async fn delete_session_forwards_request_and_returns_response() {
    let (_mock, js, bridge) = mock_bridge();
    let expected = DeleteSessionResponse::new();
    set_js_response(&js, &expected);

    let request = DeleteSessionRequest::new("s1");
    let result = bridge.delete_session(request).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn delete_session_returns_error_when_js_fails() {
    let (_mock, _js, bridge) = mock_bridge();

    let request = DeleteSessionRequest::new("s1");
    let err = bridge.delete_session(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn delete_session_returns_error_when_response_is_invalid_json() {
    let (_mock, js, bridge) = mock_bridge();
    crate::agent::test_support::set_js_raw_response(&js, b"not json");

    let request = DeleteSessionRequest::new("s1");
    let err = bridge.delete_session(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn delete_session_validates_session_id() {
    let (_mock, _js, bridge) = mock_bridge();
    let request = DeleteSessionRequest::new("invalid.session.id");
    let err = bridge.delete_session(request).await.unwrap_err();

    assert!(err.message.contains("Invalid session ID"));
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn delete_session_records_metrics_on_success() {
    let (_mock, js, bridge, exporter, provider) = mock_bridge_with_metrics();
    set_js_response(&js, &DeleteSessionResponse::new());

    let _ = bridge.delete_session(DeleteSessionRequest::new("s1")).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "delete_session", true),
        "expected acp.requests with method=delete_session, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn delete_session_records_metrics_on_failure() {
    let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

    let _ = bridge.delete_session(DeleteSessionRequest::new("s1")).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "delete_session", false),
        "expected acp.requests with method=delete_session, success=false"
    );
    provider.shutdown().unwrap();
}
