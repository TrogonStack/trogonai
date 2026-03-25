use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, agent};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{Error, ErrorCode, LoadSessionRequest, LoadSessionResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.load",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: LoadSessionRequest,
) -> Result<LoadSessionResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Load session request");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge
            .metrics
            .record_error("session_validate", "invalid_session_id");
        Error::new(
            ErrorCode::InvalidParams.into(),
            format!("Invalid session ID: {}", e),
        )
    })?;
    let nats = bridge.nats();
    let subject = agent::session_load(bridge.config.acp_prefix(), session_id.as_str());

    let result = nats::request_with_timeout::<N, LoadSessionRequest, LoadSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "load_session",
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
    use agent_client_protocol::{Agent, ErrorCode, LoadSessionRequest, LoadSessionResponse};

    #[tokio::test]
    async fn load_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = LoadSessionResponse::new();
        set_json_response(&mock, "acp.s1.agent.session.load", &expected);

        let request = LoadSessionRequest::new("s1", ".");
        let result = bridge.load_session(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn load_session_returns_error_when_nats_request_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = LoadSessionRequest::new("s1", ".");
        let err = bridge.load_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn load_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.s1.agent.session.load", "not json".into());

        let request = LoadSessionRequest::new("s1", ".");
        let err = bridge.load_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn load_session_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.s1.agent.session.load",
            &LoadSessionResponse::new(),
        );

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "load_session", true),
            "expected acp.requests with method=load_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn load_session_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "load_session", false),
            "expected acp.requests with method=load_session, success=false"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn load_session_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = LoadSessionRequest::new("invalid.session.id", ".");
        let err = bridge.load_session(request).await.unwrap_err();
        assert!(err.to_string().contains("Invalid session ID"));
    }
}
