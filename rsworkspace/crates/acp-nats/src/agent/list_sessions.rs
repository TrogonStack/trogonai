use super::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::nats::{self, RequestClient, agent};
use agent_client_protocol::{Error, ErrorCode, ListSessionsRequest, ListSessionsResponse, Result};
use tracing::{info, instrument, warn};
use trogon_nats::NatsError;
use trogon_std::time::GetElapsed;

fn map_list_sessions_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "list_sessions request timed out");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "List sessions request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "list_sessions NATS request failed");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                format!("Agent unavailable: {}", error),
            )
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize list_sessions request");
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to serialize list_sessions request: {}", inner),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize list_sessions response");
            Error::new(
                ErrorCode::InternalError.into(),
                "Invalid response from agent",
            )
        }
        _ => {
            warn!(error = %e, "list_sessions NATS request failed");
            Error::new(
                ErrorCode::InternalError.into(),
                "List sessions request failed",
            )
        }
    }
}

#[instrument(name = "acp.session.list", skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: ListSessionsRequest,
) -> Result<ListSessionsResponse> {
    let start = bridge.clock.now();

    info!("List sessions request");

    let nats = bridge.nats();
    let subject = agent::session_list(bridge.config.acp_prefix());

    let result =
        nats::request_with_timeout::<N, ListSessionsRequest, ListSessionsResponse>(
            nats,
            &subject,
            &args,
            bridge.config.operation_timeout,
        )
        .await
        .map_err(map_list_sessions_error);

    bridge.metrics.record_request(
        "list_sessions",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{Agent, ErrorCode, ListSessionsRequest, ListSessionsResponse};
    use trogon_nats::{AdvancedMockNatsClient, NatsError};

    fn mock_bridge() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
    ) {
        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &opentelemetry::global::meter("acp-nats-test"),
            Config::for_test("acp"),
            tokio::sync::mpsc::channel(1).0,
        );
        (mock, bridge)
    }

    fn set_json_response<T: serde::Serialize>(
        mock: &AdvancedMockNatsClient,
        subject: &str,
        resp: &T,
    ) {
        let bytes = serde_json::to_vec(resp).unwrap();
        mock.set_response(subject, bytes.into());
    }

    #[tokio::test]
    async fn list_sessions_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = ListSessionsResponse::new(vec![]);
        set_json_response(&mock, "acp.agent.session.list", &expected);

        let result = bridge.list_sessions(ListSessionsRequest::new()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().sessions.is_empty());
    }

    #[tokio::test]
    async fn list_sessions_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let err = bridge
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn list_sessions_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.session.list", "not json".into());

        let err = bridge
            .list_sessions(ListSessionsRequest::new())
            .await
            .unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_error_timeout() {
        let err = map_list_sessions_error(NatsError::Timeout {
            subject: "acp.agent.session.list".into(),
        });
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_error_request() {
        let err = map_list_sessions_error(NatsError::Request {
            subject: "acp.agent.session.list".into(),
            error: "connection refused".into(),
        });
        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_error_serialize() {
        let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
        let err = map_list_sessions_error(NatsError::Serialize(serde_err));
        assert!(err.to_string().contains("serialize"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_error_deserialize() {
        let serde_err = serde_json::from_str::<ListSessionsResponse>("[]").unwrap_err();
        let err = map_list_sessions_error(NatsError::Deserialize(serde_err));
        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_error_other() {
        let err = map_list_sessions_error(NatsError::Other("misc failure".into()));
        assert!(err.to_string().contains("List sessions request failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    struct FailsSerialize;
    impl serde::Serialize for FailsSerialize {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("test serialize failure"))
        }
    }
}
