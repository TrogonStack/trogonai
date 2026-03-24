use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{NewSessionRequest, NewSessionResponse, Result};
use tracing::{Span, info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.new",
    skip(bridge, args),
    fields(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), session_id = tracing::field::Empty)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: NewSessionRequest,
) -> Result<NewSessionResponse> {
    let start = bridge.clock.now();

    info!(cwd = ?args.cwd, mcp_servers = args.mcp_servers.len(), "New session request");

    let nats = bridge.nats();
    let subject = agent::session_new(bridge.config.acp_prefix());

    let result = nats::request_with_timeout::<N, NewSessionRequest, NewSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    if let Ok(ref response) = result {
        Span::current().record("session_id", response.session_id.to_string().as_str());
        info!(session_id = %response.session_id, "Session created");

        bridge.schedule_session_ready(response.session_id.clone());
    }

    bridge.metrics.record_request(
        "new_session",
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
        Agent, ErrorCode, NewSessionRequest, NewSessionResponse, SessionId,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn new_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let session_id = SessionId::from("test-session-1");
        let expected = NewSessionResponse::new(session_id.clone());
        set_json_response(&mock, "acp.agent.session.new", &expected);

        let request = NewSessionRequest::new(".");
        let result = bridge.new_session(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.session_id, session_id);
    }

    #[tokio::test]
    async fn new_session_returns_error_when_nats_request_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = NewSessionRequest::new(".");
        let err = bridge.new_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn new_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.session.new", "not json".into());

        let request = NewSessionRequest::new(".");
        let err = bridge.new_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn new_session_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        let session_id = SessionId::from("test-session-1");
        set_json_response(
            &mock,
            "acp.agent.session.new",
            &NewSessionResponse::new(session_id),
        );

        let _ = bridge.new_session(NewSessionRequest::new(".")).await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "new_session", true),
            "expected acp.requests with method=new_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn new_session_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge.new_session(NewSessionRequest::new(".")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "new_session", false),
            "expected acp.requests with method=new_session, success=false"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn new_session_records_error_when_session_ready_publish_fails() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        let session_id = SessionId::from("test-session-1");
        set_json_response(
            &mock,
            "acp.agent.session.new",
            &NewSessionResponse::new(session_id),
        );
        mock.fail_publish_count(4);

        let _ = bridge.new_session(NewSessionRequest::new(".")).await;

        tokio::time::sleep(Duration::from_millis(600)).await;
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_session_ready_error_metric(&finished_metrics),
            "expected acp.errors.total datapoint with operation=session_ready, reason=session_ready_publish_failed"
        );
        assert!(
            has_request_metric(&finished_metrics, "new_session", true),
            "expected acp.requests with method=new_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn new_session_publishes_session_ready_to_correct_subject() {
        let (mock, bridge) = mock_bridge();
        let session_id = SessionId::from("test-session-1");
        set_json_response(
            &mock,
            "acp.agent.session.new",
            &NewSessionResponse::new(session_id),
        );

        let _ = bridge.new_session(NewSessionRequest::new(".")).await;

        tokio::time::sleep(Duration::from_millis(300)).await;
        let published = mock.published_messages();
        assert!(
            published.contains(&"acp.test-session-1.agent.ext.session.ready".to_string()),
            "expected publish to acp.test-session-1.agent.ext.session.ready, got: {:?}",
            published
        );
    }
}
