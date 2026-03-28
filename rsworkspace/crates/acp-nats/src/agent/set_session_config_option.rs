use super::Bridge;
use super::js_request;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, session};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{
    Error, ErrorCode, Result, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};
use tracing::{info, instrument};
use trogon_nats::jetstream::{JetStreamConsumerFactory, JetStreamPublisher};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.set_config_option",
    skip(bridge, args),
    fields(session_id = %args.session_id, config_id = %args.config_id)
)]
pub async fn handle<
    N: RequestClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamConsumerFactory,
>(
    bridge: &Bridge<N, C, J>,
    args: SetSessionConfigOptionRequest,
) -> Result<SetSessionConfigOptionResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, config_id = %args.config_id, "Set session config option request");

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
    let subject = session::agent::set_config_option(prefix, session_id.as_str());

    let result = match bridge.js() {
        Some(js) => {
            let req_id = uuid::Uuid::new_v4().to_string();
            js_request::js_request::<J, _, SetSessionConfigOptionResponse, _>(
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
        None => nats::request_with_timeout::<
            N,
            SetSessionConfigOptionRequest,
            SetSessionConfigOptionResponse,
        >(
            bridge.nats(),
            &subject,
            &args,
            bridge.config.operation_timeout,
        )
        .await
        .map_err(map_nats_error),
    };

    bridge.metrics.record_request(
        "set_session_config_option",
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
        Agent, ErrorCode, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    };

    #[tokio::test]
    async fn set_session_config_option_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = SetSessionConfigOptionResponse::new(vec![]);
        set_json_response(&mock, "acp.session.s1.agent.set_config_option", &expected);

        let request = SetSessionConfigOptionRequest::new("s1", "theme", "dark");
        let result = bridge.set_session_config_option(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn set_session_config_option_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = SetSessionConfigOptionRequest::new("s1", "theme", "dark");
        let err = bridge.set_session_config_option(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn set_session_config_option_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.session.s1.agent.set_config_option", "not json".into());

        let request = SetSessionConfigOptionRequest::new("s1", "theme", "dark");
        let err = bridge.set_session_config_option(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn set_session_config_option_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = SetSessionConfigOptionRequest::new("invalid.session.id", "theme", "dark");
        let err = bridge.set_session_config_option(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn set_session_config_option_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.session.s1.agent.set_config_option",
            &SetSessionConfigOptionResponse::new(vec![]),
        );

        let _ = bridge
            .set_session_config_option(SetSessionConfigOptionRequest::new("s1", "theme", "dark"))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "set_session_config_option", true),
            "expected acp.requests with method=set_session_config_option, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn set_session_config_option_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge
            .set_session_config_option(SetSessionConfigOptionRequest::new("s1", "theme", "dark"))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "set_session_config_option", false),
            "expected acp.requests with method=set_session_config_option, success=false"
        );
        provider.shutdown().unwrap();
    }
}
