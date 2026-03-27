use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, session};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{
    Error, ErrorCode, Result, SetSessionModelRequest, SetSessionModelResponse,
};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.set_model",
    skip(bridge, args),
    fields(session_id = %args.session_id, model_id = %args.model_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: SetSessionModelRequest,
) -> Result<SetSessionModelResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, model_id = %args.model_id, "Set session model request");

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
    let subject = session::agent::set_model(bridge.config.acp_prefix(), session_id.as_str());

    let result = nats::request_with_timeout::<N, SetSessionModelRequest, SetSessionModelResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "set_session_model",
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
    use agent_client_protocol::{
        Agent, ErrorCode, SetSessionModelRequest, SetSessionModelResponse,
    };

    #[tokio::test]
    async fn set_session_model_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = SetSessionModelResponse::new();
        set_json_response(&mock, "acp.session.s1.agent.set_model", &expected);

        let request = SetSessionModelRequest::new("s1", "claude-sonnet-4-6");
        let result = bridge.set_session_model(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_session_model_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = SetSessionModelRequest::new("s1", "claude-sonnet-4-6");
        let err = bridge.set_session_model(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn set_session_model_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.session.s1.agent.set_model", "not json".into());

        let request = SetSessionModelRequest::new("s1", "claude-sonnet-4-6");
        let err = bridge.set_session_model(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn set_session_model_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = SetSessionModelRequest::new("invalid.session.id", "claude-sonnet-4-6");
        let err = bridge.set_session_model(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn set_session_model_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.session.s1.agent.set_model",
            &SetSessionModelResponse::new(),
        );

        let _ = bridge
            .set_session_model(SetSessionModelRequest::new("s1", "claude-sonnet-4-6"))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "set_session_model", true),
            "expected acp.requests with method=set_session_model, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn set_session_model_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge
            .set_session_model(SetSessionModelRequest::new("s1", "claude-sonnet-4-6"))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "set_session_model", false),
            "expected acp.requests with method=set_session_model, success=false"
        );
        provider.shutdown().unwrap();
    }
}
