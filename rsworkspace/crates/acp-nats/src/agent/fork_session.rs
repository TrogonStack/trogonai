use super::Bridge;
use super::js_request;
use crate::error::map_nats_error;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, session};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{Error, ErrorCode, ForkSessionRequest, ForkSessionResponse, Result};
use tracing::{Span, info, instrument};
use trogon_nats::jetstream::{JetStreamConsumerFactory, JetStreamPublisher};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.fork",
    skip(bridge, args),
    fields(session_id = %args.session_id, new_session_id = tracing::field::Empty)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamConsumerFactory,
>(
    bridge: &Bridge<N, C, J>,
    args: ForkSessionRequest,
) -> Result<ForkSessionResponse> {
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
    let prefix = bridge.config.acp_prefix();
    let subject = session::agent::fork(prefix, session_id.as_str());

    let result = match bridge.js() {
        Some(js) => {
            let req_id = uuid::Uuid::new_v4().to_string();
            js_request::js_request::<J, _, ForkSessionResponse, _>(
                js,
                &subject,
                &args,
                &trogon_std::StdJsonSerialize,
                prefix,
                session_id.as_str(),
                &req_id,
                bridge.config.operation_timeout,
            )
            .await
        }
        None => nats::request_with_timeout::<N, ForkSessionRequest, ForkSessionResponse>(
            bridge.nats(),
            &subject,
            &args,
            bridge.config.operation_timeout,
        )
        .await
        .map_err(map_nats_error),
    };

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
        set_json_response,
    };
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{
        Agent, ErrorCode, ForkSessionRequest, ForkSessionResponse, SessionId,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn fork_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let new_session_id = SessionId::from("forked-session-1");
        let expected = ForkSessionResponse::new(new_session_id.clone());
        set_json_response(&mock, "acp.session.s1.agent.fork", &expected);

        let request = ForkSessionRequest::new("s1", ".");
        let result = bridge.fork_session(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.session_id, new_session_id);
    }

    #[tokio::test]
    async fn fork_session_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = ForkSessionRequest::new("s1", ".");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn fork_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.session.s1.agent.fork", "not json".into());

        let request = ForkSessionRequest::new("s1", ".");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn fork_session_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = ForkSessionRequest::new("invalid.session.id", ".");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn fork_session_publishes_session_ready_to_correct_subject() {
        let (mock, bridge) = mock_bridge();
        let new_session_id = SessionId::from("forked-session-1");
        set_json_response(
            &mock,
            "acp.session.s1.agent.fork",
            &ForkSessionResponse::new(new_session_id),
        );

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
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.session.s1.agent.fork",
            &ForkSessionResponse::new("forked-1"),
        );

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
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

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
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.session.s1.agent.fork",
            &ForkSessionResponse::new("forked-1"),
        );
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
