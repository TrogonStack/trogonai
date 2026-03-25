use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, agent};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{
    Error, ErrorCode, Result, ResumeSessionRequest, ResumeSessionResponse,
};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.resume",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: ResumeSessionRequest,
) -> Result<ResumeSessionResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Resume session request");

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
    let subject = agent::session_resume(bridge.config.acp_prefix(), session_id.as_str());

    let result = nats::request_with_timeout::<N, ResumeSessionRequest, ResumeSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "resume_session",
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
    use agent_client_protocol::{Agent, ErrorCode, ResumeSessionRequest, ResumeSessionResponse};

    #[tokio::test]
    async fn resume_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = ResumeSessionResponse::new();
        set_json_response(&mock, "acp.s1.agent.session.resume", &expected);

        let request = ResumeSessionRequest::new("s1", ".");
        let result = bridge.resume_session(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn resume_session_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = ResumeSessionRequest::new("s1", ".");
        let err = bridge.resume_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn resume_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.s1.agent.session.resume", "not json".into());

        let request = ResumeSessionRequest::new("s1", ".");
        let err = bridge.resume_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn resume_session_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = ResumeSessionRequest::new("invalid.session.id", ".");
        let err = bridge.resume_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn resume_session_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.s1.agent.session.resume",
            &ResumeSessionResponse::new(),
        );

        let _ = bridge
            .resume_session(ResumeSessionRequest::new("s1", "."))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "resume_session", true),
            "expected acp.requests with method=resume_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn resume_session_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge
            .resume_session(ResumeSessionRequest::new("s1", "."))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "resume_session", false),
            "expected acp.requests with method=resume_session, success=false"
        );
        provider.shutdown().unwrap();
    }
}
