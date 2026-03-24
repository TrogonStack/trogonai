use super::Bridge;
use crate::error::map_nats_error;
use crate::ext_method_name::ExtMethodName;
use crate::nats::{self, RequestClient, agent};
use agent_client_protocol::{Error, ErrorCode, ExtRequest, ExtResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.ext",
    skip(bridge, args),
    fields(method = %args.method)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: ExtRequest,
) -> Result<ExtResponse> {
    let start = bridge.clock.now();

    info!(method = %args.method, "Extension method request");

    let method_name = ExtMethodName::new(&args.method).map_err(|e| {
        bridge.metrics.record_request(
            "ext_method",
            bridge.clock.elapsed(start).as_secs_f64(),
            false,
        );
        bridge
            .metrics
            .record_error("ext_method", "invalid_method_name");
        Error::new(
            ErrorCode::InvalidParams.into(),
            format!("Invalid method name: {}", e),
        )
    })?;

    let nats = bridge.nats();
    let subject = agent::ext(bridge.config.acp_prefix(), method_name.as_str());

    let result = nats::request_with_timeout::<N, ExtRequest, ExtResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout(),
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "ext_method",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use agent_client_protocol::{Agent, ErrorCode, ExtRequest, ExtResponse};
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use serde_json::value::RawValue;
    use std::time::Duration;
    use trogon_nats::AdvancedMockNatsClient;

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

    fn set_json_response<T: serde::Serialize>(
        mock: &AdvancedMockNatsClient,
        subject: &str,
        resp: &T,
    ) {
        let bytes = serde_json::to_vec(resp).unwrap();
        mock.set_response(subject, bytes.into());
    }

    fn has_request_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        method: &str,
        expected_success: bool,
    ) -> bool {
        finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .find(|m| m.name() == "acp.requests")
            .and_then(|metric| {
                let data = metric.data();
                if let AggregatedMetrics::U64(MetricData::Sum(s)) = data {
                    s.data_points()
                        .find(|dp| {
                            let mut method_ok = false;
                            let mut success_ok = false;
                            for attr in dp.attributes() {
                                if attr.key.as_str() == "method" {
                                    method_ok = attr.value.as_str() == method;
                                } else if attr.key.as_str() == "success" {
                                    success_ok = attr.value == Value::from(expected_success);
                                }
                            }
                            method_ok && success_ok
                        })
                        .map(|_| ())
                } else {
                    None
                }
            })
            .is_some()
    }

    fn has_error_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        operation: &str,
        reason: &str,
    ) -> bool {
        finished_metrics
            .iter()
            .flat_map(|rm| rm.scope_metrics())
            .flat_map(|sm| sm.metrics())
            .find(|m| m.name() == "acp.errors")
            .and_then(|metric| {
                let data = metric.data();
                if let AggregatedMetrics::U64(MetricData::Sum(s)) = data {
                    s.data_points()
                        .find(|dp| {
                            let mut operation_ok = false;
                            let mut reason_ok = false;
                            for attr in dp.attributes() {
                                if attr.key.as_str() == "operation" {
                                    operation_ok = attr.value.as_str() == operation;
                                } else if attr.key.as_str() == "reason" {
                                    reason_ok = attr.value.as_str() == reason;
                                }
                            }
                            operation_ok && reason_ok
                        })
                        .map(|_| ())
                } else {
                    None
                }
            })
            .is_some()
    }

    #[tokio::test]
    async fn ext_method_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let raw = RawValue::from_string(r#"{"result":"ok"}"#.to_string()).unwrap();
        let expected = ExtResponse::new(raw.into());
        set_json_response(&mock, "acp.agent.ext.my_method", &expected);

        let params = RawValue::from_string(r#"{"key":"value"}"#.to_string()).unwrap();
        let request = ExtRequest::new("my_method", params.into());
        let result = bridge.ext_method(request).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn ext_method_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("my_method", params.into());
        let err = bridge.ext_method(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(crate::error::AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn ext_method_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.agent.ext.my_method", "not json".into());

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("my_method", params.into());
        let err = bridge.ext_method(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn ext_method_validates_method_name() {
        let (_mock, bridge) = mock_bridge();
        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("method.*", params.into());
        let err = bridge.ext_method(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid method name"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn ext_method_records_error_metric_on_invalid_method_name() {
        let (_mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        let params = RawValue::from_string("{}".to_string()).unwrap();
        let request = ExtRequest::new("invalid method", params.into());

        let _ = bridge.ext_method(request).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_error_metric(&finished_metrics, "ext_method", "invalid_method_name"),
            "expected acp.errors with operation=ext_method, reason=invalid_method_name"
        );
        assert!(
            has_request_metric(&finished_metrics, "ext_method", false),
            "expected acp.requests with method=ext_method, success=false on validation failure"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ext_method_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        let raw = RawValue::from_string("{}".to_string()).unwrap();
        set_json_response(
            &mock,
            "acp.agent.ext.my_method",
            &ExtResponse::new(raw.into()),
        );

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let _ = bridge
            .ext_method(ExtRequest::new("my_method", params.into()))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "ext_method", true),
            "expected acp.requests with method=ext_method, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ext_method_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let params = RawValue::from_string("{}".to_string()).unwrap();
        let _ = bridge
            .ext_method(ExtRequest::new("my_method", params.into()))
            .await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "ext_method", false),
            "expected acp.requests with method=ext_method, success=false"
        );
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_request_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.requests")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_request_metric(&finished_metrics, "ext_method", true));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_error_metric_returns_false_when_metric_is_histogram() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_millis(100))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test");
        let histogram = meter
            .f64_histogram("acp.errors")
            .with_description("test")
            .build();
        histogram.record(1.0, &[]);
        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!has_error_metric(
            &finished_metrics,
            "ext_method",
            "invalid_method_name"
        ));
        provider.shutdown().unwrap();
    }
}
