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
    use super::Bridge;
    use crate::config::Config;
    use crate::error::AGENT_UNAVAILABLE;
    use agent_client_protocol::{
        Agent, ErrorCode, NewSessionRequest, NewSessionResponse, SessionId,
    };
    use opentelemetry::Value;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{
        PeriodicReader, SdkMeterProvider, in_memory_exporter::InMemoryMetricExporter,
    };
    use std::time::Duration;
    use trogon_nats::AdvancedMockNatsClient;

    fn has_session_ready_error_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
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
                                    operation_ok = attr.value.as_str() == "session_ready";
                                } else if attr.key.as_str() == "reason" {
                                    reason_ok =
                                        attr.value.as_str() == "session_ready_publish_failed";
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

    fn assert_session_ready_error_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
    ) {
        assert!(
            has_session_ready_error_metric(finished_metrics),
            "expected acp.errors.total datapoint with operation=session_ready, reason=session_ready_publish_failed"
        );
    }

    fn has_new_session_metric(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
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
                                    method_ok = attr.value.as_str() == "new_session";
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

    fn assert_new_session_metric_recorded(
        finished_metrics: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
        expected_success: bool,
    ) {
        assert!(
            has_new_session_metric(finished_metrics, expected_success),
            "expected acp.request.count datapoint with method=new_session, success={}",
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
        assert_new_session_metric_recorded(&finished_metrics, true);
        provider.shutdown().unwrap();
    }

    #[tokio::test]
    async fn new_session_records_metrics_on_failure() {
        let (mock, bridge, exporter, provider) = mock_bridge_with_metrics();
        mock.fail_next_request();

        let _ = bridge.new_session(NewSessionRequest::new(".")).await;

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert_new_session_metric_recorded(&finished_metrics, false);
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_session_ready_error_metric_returns_false_when_metric_is_histogram() {
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
        assert!(!has_session_ready_error_metric(&finished_metrics));
        provider.shutdown().unwrap();
    }

    #[test]
    fn has_new_session_metric_returns_false_when_metric_is_histogram() {
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
        assert!(!has_new_session_metric(&finished_metrics, true));
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
        assert_session_ready_error_recorded(&finished_metrics);
        assert_new_session_metric_recorded(&finished_metrics, true);
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
