use super::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::nats::{self, RequestClient, agent};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{Error, ErrorCode, ForkSessionRequest, ForkSessionResponse, Result};
use tracing::{info, instrument, warn};
use trogon_nats::NatsError;
use trogon_std::time::GetElapsed;

fn map_fork_session_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "fork_session request timed out");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "Fork session request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "fork_session NATS request failed");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                format!("Agent unavailable: {}", error),
            )
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize fork_session request");
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to serialize fork_session request: {}", inner),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize fork_session response");
            Error::new(
                ErrorCode::InternalError.into(),
                "Invalid response from agent",
            )
        }
        _ => {
            warn!(error = %e, "fork_session NATS request failed");
            Error::new(
                ErrorCode::InternalError.into(),
                "Fork session request failed",
            )
        }
    }
}

#[instrument(
    name = "acp.session.fork",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
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
    let nats = bridge.nats();
    let subject = agent::session_fork(bridge.config.acp_prefix(), session_id.as_str());

    let result =
        nats::request_with_timeout::<N, ForkSessionRequest, ForkSessionResponse>(
            nats,
            &subject,
            &args,
            bridge.config.operation_timeout,
        )
        .await
        .map_err(map_fork_session_error);

    bridge.metrics.record_request(
        "fork_session",
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
    use agent_client_protocol::{Agent, ErrorCode, ForkSessionRequest, ForkSessionResponse};
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
    async fn fork_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = ForkSessionResponse::new("fork-001");
        set_json_response(&mock, "acp.session-fork-001.agent.session.fork", &expected);

        let request = ForkSessionRequest::new("session-fork-001", "/workspace");
        let result = bridge.fork_session(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn fork_session_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = ForkSessionRequest::new("s1", "/workspace");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn fork_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.s1.agent.session.fork", "not json".into());

        let request = ForkSessionRequest::new("s1", "/workspace");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn fork_session_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = ForkSessionRequest::new("invalid.session.id", "/workspace");
        let err = bridge.fork_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[test]
    fn map_error_timeout() {
        let err = map_fork_session_error(NatsError::Timeout {
            subject: "acp.s1.agent.session.fork".into(),
        });
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_error_request() {
        let err = map_fork_session_error(NatsError::Request {
            subject: "acp.s1.agent.session.fork".into(),
            error: "connection refused".into(),
        });
        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_error_serialize() {
        let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
        let err = map_fork_session_error(NatsError::Serialize(serde_err));
        assert!(err.to_string().contains("serialize"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_error_deserialize() {
        let serde_err = serde_json::from_str::<ForkSessionResponse>("[]").unwrap_err();
        let err = map_fork_session_error(NatsError::Deserialize(serde_err));
        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_error_other() {
        let err = map_fork_session_error(NatsError::Other("misc failure".into()));
        assert!(err.to_string().contains("Fork session request failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    struct FailsSerialize;
    impl serde::Serialize for FailsSerialize {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("test serialize failure"))
        }
    }
}
