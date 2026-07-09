use crate::AgentHandler;
use crate::agent::test_support::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response};
use crate::error::AGENT_UNAVAILABLE;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::schema::v1::{LlmProtocol, SetProviderRequest, SetProviderResponse};

fn request() -> SetProviderRequest {
    SetProviderRequest::new("anthropic", LlmProtocol::Anthropic, "https://api.anthropic.com")
}

#[tokio::test]
async fn set_provider_forwards_request_and_returns_response() {
    let (mock, _js, bridge) = mock_bridge();
    let expected = SetProviderResponse::new();
    set_json_response(&mock, "acp.agent.providers.set", &expected);

    let result = bridge.set_provider(request()).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn set_provider_returns_error_when_nats_fails() {
    let (mock, _js, bridge) = mock_bridge();
    mock.fail_next_request();

    let err = bridge.set_provider(request()).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
}

#[tokio::test]
async fn set_provider_returns_error_when_response_is_invalid_json() {
    let (mock, _js, bridge) = mock_bridge();
    mock.set_response("acp.agent.providers.set", "not json".into());

    let err = bridge.set_provider(request()).await.unwrap_err();

    assert_eq!(err.code, ErrorCode::InternalError);
}

#[tokio::test]
async fn set_provider_records_metrics_on_success() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    set_json_response(&mock, "acp.agent.providers.set", &SetProviderResponse::new());

    let _ = bridge.set_provider(request()).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "set_provider", true),
        "expected acp.requests with method=set_provider, success=true"
    );
    provider.shutdown().unwrap();
}

#[tokio::test]
async fn set_provider_records_metrics_on_failure() {
    let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
    mock.fail_next_request();

    let _ = bridge.set_provider(request()).await;

    provider.force_flush().unwrap();
    let finished_metrics = exporter.get_finished_metrics().unwrap();
    assert!(
        has_request_metric(&finished_metrics, "set_provider", false),
        "expected acp.requests with method=set_provider, success=false"
    );
    provider.shutdown().unwrap();
}
