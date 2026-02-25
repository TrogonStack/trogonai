use super::Bridge;
use crate::error::AGENT_UNAVAILABLE;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, agent};
use agent_client_protocol::{Error, ErrorCode, InitializeRequest, InitializeResponse, Result};
use tracing::{info, instrument, warn};
use trogon_nats::NatsError;
use trogon_std::time::GetElapsed;

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
pub async fn handle<N: RequestClient + PublishClient + FlushClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: InitializeRequest,
) -> Result<InitializeResponse> {
    let start = bridge.clock.now();

    let client_name = args
        .client_info
        .as_ref()
        .map(|c| c.name.as_str())
        .unwrap_or("unknown");

    info!(client = %client_name, "Initialize request");

    let nats = bridge.nats();
    let subject = agent::initialize(bridge.config.acp_prefix());

    let result = nats::request_with_timeout::<N, InitializeRequest, InitializeResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_initialize_error);

    bridge.metrics.record_request(
        "initialize",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::{Bridge, map_initialize_error};
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{
        Agent, ErrorCode, Implementation, InitializeRequest, InitializeResponse, ProtocolVersion,
    };
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;
    use trogon_nats::{AdvancedMockNatsClient, NatsError};

    fn assert_initialize_metric_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        expected_success: bool,
    ) {
        let found = finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .any(|sm| {
                sm.metrics().any(|metric| {
                    if metric.name() != "acp.request.count" {
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
                                method_ok = attr.value.as_str() == "initialize";
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
            "expected acp.request.count datapoint with method=initialize, success={}",
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

    #[tokio::test]
    async fn initialize_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.agent.initialize",
            &InitializeResponse::new(ProtocolVersion::LATEST),
        );

        let _ = bridge
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_initialize_metric_recorded(&finished_metrics, true);
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn initialize_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge
            .initialize(InitializeRequest::new(ProtocolVersion::LATEST))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_initialize_metric_recorded(&finished_metrics, false);
        provider.shutdown().unwrap();
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
