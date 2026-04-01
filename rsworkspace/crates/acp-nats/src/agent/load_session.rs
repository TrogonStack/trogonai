use super::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, session};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{Error, ErrorCode, LoadSessionRequest, LoadSessionResponse, Result};
use tracing::{info, instrument};
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.load",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
>(
    bridge: &Bridge<N, C, J>,
    args: LoadSessionRequest,
) -> Result<LoadSessionResponse>
where
    <<J::Stream as trogon_nats::jetstream::JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Message: JsRequestMessage,
{
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
    let prefix = bridge.config.acp_prefix_ref();
    let subject = session::agent::LoadSubject::new(prefix, &session_id);

    let result = bridge
        .session_request::<LoadSessionRequest, LoadSessionResponse>(
            &subject,
            &args,
            session_id.as_str(),
        )
        .await;

    if result.is_ok() {
        bridge.schedule_session_ready(args.session_id.clone());
    }

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
        has_request_metric, has_session_ready_error_metric, mock_bridge, mock_bridge_with_metrics,
        set_json_response,
    };
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{Agent, ErrorCode, LoadSessionRequest, LoadSessionResponse};
    use std::time::Duration;

    #[tokio::test]
    async fn load_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = LoadSessionResponse::new();
        set_json_response(&mock, "acp.session.s1.agent.load", &expected);

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
        mock.set_response("acp.session.s1.agent.load", "not json".into());

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
            "acp.session.s1.agent.load",
            &LoadSessionResponse::new(),
        );

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(150)).await;
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

    #[tokio::test]
    async fn load_session_records_error_when_session_ready_publish_fails() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.session.s1.agent.load",
            &LoadSessionResponse::new(),
        );
        mock.fail_publish_count(4);

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(600)).await;
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_session_ready_error_metric(&finished_metrics),
            "expected acp.errors.total datapoint with operation=session_ready, reason=session_ready_publish_failed"
        );
        assert!(
            has_request_metric(&finished_metrics, "load_session", true),
            "expected acp.requests with method=load_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn load_session_publishes_session_ready_to_correct_subject() {
        let (mock, bridge) = mock_bridge();
        set_json_response(
            &mock,
            "acp.session.s1.agent.load",
            &LoadSessionResponse::new(),
        );

        let _ = bridge
            .load_session(LoadSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.session.s1.agent.ext.ready".to_string()),
            "expected publish to acp.session.s1.agent.ext.ready, got: {:?}",
            published
        );
    }
}
