use crate::agent::test_support::{
    has_error_metric, has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response,
};
use agent_client_protocol::{Agent, ErrorCode, ExtRequest, ExtResponse};
use serde_json::value::RawValue;

#[tokio::test]
async fn ext_method_forwards_request_and_returns_response() {
    let (mock, _js, bridge) = mock_bridge();
    let raw = RawValue::from_string(r#"{"result":"ok"}"#.to_string()).unwrap();
    let expected = ExtResponse::new(raw.into());
    set_json_response(&mock, "acp.agent.ext.my_method", &expected);

    let params = RawValue::from_string(r#"{"key":"value"}"#.to_string()).unwrap();
    let request = ExtRequest::new("my_method", params.into());
    let result = bridge.ext_method(request).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn ext_method_returns_error_when_nats_fails() {
    let (mock, _js, bridge) = mock_bridge();
    mock.fail_next_request();

    let params = RawValue::from_string("{}".to_string()).unwrap();
    let request = ExtRequest::new("my_method", params.into());
    let err = bridge.ext_method(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::Other(crate::error::AGENT_UNAVAILABLE));
}

#[tokio::test]
async fn ext_method_returns_error_when_response_is_invalid_json() {
    let (mock, _js, bridge) = mock_bridge();
    mock.set_response("acp.agent.ext.my_method", "not json".into());

    let params = RawValue::from_string("{}".to_string()).unwrap();
    let request = ExtRequest::new("my_method", params.into());
    let err = bridge.ext_method(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn ext_method_validates_method_name() {
    let (_mock, _js, bridge) = mock_bridge();
    let params = RawValue::from_string("{}".to_string()).unwrap();
    let request = ExtRequest::new("method.*", params.into());
    let err = bridge.ext_method(request).await.unwrap_err();

    assert!(err.to_string().contains("Invalid method name"));
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn ext_method_records_error_metric_on_invalid_method_name() {
    let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    let params = RawValue::from_string("{}".to_string()).unwrap();
    let request = ExtRequest::new("invalid method", params.into());

    let _ = bridge.ext_method(request).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_error_metric(&finished_metrics, "ext_method", "invalid_method_name"),
        "expected acp.errors with operation=ext_method, reason=invalid_method_name"
    );
    assert!(
        has_request_metric(&finished_metrics, "ext_method", false),
        "expected acp.requests with method=ext_method, success=false on validation failure"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn ext_method_records_metrics_on_success() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    let raw = RawValue::from_string("{}".to_string()).unwrap();
    set_json_response(&mock, "acp.agent.ext.my_method", &ExtResponse::new(raw.into()));

    let params = RawValue::from_string("{}".to_string()).unwrap();
    let _ = bridge.ext_method(ExtRequest::new("my_method", params.into())).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "ext_method", true),
        "expected acp.requests with method=ext_method, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn ext_method_records_metrics_on_failure() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    mock.fail_next_request();

    let params = RawValue::from_string("{}".to_string()).unwrap();
    let _ = bridge.ext_method(ExtRequest::new("my_method", params.into())).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "ext_method", false),
        "expected acp.requests with method=ext_method, success=false"
    );
    provider.shutdown().unwrap();
}
