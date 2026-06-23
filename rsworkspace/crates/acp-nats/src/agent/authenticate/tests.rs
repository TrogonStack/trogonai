use crate::agent::test_support::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response};
use crate::error::AGENT_UNAVAILABLE;
use agent_client_protocol::{Agent, AuthenticateRequest, AuthenticateResponse, ErrorCode};

#[tokio::test]
async fn authenticate_forwards_request_and_returns_response() {
    let (mock, _js, bridge) = mock_bridge();
    let expected = AuthenticateResponse::new();
    set_json_response(&mock, "acp.agent.authenticate", &expected);

    let request = AuthenticateRequest::new("api-key");
    let result = bridge.authenticate(request).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn authenticate_returns_error_when_nats_request_fails() {
    let (mock, _js, bridge) = mock_bridge();
    mock.fail_next_request();

    let request = AuthenticateRequest::new("test");
    let err = bridge.authenticate(request).await.unwrap_err();

    assert!(err.to_string().contains("Agent unavailable"));
    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

#[tokio::test]
async fn authenticate_returns_error_when_response_is_invalid_json() {
    let (mock, _js, bridge) = mock_bridge();
    mock.set_response("acp.agent.authenticate", "not json".into());

    let request = AuthenticateRequest::new("test");
    let err = bridge.authenticate(request).await.unwrap_err();

    assert!(err.to_string().contains("Invalid response from agent"));
    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn authenticate_records_metrics_on_success() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    set_json_response(&mock, "acp.agent.authenticate", &AuthenticateResponse::default());

    let _ = bridge.authenticate(AuthenticateRequest::new("test")).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "authenticate", true),
        "expected acp.requests with method=authenticate, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn authenticate_records_metrics_on_failure() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    mock.fail_next_request();

    let _ = bridge.authenticate(AuthenticateRequest::new("test")).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "authenticate", false),
        "expected acp.requests with method=authenticate, success=false"
    );
    provider.shutdown().unwrap();
}
