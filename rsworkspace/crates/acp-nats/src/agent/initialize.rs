use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, agent};
use agent_client_protocol::{InitializeRequest, InitializeResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.initialize",
    skip(bridge, args),
    fields(protocol_version = ?args.protocol_version)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: InitializeRequest,
) -> Result<InitializeResponse> {
    let start = bridge.clock.now();

    let client_name = args
        .client_info
        .as_ref()
        .map(|c| c.name.as_str())
        .unwrap_or("unknown");

    info!(client = %client_name, "Initialize request");

    let nats = bridge.nats();
    let subject = agent::initialize(bridge.config.acp_prefix());

    let result = nats::request_with_timeout::<N, InitializeRequest, InitializeResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "initialize",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::Bridge;
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use crate::test_helpers::{has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response};
    use agent_client_protocol::{
        Agent, ErrorCode, Implementation, InitializeRequest, InitializeResponse, ProtocolVersion,
    };
    use trogon_nats::AdvancedMockNatsClient;

    #[tokio::test]
    async fn initialize_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = InitializeResponse::new(ProtocolVersion::LATEST);
        set_json_response(&mock, "acp.agent.initialize", &expected);

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let result = bridge.initialize(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.protocol_version, ProtocolVersion::LATEST);
    }

    #[tokio::test]
    async fn initialize_logs_client_name_when_client_info_provided() {
        let (mock, bridge) = mock_bridge();
        let expected = InitializeResponse::new(ProtocolVersion::LATEST);
        set_json_response(&mock, "acp.agent.initialize", &expected);

        let request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("my-client", "1.0.0"));
        let result = bridge.initialize(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn initialize_returns_error_when_nats_request_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let err = bridge.initialize(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn initialize_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.initialize", "not json".into());

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let err = bridge.initialize(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn initialize_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.agent.initialize",
            &InitializeResponse::new(ProtocolVersion::LATEST),
        );

        let _ = bridge
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(has_request_metric(&finished_metrics, "initialize", true), "expected acp.requests with method=initialize, success=true");
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn initialize_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(has_request_metric(&finished_metrics, "initialize", false), "expected acp.requests with method=initialize, success=false");
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn handlers_use_custom_prefix() {
        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("myorg.prod"),
            tokio::sync::mpsc::channel(1).0,
        );
        let expected = InitializeResponse::new(ProtocolVersion::LATEST);
        set_json_response(&mock, "myorg.prod.agent.initialize", &expected);

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let result = bridge.initialize(request).await;
        assert!(result.is_ok());
    }

}
