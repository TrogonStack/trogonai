use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, agent};
use agent_client_protocol::{ListSessionsRequest, ListSessionsResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(name = "acp.session.list", skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: ListSessionsRequest,
) -> Result<ListSessionsResponse> {
    let start = bridge.clock.now();

    info!("List sessions request");

    let nats = bridge.nats();
    let subject = agent::session_list(bridge.config.acp_prefix());

    let result = nats::request_with_timeout::<N, ListSessionsRequest, ListSessionsResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "list_sessions",
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
    use agent_client_protocol::{Agent, ErrorCode, ListSessionsRequest, ListSessionsResponse};

    #[tokio::test]
    async fn list_sessions_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = ListSessionsResponse::new(vec![]);
        set_json_response(&mock, "acp.agent.session.list", &expected);

        let request = ListSessionsRequest::new();
        let result = bridge.list_sessions(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn list_sessions_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = ListSessionsRequest::new();
        let err = bridge.list_sessions(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn list_sessions_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.session.list", "not json".into());

        let request = ListSessionsRequest::new();
        let err = bridge.list_sessions(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn list_sessions_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.agent.session.list",
            &ListSessionsResponse::new(vec![]),
        );

        let _ = bridge.list_sessions(ListSessionsRequest::new()).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "list_sessions", true),
            "expected acp.requests with method=list_sessions, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn list_sessions_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge.list_sessions(ListSessionsRequest::new()).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "list_sessions", false),
            "expected acp.requests with method=list_sessions, success=false"
        );
        provider.shutdown().unwrap();
    }
}
