use super::Bridge;
use crate::nats::{FlushClient, PublishClient, RequestClient, session};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{Error, ErrorCode, ForkSessionRequest, ForkSessionResponse, Result};
use tracing::{Span, info, instrument};
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.fork",
    skip(bridge, args),
    fields(session_id = %args.session_id, new_session_id = tracing::field::Empty)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
>(
    bridge: &Bridge<N, C, J>,
    args: ForkSessionRequest,
) -> Result<ForkSessionResponse>
where
    <<J::Stream as trogon_nats::jetstream::JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Message: JsRequestMessage,
{
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Fork session request");

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
    let subject = session::agent::ForkSubject::new(prefix, &session_id);

    let result = bridge
        .session_request::<ForkSessionRequest, ForkSessionResponse>(
            &subject,
            &args,
            session_id.as_str(),
        )
        .await;

    if let Ok(ref response) = result {
        Span::current().record("new_session_id", response.session_id.to_string().as_str());
        info!(new_session_id = %response.session_id, "Session forked");

        bridge.schedule_session_ready(response.session_id.clone());
    }

    bridge.metrics.record_request(
        "fork_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use crate::agent::test_support::{
        has_request_metric, has_session_ready_error_metric, mock_bridge, mock_bridge_with_metrics,
        set_js_response,
    };
    use agent_client_protocol::{
        Agent, ErrorCode, ForkSessionRequest, ForkSessionResponse, SessionId,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn fork_session_forwards_request_and_returns_response() {
        let (_mock, js, bridge) = mock_bridge();
        let new_session_id = SessionId::from("forked-session-1");
        let expected = ForkSessionResponse::new(new_session_id.clone());
        set_js_response(&js, &expected);

        let request = ForkSessionRequest::new("s1", ".");
        let result = bridge.fork_session(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.session_id, new_session_id);
    }

    #[tokio::test]
    async fn fork_session_returns_error_when_js_fails() {
        let (_mock, _js, bridge) = mock_bridge();

        let request = ForkSessionRequest::new("s1", ".");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn fork_session_returns_error_when_response_is_invalid_json() {
        let (_mock, js, bridge) = mock_bridge();
        crate::agent::test_support::set_js_raw_response(&js, b"not json");

        let request = ForkSessionRequest::new("s1", ".");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn fork_session_validates_session_id() {
        let (_mock, _js, bridge) = mock_bridge();
        let request = ForkSessionRequest::new("invalid.session.id", ".");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn fork_session_publishes_session_ready_to_correct_subject() {
        let (mock, js, bridge) = mock_bridge();
        let new_session_id = SessionId::from("forked-session-1");
        set_js_response(&js, &ForkSessionResponse::new(new_session_id));

        let _ = bridge
            .fork_session(ForkSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.session.forked-session-1.agent.ext.ready".to_string()),
            "expected publish to acp.session.forked-session-1.agent.ext.ready, got: {:?}",
            published
        );
    }

    #[tokio::test]
    async fn fork_session_records_metrics_on_success() {
        let (_mock, js, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_js_response(&js, &ForkSessionResponse::new("forked-1"));

        let _ = bridge
            .fork_session(ForkSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "fork_session", true),
            "expected acp.requests with method=fork_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn fork_session_records_metrics_on_failure() {
        let (_mock, _js, bridge, exporter, provider) = mock_bridge_with_metrics();

        let _ = bridge
            .fork_session(ForkSessionRequest::new("s1", "."))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "fork_session", false),
            "expected acp.requests with method=fork_session, success=false"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn fork_session_records_error_when_session_ready_publish_fails() {
        let (mock, js, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_js_response(&js, &ForkSessionResponse::new("forked-1"));
        mock.fail_publish_count(4);

        let _ = bridge
            .fork_session(ForkSessionRequest::new("s1", "."))
            .await;

        tokio::time::sleep(Duration::from_millis(600)).await;
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_session_ready_error_metric(&finished_metrics),
            "expected acp.errors.total datapoint with operation=session_ready, reason=session_ready_publish_failed"
        );
        assert!(
            has_request_metric(&finished_metrics, "fork_session", true),
            "expected acp.requests with method=fork_session, success=true"
        );
        provider.shutdown().unwrap();
    }
}
