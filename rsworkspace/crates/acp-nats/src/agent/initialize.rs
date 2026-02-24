use super::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{Error, ErrorCode, InitializeRequest, InitializeResponse, Result};
use tracing::{info, instrument, warn};
use trogon_nats::NatsError;

fn map_initialize_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "initialize request timed out");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "Initialize request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "initialize NATS request failed");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                format!("Agent unavailable: {}", error),
            )
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize initialize request");
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to serialize initialize request: {}", inner),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize initialize response");
            Error::new(
                ErrorCode::InternalError.into(),
                "Invalid response from agent",
            )
        }
        _ => {
            warn!(error = %e, "initialize NATS request failed");
            Error::new(ErrorCode::InternalError.into(), "Initialize request failed")
        }
    }
}

#[instrument(
    name = "acp.initialize",
    skip(bridge, args),
    fields(protocol_version = ?args.protocol_version)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: InitializeRequest,
) -> Result<InitializeResponse> {
    let client_name = args
        .client_info
        .as_ref()
        .map(|c| c.name.as_str())
        .unwrap_or("unknown");

    info!(client = %client_name, "Initialize request");

    let nats = bridge.nats();
    let subject = agent::initialize(bridge.config.acp_prefix());

    nats::request_with_timeout::<N, InitializeRequest, InitializeResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_initialize_error)
}

#[cfg(test)]
mod tests {
    use super::{Bridge, map_initialize_error};
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{
        Agent, ErrorCode, Implementation, InitializeRequest, InitializeResponse, ProtocolVersion,
    };
    use trogon_nats::{AdvancedMockNatsClient, NatsError};

    fn mock_bridge() -> (AdvancedMockNatsClient, Bridge<AdvancedMockNatsClient>) {
        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(mock.clone(), Config::for_test("acp"));
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

    struct FailsSerialize;
    impl serde::Serialize for FailsSerialize {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("test serialize failure"))
        }
    }

    #[tokio::test]
    async fn initialize_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = InitializeResponse::new(ProtocolVersion::LATEST);
        set_json_response(&mock, "acp.agent.initialize", &expected);

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let result = bridge.initialize(request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.protocol_version, ProtocolVersion::LATEST);
    }

    #[tokio::test]
    async fn initialize_logs_client_name_when_client_info_provided() {
        let (mock, bridge) = mock_bridge();
        let expected = InitializeResponse::new(ProtocolVersion::LATEST);
        set_json_response(&mock, "acp.agent.initialize", &expected);

        let request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new("my-client", "1.0.0"));
        let result = bridge.initialize(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn initialize_returns_error_when_nats_request_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let err = bridge.initialize(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn initialize_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.initialize", "not json".into());

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let err = bridge.initialize(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_initialize_error_timeout() {
        let err = map_initialize_error(NatsError::Timeout {
            subject: "acp.agent.initialize".into(),
        });
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_initialize_error_request() {
        let err = map_initialize_error(NatsError::Request {
            subject: "acp.agent.initialize".into(),
            error: "connection refused".into(),
        });
        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_initialize_error_serialize() {
        let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
        let err = map_initialize_error(NatsError::Serialize(serde_err));
        assert!(err.to_string().contains("serialize"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_initialize_error_deserialize() {
        let serde_err = serde_json::from_str::<InitializeResponse>("{}").unwrap_err();
        let err = map_initialize_error(NatsError::Deserialize(serde_err));
        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_initialize_error_other() {
        let err = map_initialize_error(NatsError::Other("misc failure".into()));
        assert!(err.to_string().contains("Initialize request failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }
}
