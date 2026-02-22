use super::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{AuthenticateRequest, AuthenticateResponse, Error, ErrorCode, Result};
use tracing::{info, instrument, warn};
use trogon_nats::NatsError;
use trogon_std::time::GetElapsed;

fn map_authenticate_error(e: NatsError) -> Error {
    match &e {
        NatsError::Timeout { subject } => {
            warn!(subject = %subject, "authenticate request timed out");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "Authenticate request timed out; agent may be overloaded or unavailable",
            )
        }
        NatsError::Request { subject, error } => {
            warn!(subject = %subject, error = %error, "authenticate NATS request failed");
            Error::new(
                ErrorCode::Other(AGENT_UNAVAILABLE).into(),
                "Agent unavailable",
            )
        }
        NatsError::Serialize(inner) => {
            warn!(error = %inner, "failed to serialize authenticate request");
            Error::new(
                ErrorCode::InternalError.into(),
                format!("Failed to serialize authenticate request: {}", inner),
            )
        }
        NatsError::Deserialize(inner) => {
            warn!(error = %inner, "failed to deserialize authenticate response");
            Error::new(
                ErrorCode::InternalError.into(),
                "Invalid response from agent",
            )
        }
        _ => {
            warn!(error = %e, "authenticate NATS request failed");
            Error::new(
                ErrorCode::InternalError.into(),
                "Authenticate request failed",
            )
        }
    }
}

#[instrument(
    name = "acp.authenticate",
    skip(bridge, args),
    fields(method_id = %args.method_id)
)]
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: AuthenticateRequest,
) -> Result<AuthenticateResponse> {
    let start = bridge.clock.now();

    info!(method_id = %args.method_id, "Authenticate request");
    let nats = bridge.nats();

    let result = nats::request_with_timeout::<N, AuthenticateRequest, AuthenticateResponse>(
        nats,
        &agent::authenticate(bridge.config.acp_prefix()),
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_authenticate_error);

    bridge.metrics.record_request(
        "authenticate",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::{Bridge, map_authenticate_error};
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{Agent, AuthenticateRequest, AuthenticateResponse, ErrorCode};
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;
    use trogon_nats::{AdvancedMockNatsClient, NatsError};

    fn assert_authenticate_metric_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        expected_success: bool,
    ) {
        let found = finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .any(|sm| {
                sm.metrics().any(|metric| {
                    if metric.name() != "acp.requests" {
                        return false;
                    }
                    let data = metric.data();
                    let sum = match data {
                        AggregatedMetrics::U64(MetricData::Sum(s)) => s,
                        _ => return false,
                    };
                    sum.data_points().any(|dp| {
                        let mut method_ok = false;
                        let mut success_ok = false;
                        for attr in dp.attributes() {
                            if attr.key.as_str() == "method" {
                                method_ok = attr.value.as_str() == "authenticate";
                            } else if attr.key.as_str() == "success" {
                                success_ok = attr.value == Value::from(expected_success);
                            }
                        }
                        method_ok && success_ok
                    })
                })
            });
        assert!(
            found,
            "expected acp.requests datapoint with method=authenticate, success={}",
            expected_success
        );
    }

    fn mock_bridge_with_metrics() -> (
        AdvancedMockNatsClient,
        Bridge<AdvancedMockNatsClient, trogon_std::time::SystemClock>,
        InMemoryMetricExporter,
        SdkMeterProvider,
    ) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("acp-nats-test");

        let mock = AdvancedMockNatsClient::new();
        let bridge = Bridge::new(
            mock.clone(),
            trogon_std::time::SystemClock,
            &meter,
            Config::for_test("acp"),
        );
        (mock, bridge, exporter, provider)
    }

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

    struct FailsSerialize;
    impl serde::Serialize for FailsSerialize {
        fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("test serialize failure"))
        }
    }

    #[tokio::test]
    async fn authenticate_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = AuthenticateResponse::default();
        set_json_response(&mock, "acp.agent.authenticate", &expected);

        let request = AuthenticateRequest::new("test");
        let result = bridge.authenticate(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn authenticate_returns_error_when_nats_request_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = AuthenticateRequest::new("test");
        let err = bridge.authenticate(request).await.unwrap_err();

        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn authenticate_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.authenticate", "not json".into());

        let request = AuthenticateRequest::new("test");
        let err = bridge.authenticate(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn authenticate_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.agent.authenticate",
            &AuthenticateResponse::default(),
        );

        let _ = bridge.authenticate(AuthenticateRequest::new("test")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_authenticate_metric_recorded(&finished_metrics, true);
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn authenticate_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge.authenticate(AuthenticateRequest::new("test")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_authenticate_metric_recorded(&finished_metrics, false);
        provider.shutdown().unwrap();
    }

    #[test]
    fn map_authenticate_error_timeout() {
        let err = map_authenticate_error(NatsError::Timeout {
            subject: "acp.agent.authenticate".into(),
        });
        assert!(err.to_string().contains("timed out"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_authenticate_error_request() {
        let err = map_authenticate_error(NatsError::Request {
            subject: "acp.agent.authenticate".into(),
            error: "connection refused".into(),
        });
        assert!(err.to_string().contains("Agent unavailable"));
        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[test]
    fn map_authenticate_error_serialize() {
        let serde_err = serde_json::to_vec(&FailsSerialize).unwrap_err();
        let err = map_authenticate_error(NatsError::Serialize(serde_err));
        assert!(err.to_string().contains("serialize"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_authenticate_error_deserialize() {
        let serde_err = serde_json::from_str::<AuthenticateResponse>("[]").unwrap_err();
        let err = map_authenticate_error(NatsError::Deserialize(serde_err));
        assert!(err.to_string().contains("Invalid response from agent"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[test]
    fn map_authenticate_error_other() {
        let err = map_authenticate_error(NatsError::Other("misc failure".into()));
        assert!(err.to_string().contains("Authenticate request failed"));
        assert_eq!(err.code, ErrorCode::InternalError);
    }
}
