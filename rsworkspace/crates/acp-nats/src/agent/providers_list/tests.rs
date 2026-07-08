use crate::AgentHandler;
use crate::agent::test_support::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response};
use crate::error::AGENT_UNAVAILABLE;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{ListProvidersRequest, ListProvidersResponse};

#[tokio::test]
async fn list_providers_forwards_request_and_returns_response() {
    let (mock, _js, bridge) = mock_bridge();
    let expected = ListProvidersResponse::new(vec![]);
    set_json_response(&mock, "acp.agent.providers.list", &expected);

    let request = ListProvidersRequest::new();
    let result = bridge.list_providers(request).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn list_providers_returns_error_when_nats_fails() {
    let (mock, _js, bridge) = mock_bridge();
    mock.fail_next_request();

    let request = ListProvidersRequest::new();
    let err = bridge.list_providers(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

#[tokio::test]
async fn list_providers_returns_error_when_response_is_invalid_json() {
    let (mock, _js, bridge) = mock_bridge();
    mock.set_response("acp.agent.providers.list", "not json".into());

    let request = ListProvidersRequest::new();
    let err = bridge.list_providers(request).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn list_providers_records_metrics_on_success() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    set_json_response(&mock, "acp.agent.providers.list", &ListProvidersResponse::new(vec![]));

    let _ = bridge.list_providers(ListProvidersRequest::new()).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "list_providers", true),
        "expected acp.requests with method=list_providers, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn list_providers_records_metrics_on_failure() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    mock.fail_next_request();

    let _ = bridge.list_providers(ListProvidersRequest::new()).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "list_providers", false),
        "expected acp.requests with method=list_providers, success=false"
    );
    provider.shutdown().unwrap();
}
