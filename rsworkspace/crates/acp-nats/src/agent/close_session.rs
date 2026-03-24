use super::Bridge;
use crate::error::map_nats_error;
use crate::nats::{self, RequestClient, agent};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{CloseSessionRequest, CloseSessionResponse, Error, ErrorCode, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.session.close",
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed>(
    bridge: &Bridge<N, C>,
    args: CloseSessionRequest,
) -> Result<CloseSessionResponse> {
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Close session request");

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
    let subject = agent::session_close(bridge.config.acp_prefix(), session_id.as_str());

    let result = nats::request_with_timeout::<N, CloseSessionRequest, CloseSessionResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout,
    )
    .await
    .map_err(map_nats_error);

    bridge.metrics.record_request(
        "close_session",
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
    use agent_client_protocol::{Agent, CloseSessionRequest, CloseSessionResponse, ErrorCode};
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
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

    #[tokio::test]
    async fn close_session_forwards_request_and_returns_response() {
        let (mock, bridge) = mock_bridge();
        let expected = CloseSessionResponse::new();
        set_json_response(&mock, "acp.s1.agent.session.close", &expected);

        let request = CloseSessionRequest::new("s1");
        let result = bridge.close_session(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn close_session_returns_error_when_nats_fails() {
        let (mock, bridge) = mock_bridge();
        mock.fail_next_request();

        let request = CloseSessionRequest::new("s1");
        let err = bridge.close_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::Other(AGENT_UNAVAILABLE));
    }

    #[tokio::test]
    async fn close_session_returns_error_when_response_is_invalid_json() {
        let (mock, bridge) = mock_bridge();
        mock.set_response("acp.s1.agent.session.close", "not json".into());

        let request = CloseSessionRequest::new("s1");
        let err = bridge.close_session(request).await.unwrap_err();

        assert_eq!(err.code, ErrorCode::InternalError);
    }

    #[tokio::test]
    async fn close_session_validates_session_id() {
        let (_mock, bridge) = mock_bridge();
        let request = CloseSessionRequest::new("invalid.session.id");
        let err = bridge.close_session(request).await.unwrap_err();

        assert!(err.to_string().contains("Invalid session ID"));
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn close_session_records_metrics_on_success() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        set_json_response(
            &mock,
            "acp.s1.agent.session.close",
            &CloseSessionResponse::new(),
        );

        let _ = bridge.close_session(CloseSessionRequest::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "close_session", true),
            "expected acp.requests with method=close_session, success=true"
        );
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn close_session_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge.close_session(CloseSessionRequest::new("s1")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            has_request_metric(&finished_metrics, "close_session", false),
            "expected acp.requests with method=close_session, success=false"
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
        assert!(!has_request_metric(
            &finished_metrics,
            "close_session",
            true
        ));
        provider.shutdown().unwrap();
    }
}
