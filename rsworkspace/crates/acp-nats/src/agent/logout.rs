use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, agent};
use agent_client_protocol::{LogoutRequest, LogoutResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(name = "acp.logout", skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: LogoutRequest,
) -> Result<LogoutResponse> {
    let start = bridge.clock.now();

    info!("Logout request");
    let nats = bridge.nats();

    let result = nats::request_with_timeout::<N, LogoutRequest, LogoutResponse>(
        nats,
        &agent::LogoutSubject::new(bridge.config.acp_prefix_ref()),
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "logout",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use crate::agent::test_support::{
        has_request_metric, mock_bridge, mock_bridge_with_metrics, set_json_response,
    };
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{Agent, ErrorCode, LogoutRequest, LogoutResponse};

    #[tokio::test]
    async fn logout_forwards_request_and_returns_response() {
        let (mock, _js, bridge) = mock_bridge();
        let expected = LogoutResponse::new();
        set_json_response(&mock, "acp.agent.logout", &expected);

        let request = LogoutRequest::new();
        let result = bridge.logout(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn logout_returns_error_when_nats_request_fails() {
        let (mock, _js, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = LogoutRequest::new();
        let err = bridge.logout(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn logout_returns_error_when_response_is_invalid_json() {
        let (mock, _js, bridge) = mock_bridge();
        mock.set_response("acp.agent.logout", "not json".into());

        let request = LogoutRequest::new();
        let err = bridge.logout(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn logout_records_metrics_on_success() {
        let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(&mock, "acp.agent.logout", &LogoutResponse::default());

        let _ = bridge.logout(LogoutRequest::new()).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "logout", true),
            "expected acp.requests with method=logout, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn logout_records_metrics_on_failure() {
        let (mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge.logout(LogoutRequest::new()).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "logout", false),
            "expected acp.requests with method=logout, success=false"
        );
        provider.shutdown().unwrap();
    }
}
