use crate::agent::test_support::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_js_response};
use agent_client_protocol::{Agent, ErrorCode, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse};

#[tokio::test]
async fn set_session_config_option_forwards_request_and_returns_response() {
    let (_mock, js, bridge) = mock_bridge();
    let expected = SetSessionConfigOptionResponse::new(vec![]);
    set_js_response(&js, &expected);

    let request = SetSessionConfigOptionRequest::new("s1", "theme", "dark");
    let result = bridge.set_session_config_option(request).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn set_session_config_option_returns_error_when_js_fails() {
    let (_mock, _js, bridge) = mock_bridge();

    let request = SetSessionConfigOptionRequest::new("s1", "theme", "dark");
    let err = bridge.set_session_config_option(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn set_session_config_option_returns_error_when_response_is_invalid_json() {
    let (_mock, js, bridge) = mock_bridge();
    crate::agent::test_support::set_js_raw_response(&js, b"not json");

    let request = SetSessionConfigOptionRequest::new("s1", "theme", "dark");
    let err = bridge.set_session_config_option(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn set_session_config_option_validates_session_id() {
    let (_mock, _js, bridge) = mock_bridge();
    let request = SetSessionConfigOptionRequest::new("invalid.session.id", "theme", "dark");
    let err = bridge.set_session_config_option(request).await.unwrap_err();

    assert!(err.message.contains("Invalid session ID"));
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn set_session_config_option_records_metrics_on_success() {
    let (_mock, js, bridge, exporter, provider) = mock_bridge_with_metrics();
    set_js_response(&js, &SetSessionConfigOptionResponse::new(vec![]));

    let _ = bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new("s1", "theme", "dark"))
        .await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "set_session_config_option", true),
        "expected acp.requests with method=set_session_config_option, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn set_session_config_option_records_metrics_on_failure() {
    let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

    let _ = bridge
        .set_session_config_option(SetSessionConfigOptionRequest::new("s1", "theme", "dark"))
        .await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "set_session_config_option", false),
        "expected acp.requests with method=set_session_config_option, success=false"
    );
    provider.shutdown().unwrap();
}
