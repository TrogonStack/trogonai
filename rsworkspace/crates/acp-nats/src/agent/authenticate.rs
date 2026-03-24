use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, agent};
use agent_client_protocol::{AuthenticateRequest, AuthenticateResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.authenticate",
    skip(bridge, args),
    fields(method_id = %args.method_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
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
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "authenticate",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::Bridge;
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
    use trogon_nats::AdvancedMockNatsClient;

    fn has_authenticate_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        expected_success: bool,
    ) -> bool {
        finished_metrics
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
            })
    }

    fn assert_authenticate_metric_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        expected_success: bool,
    ) {
        assert!(
            has_authenticate_metric(finished_metrics, expected_success),
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
            tokio::sync::mpsc::channel(1).0,
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
    async fn authenticate_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = AuthenticateResponse::new();
        set_json_response(&mock, "acp.agent.authenticate", &expected);

        let request = AuthenticateRequest::new("api-key");
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
    fn has_authenticate_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let counter = meter.u64_counter("acp.other_metric").build();
        counter.add(1, &[]);
        let histogram = meter
            .f64_histogram("acp.requests")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_authenticate_metric(&finished_metrics, true));
        provider.shutdown().unwrap();
    }
}
